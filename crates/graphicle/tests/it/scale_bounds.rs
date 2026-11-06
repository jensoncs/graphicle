//! Past the former hard bounds: a project with more than 5,000 documents and
//! one with more reviewed memories than recall ranks still list and answer.
use graphicle_runtime::store::{Store, s};
use serde_json::{Value, json};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    store: Store,
    workspace: String,
    key: String,
    repo: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("Scale").unwrap();
        let (workspace, key) = (s(&ws, "id").to_string(), s(&ws, "capability").to_string());
        let registered = store
            .register(
                &workspace,
                &key,
                &json!({"project_key":"scale","name":"Scale"}),
            )
            .unwrap();
        let repo = s(&registered, "id").to_string();
        Self {
            dir,
            store,
            workspace,
            key,
            repo,
        }
    }
    fn args(&self, mut args: Value) -> Value {
        args["repository_id"] = json!(self.repo);
        args
    }
    fn context(&self, question: &str) -> Value {
        self.store
            .project_context(
                &self.workspace,
                &self.key,
                &self.args(json!({"question":question})),
            )
            .unwrap()
    }
}

#[test]
fn context_searches_more_than_five_thousand_documents() {
    let f = Fixture::new();
    let added = f
        .store
        .connector_add(
            &f.workspace,
            &f.key,
            &f.args(json!({"kind":"jsonl","name":"Chat",
            "config":{"acl":{"workspace_readers":true,"credentials":[]}}})),
        )
        .unwrap();
    let connector = s(&added, "id").to_string();
    let total = 5100;
    for start in (0..total).step_by(1700) {
        let documents: Vec<Value> = (start..(start + 1700).min(total))
            .map(|i| {
                let content = if i == 5077 {
                    "user: The aquarium tank now holds six neon tetras.".to_string()
                } else {
                    format!("user: Note {i} about the vegetable garden.")
                };
                json!({"id":format!("s{i:05}"),"title":format!("Session {i}"),"content":content})
            })
            .collect();
        f.store
            .import_documents(
                &f.workspace,
                &f.key,
                &f.args(json!({"connector_id":connector,"documents":documents})),
            )
            .unwrap();
    }
    let packet = f.context("How many neon tetras does the aquarium tank hold?");
    let coverage = &packet["coverage"];
    assert_eq!(coverage["source_documents"], total, "{coverage}");
    assert_eq!(coverage["passage_limit_reached"], false, "{coverage}");
    assert_eq!(coverage["omitted_documents"], 0, "{coverage}");
    assert_eq!(coverage["omitted_documents_are"], "none", "{coverage}");
    let needle = format!("connector:{connector}:s05077");
    assert!(
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == json!(needle)),
        "{}",
        packet["evidence"]
    );
}

/// The full path's raw-word preselection misses a stemmed match in an early
/// file. Indexed retrieval must search it even when a filter or graph is on,
/// both above the document bound and below it but above the passage bound.
#[test]
fn indexed_graph_and_filters_search_beyond_chunking_bounds() {
    use graphicle_runtime::{passage_index, query};
    let f = Fixture::new();
    let total = graphicle_memory::ingest::MAX_DOCUMENTS + 1;
    let needle = "Workers retry twice after a transient failure.\n";
    let filler = "The vegetable garden needs compost.\n";
    let needle_sha = graphicle_memory::text::sha256(needle.as_bytes());
    let filler_sha = graphicle_memory::text::sha256(filler.as_bytes());
    // Populate the same content-addressed rows admission writes, without
    // making 50,001 filesystem entries or testing parsers in a recall test.
    f.store
        .transaction(|db| {
            for (sha, text) in [(&needle_sha, needle), (&filler_sha, filler)] {
                db.execute(
                    "INSERT INTO repo_objects VALUES(?,?,?,?)",
                    rusqlite::params![f.repo, sha, text, text.len()],
                )?;
            }
            let mut insert = db.prepare_cached("INSERT INTO repo_heads VALUES(?,?,?)")?;
            for i in 0..total {
                insert.execute(rusqlite::params![
                    f.repo,
                    format!("notes/{i:05}.txt"),
                    if i == 0 { &needle_sha } else { &filler_sha }
                ])?;
            }
            Ok(())
        })
        .unwrap();
    passage_index::update(&f.store, &f.repo, true).unwrap();
    f.store
        .authorized(&f.workspace, &f.key, "read", |db, p| {
            let repository = graphicle_runtime::store::repo(db, p, &f.repo)?;
            let mut snap = query::snapshot(db, &repository, p)?;
            let bounded = query::passages_for(&mut snap, "retrying")?;
            assert_eq!(bounded.rows.len(), graphicle_memory::ingest::MAX_PASSAGES);
            assert!(
                bounded
                    .rows
                    .iter()
                    .all(|r| r["source"]["id"] != "file:notes/00000.txt")
            );
            Ok(())
        })
        .unwrap();
    f.store
        .note(
            &f.workspace,
            &f.key,
            &f.args(json!({
                "title":"Operator procedure", "body":"Follow the operating procedure.",
                "dependencies":["notes/00000.txt", "notes/39999.txt"]
            })),
        )
        .unwrap();
    let request = |extra: Value| {
        let mut args = f.args(json!({"question":"retrying", "max_tokens":1000,
            "max_chars":4000, "graph_retrieval":true}));
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        args
    };
    let verify = |packet: &Value, count: usize| {
        assert_eq!(packet["passage_index"]["status"], "ready");
        assert_eq!(packet["coverage"]["source_documents"], count);
        assert_eq!(packet["coverage"]["source_passages"], count);
        assert_eq!(packet["coverage"]["omitted_documents"], 0);
        assert_eq!(packet["coverage"]["passage_limit_reached"], false);
        assert!(
            packet["coverage"]
                .get("omitted_by_document_limit")
                .is_none()
        );
        assert!(packet["passage_index"]["loaded_files"].as_u64().unwrap() < 100);
        assert!(packet["tokens"].as_u64().unwrap() <= 1000);
        let evidence = packet["evidence"].as_array().unwrap();
        assert!(
            evidence.iter().any(|e| e["id"] == "file:notes/00000.txt"),
            "{packet}"
        );
        for e in evidence {
            let (content, sha) = if e["id"] == "file:notes/00000.txt" {
                (needle, &needle_sha)
            } else {
                (filler, &filler_sha)
            };
            assert_eq!(e["sha256"], *sha);
            assert_eq!(
                e["text"],
                content
                    .get(
                        e["byte_start"].as_u64().unwrap() as usize
                            ..e["byte_end"].as_u64().unwrap() as usize
                    )
                    .unwrap()
            );
        }
    };
    for extra in [
        json!({}),
        json!({"include":["file:notes/"]}),
        json!({"include":["file:notes/"],"graph_retrieval":false}),
    ] {
        let args = request(extra);
        let packet = f
            .store
            .project_context(&f.workspace, &f.key, &args)
            .unwrap();
        verify(&packet, total);
        if args["graph_retrieval"] == true {
            assert_eq!(packet["mechanisms"]["graph_local_status"], "degraded");
            assert_eq!(packet["mechanisms"]["graph_local_degraded"]["nodes"], total);
        }
    }
    // 40,001 documents fit the property graph. Its seed must use the index
    // too, then reach the other dependency through the reviewed memory.
    let filtered = request(json!({"include":["file:notes/0", "file:notes/1",
        "file:notes/2", "file:notes/3", "file:notes/40000.txt"]}));
    let packet = f
        .store
        .project_context(&f.workspace, &f.key, &filtered)
        .unwrap();
    verify(&packet, 40_001);
    assert_eq!(packet["mechanisms"]["graph_local_searched"], true);
    assert!(
        packet["mechanisms"]["graph_candidate_passages"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["id"].as_str().unwrap() <= "file:notes/40000.txt")
    );
    // Missing index entries still read the current, hash-verified source.
    f.store
        .connect()
        .unwrap()
        .execute(
            "DELETE FROM am_passage_files WHERE repository_id=? AND path='notes/00000.txt'",
            [&f.repo],
        )
        .unwrap();
    let packet = f
        .store
        .project_context(&f.workspace, &f.key, &request(json!({})))
        .unwrap();
    verify(&packet, total);
    assert_eq!(packet["passage_index"]["read_here"], 1);
    f.store
        .connect()
        .unwrap()
        .execute(
            "UPDATE repo_objects SET content='corrupt' WHERE repository_id=? AND sha256=?",
            rusqlite::params![f.repo, needle_sha],
        )
        .unwrap();
    assert!(
        f.store
            .project_context(&f.workspace, &f.key, &request(json!({})))
            .is_err()
    );
    // A deleted source and its dependent note cannot reappear from the index.
    f.store
        .connect()
        .unwrap()
        .execute(
            "DELETE FROM repo_heads WHERE repository_id=? AND path='notes/00000.txt'",
            [&f.repo],
        )
        .unwrap();
    let packet = f
        .store
        .project_context(
            &f.workspace,
            &f.key,
            &request(json!({"graph_retrieval":false})),
        )
        .unwrap();
    assert_eq!(packet["coverage"]["source_documents"], total - 1);
    assert!(
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["id"] != "file:notes/00000.txt")
    );
    assert!(packet["memories"].as_array().unwrap().is_empty());
}

#[test]
fn oversized_unindexed_connectors_keep_explicit_coverage_limits() {
    let f = Fixture::new();
    let content = "The aquarium holds six neon tetras.";
    let sha = graphicle_memory::text::sha256(content.as_bytes());
    f.store
        .transaction(|db| {
            db.execute(
                "INSERT INTO repo_objects VALUES(?,?,?,?)",
                rusqlite::params![f.repo, sha, content, content.len()],
            )?;
            db.execute(
                "INSERT INTO repo_heads VALUES(?,'fish.txt',?)",
                rusqlite::params![f.repo, sha],
            )?;
            Ok(())
        })
        .unwrap();
    graphicle_runtime::passage_index::update(&f.store, &f.repo, true).unwrap();
    let connector = f.store.connector_add(&f.workspace, &f.key, &f.args(json!({
        "kind":"jsonl", "name":"Notes", "config":{"acl":{"workspace_readers":true,"credentials":[]}}
    }))).unwrap();
    f.store.import_documents(&f.workspace, &f.key, &f.args(json!({
        "connector_id":connector["id"], "documents":[{"id":"seed","content":content,"title":"Aquarium"}]
    }))).unwrap();
    f.store.transaction(|db| {
        let mut insert = db.prepare_cached("INSERT INTO am_connector_docs SELECT connector_id,?,content,sha256,metadata,acl,present,generation FROM am_connector_docs WHERE connector_id=? AND id='seed'")?;
        for i in 0..graphicle_memory::ingest::MAX_PASSAGES {
            insert.execute(rusqlite::params![format!("note{i:05}"), connector["id"].as_str().unwrap()])?;
        }
        Ok(())
    }).unwrap();
    let args =
        f.args(json!({"question":"aquarium", "graph_retrieval":false, "memory_retrieval":false}));
    let packet = f
        .store
        .project_context(&f.workspace, &f.key, &args)
        .unwrap();
    assert!(packet.get("passage_index").is_none());
    assert_eq!(packet["coverage"]["passage_limit_reached"], true);
    assert!(packet["coverage"]["omitted_documents"].as_u64().unwrap() > 0);
    // Excluded connectors do not force a fallback or affect indexed coverage.
    let mut filtered = args;
    filtered["include"] = json!(["file:"]);
    let packet = f
        .store
        .project_context(&f.workspace, &f.key, &filtered)
        .unwrap();
    assert_eq!(packet["passage_index"]["status"], "ready");
    assert_eq!(packet["coverage"]["source_documents"], 1);
    assert_eq!(packet["coverage"]["omitted_documents"], 0);
}

/// A current query file and file relationships can serve a large catalogue
/// without opening the resident graph. Filtering back into the property-graph
/// budget, or losing the saved relationships, must still load it.
#[test]
fn over_budget_graph_uses_saved_relationships_and_keeps_fallbacks() {
    use graphicle::{graph::Graph, session::registry};
    use graphicle_runtime::{graph_projection, indexed_query, passage_index, query_graph};
    let f = Fixture::new();
    let total = graphicle_runtime::graph::MAX_NODES + 1;
    let content = "The garden needs compost.\n";
    let needle = "The aquarium holds six neon tetras.\n";
    let sha = graphicle_memory::text::sha256(content.as_bytes());
    let needle_sha = graphicle_memory::text::sha256(needle.as_bytes());
    f.store
        .transaction(|db| {
            for (sha, text) in [(&sha, content), (&needle_sha, needle)] {
                db.execute(
                    "INSERT INTO repo_objects VALUES(?,?,?,?)",
                    rusqlite::params![f.repo, sha, text, text.len()],
                )?;
            }
            let mut insert = db.prepare_cached("INSERT INTO repo_heads VALUES(?,?,?)")?;
            for i in 0..total {
                insert.execute(rusqlite::params![
                    f.repo,
                    format!("notes/{i:05}.txt"),
                    if i == 0 { &needle_sha } else { &sha }
                ])?;
            }
            Ok(())
        })
        .unwrap();
    passage_index::update(&f.store, &f.repo, true).unwrap();
    // Two connected files suffice to check the lift. The other plain-text
    // documents exercise the catalogue budget without testing graph extraction.
    let graph = Graph::from_node_link(&json!({"directed":true,"multigraph":false,
        "nodes":[
            {"id":"a","kind":"file","source_file":"notes/00000.txt","label":"Aquarium","line":1},
            {"id":"b","kind":"file","source_file":"notes/49999.txt","label":"Garden","line":1}
        ],
        "links":[{"source":"a","target":"b","relation":"imports","source_location":"L1"}]
    }), "links").unwrap();
    let db = f.store.connect().unwrap();
    let episode = graph_projection::current_episode(&db, &f.repo).unwrap();
    graph_projection::persist(&db, &f.repo, &episode, &graph).unwrap();
    assert_eq!(
        query_graph::ensure(&db, &f.repo, true).unwrap()["status"],
        "written"
    );
    let reference = graph_projection::graph_ref(&f.repo, &episode);
    let edges = graph_projection::cache_root(&db, &f.repo)
        .unwrap()
        .join("query")
        .join(format!("{episode}.files"));
    let saved = std::fs::read(&edges).unwrap();
    drop(db);
    let run = |extra: Value| {
        let mut args = json!({"question":"aquarium", "graph_retrieval":true,
            "memory_retrieval":false,"max_tokens":2000});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        f.store
            .authorized(&f.workspace, &f.key, "read", |db, p| {
                let repository = graphicle_runtime::store::repo(db, p, &f.repo)?;
                indexed_query::packet(db, p, &repository, &args)
            })
            .unwrap()
            .unwrap()
    };
    registry().close(&reference);
    let (snap, expected) = run(json!({}));
    assert!(
        registry().get(&reference).is_err(),
        "large query opened the resident graph"
    );
    assert!(snap.graph.get("graph_nodes").is_none());
    assert_eq!(expected["mechanisms"]["graph_local_status"], "degraded");
    assert_eq!(expected["mechanisms"]["static_relationship_edges"], 1);
    assert_eq!(expected["coverage"]["omitted_documents"], 0);
    assert!(
        expected["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "file:notes/00000.txt")
    );
    // Use the filtered count, and keep the exact boundary inclusive.
    let (snap, filtered) = run(json!({"include":["file:notes/0","file:notes/1",
        "file:notes/2","file:notes/3","file:notes/4"]}));
    assert_eq!(filtered["coverage"]["source_documents"], total - 1);
    assert_eq!(filtered["mechanisms"]["graph_local_status"], "searched");
    assert_eq!(snap.graph["graph_nodes"], 2);
    assert!(registry().get(&reference).is_ok());
    for damage in ["missing", "truncated", "corrupt"] {
        registry().close(&reference);
        match damage {
            "missing" => std::fs::remove_file(&edges).unwrap(),
            "truncated" => std::fs::write(&edges, &saved[..saved.len() / 2]).unwrap(),
            _ => {
                let mut bytes = saved.clone();
                let at = bytes.len() / 2;
                bytes[at] ^= 0x80;
                std::fs::write(&edges, bytes).unwrap();
            }
        }
        let (snap, actual) = run(json!({}));
        assert_eq!(snap.graph["graph_nodes"], 2, "{damage}");
        assert!(registry().get(&reference).is_ok(), "{damage}");
        assert_eq!(actual, expected, "{damage}");
        std::fs::write(&edges, &saved).unwrap();
    }
    registry().close(&reference);
}

#[test]
fn memories_past_the_recall_bound_list_and_recall_the_newest() {
    let f = Fixture::new();
    let extra = 50;
    let total = graphicle_memory::retrieval::MAX_RECALL_MEMORIES + extra;
    assert!(total > 10_000, "past the former listing cap");
    {
        // The row shape `memory::save` writes; revision history is not read here.
        let mut db = rusqlite::Connection::open(f.dir.path().join("reviews.sqlite3")).unwrap();
        let tx = db.transaction().unwrap();
        for i in 0..total {
            let (title, body) = match i {
                0 => (
                    "Old aquarium".to_string(),
                    "The aquarium tank holds nine guppies.".to_string(),
                ),
                7321 => (
                    "Aquarium".to_string(),
                    "The aquarium tank holds six neon tetras.".to_string(),
                ),
                _ => (
                    format!("Garden note {i}"),
                    format!("Bed {i} of the vegetable garden needs compost."),
                ),
            };
            // The first `extra` are the least recently updated.
            let updated = if i < extra {
                "2020-01-01T00:00:00.000000Z"
            } else {
                "2026-09-30T00:00:00.000000Z"
            };
            tx.execute(
                "INSERT INTO repo_memories(id,repository_id,kind,revision,state,title,body,data,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![format!("m{i:05}"), f.repo, "fact", 1, "reviewed", title, body, "{}", updated, updated],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let status = f
        .store
        .project_status(&f.workspace, &f.key, &f.args(json!({})))
        .unwrap();
    assert_eq!(status["memories"], total, "{}", status["memories"]);
    assert_eq!(status["stale_memories"], 0);
    let packet = f.context("How many fish does the aquarium tank hold?");
    assert_eq!(
        packet["coverage"]["omitted_reviewed_memories"], extra,
        "{}",
        packet["coverage"]
    );
    let text = packet.to_string();
    assert!(
        text.contains("six neon tetras"),
        "recall missed the reviewed memory: {}",
        packet["memories"]
    );
    assert!(
        !text.contains("nine guppies"),
        "an omitted (oldest) memory was recalled"
    );
}
