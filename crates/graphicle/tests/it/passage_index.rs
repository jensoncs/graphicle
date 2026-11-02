//! The passage index: a question looked up in the index gets the candidates
//! and evidence the full path gives (`GRAPHICLE_PASSAGE_INDEX=0`), also when
//! the relationship lift reaches more passages than `rank` keeps, and then
//! sends `rank` only those it can keep.
use graphicle_runtime::platform::CanonicalExt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tempfile::TempDir;

const HUB: &str = "\"\"\"The metric registry.\"\"\"\n\nREGISTRY = {}\n\n\
def hub_metric_register(name, kind=\"gauge\"):\n    \
\"\"\"Register the gauge metric under name in the registry.\"\"\"\n    \
REGISTRY[name] = kind\n    return REGISTRY[name]\n";

const WORDS: &[&str] = &[
    "fold", "window", "batch", "quorum", "ledger", "cursor", "shard", "replica", "lease", "beacon",
    "harbor", "lantern", "meadow", "orchard", "pebble", "quiver", "ripple", "saddle", "thimble",
    "velvet", "wander", "yonder", "zephyr", "anchor", "bramble", "cobalt", "dapple", "ember",
];

/// One of the files that import the hub: 32 functions of about 600
/// characters (about 8 passages), some of which name the question's words.
fn spoke(i: usize) -> String {
    let mut text = format!(
        "from hub import hub_metric_register\n\n\ndef setup_{i}():\n    \
         return hub_metric_register(\"spoke_{i}_total\")\n"
    );
    for j in 0..32 {
        let mut doc = String::new();
        for k in 0..70 {
            doc.push_str(WORDS[(i * 7 + j * 3 + k * 5) % WORDS.len()]);
            doc.push(if k % 10 == 9 { '\n' } else { ' ' });
        }
        if (i + j).is_multiple_of(3) {
            doc.push_str("records the gauge metric\n");
        }
        if (i + j).is_multiple_of(5) {
            doc.push_str("see the registry\n");
        }
        text.push_str(&format!(
            "\n\ndef step_{i}_{j}(values):\n    \"\"\"\n{doc}\"\"\"\n    total = 0\n    \
             for value in values:\n        total += value * {j}\n    return total\n"
        ));
    }
    text
}

struct Home {
    _dir: TempDir,
    home: PathBuf,
    data: PathBuf,
    app: PathBuf,
}

impl Home {
    fn new(files: &[(String, String)]) -> Self {
        let dir = TempDir::new().unwrap();
        let home = dir.path().canonical().unwrap();
        let app = home.join("app");
        std::fs::create_dir_all(&app).unwrap();
        let this = Self {
            data: home.join("data"),
            app,
            home,
            _dir: dir,
        };
        this.git(&["init", "-q", "-b", "main"]);
        for (path, text) in files {
            let path = this.app.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        this.git(&["add", "-A"]);
        this.git(&["commit", "-q", "-m", "one"]);
        this
    }

    fn git(&self, args: &[&str]) {
        let mut command = Command::new("git");
        crate::child_env::home(&mut command, &self.home)
            .arg("-C")
            .arg(&self.app)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = crate::fork_guard::spawn(&mut command)
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The JSON the program prints in the repository, with `env` set.
    fn json(&self, args: &[&str], env: &[(&str, &str)]) -> Value {
        let mut command = Command::new(env!("CARGO_BIN_EXE_graphicle"));
        crate::child_env::home(&mut command, &self.home)
            .current_dir(&self.app)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GRAPHICLE_DATA_DIR")
            .env_remove("GRAPHICLE_PASSAGE_INDEX")
            .env_remove("GRAPHICLE_DEBUG_RANK")
            .arg("--data-dir")
            .arg(&self.data)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let output = crate::fork_guard::spawn(&mut command)
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "{args:?} printed no JSON: {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

/// The `rank` calls a `GRAPHICLE_DEBUG_RANK` file records, in order.
fn ranks(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn listed(packet: &Value, field: &str, key: &str) -> Vec<String> {
    packet[field]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| item[key].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn reviewed_memory_uses_the_index_without_changing_graph_recall() {
    let mut files = vec![("hub.py".to_string(), HUB.to_string())];
    for i in 0..24 {
        files.push((format!("spokes/spoke_{i:03}.py"), spoke(i)));
    }
    for i in 0..350 {
        files.push((
            format!("unrelated/other_{i:03}.txt"),
            "Distant unrelated orchard notes.\n".repeat(30),
        ));
    }
    let h = Home::new(&files);
    h.json(&["sync", "."], &[]);
    let arguments = h.home.join("arguments.json");
    let call = |operation: &str, args: Value, env: &[(&str, &str)]| {
        std::fs::write(&arguments, args.to_string()).unwrap();
        h.json(
            &[
                "call",
                operation,
                "--arguments",
                arguments.to_str().unwrap(),
            ],
            env,
        )
    };
    let note = call(
        "note",
        json!({"title":"Metric registration policy",
        "body":"Gauge metrics must use the central registry through hub_metric_register.",
        "dependencies":["hub.py"]}),
        &[],
    );
    assert_eq!(note["state"], "reviewed");
    let store = graphicle_runtime::store::Store::open(&h.data).unwrap();
    let db = store.connect().unwrap();
    let repository: String = db
        .query_row("SELECT id FROM repo_projects", [], |r| r.get(0))
        .unwrap();
    assert!(graphicle_runtime::indexed_query::ready(&db, &repository).unwrap());
    assert_eq!(
        graphicle_runtime::indexed_query::readiness(&db, &repository).unwrap()["state"],
        "ready"
    );
    assert_eq!(
        graphicle_runtime::indexed_query::hook_route(&db, &repository).unwrap(),
        graphicle_runtime::indexed_query::HookRoute::Full
    );
    drop(db);
    let compare = |extra: Value| {
        let mut args = json!({"question":"Where does hub_metric_register register the gauge metric?",
            "debug_candidates":400});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let reused = call("context", args.clone(), &[]);
        let full = call("context", args, &[("GRAPHICLE_PASSAGE_INDEX", "0")]);
        let mut comparable = reused.clone();
        comparable.as_object_mut().unwrap().remove("passage_index");
        assert_eq!(comparable, full);
        reused
    };
    let packet = compare(json!({}));
    assert_eq!(packet["passage_index"]["status"], "ready");
    assert!(packet["passage_index"]["loaded_files"].as_u64().unwrap() < 50);
    assert_eq!(packet["mechanisms"]["graph_local_searched"], true);
    assert!(
        packet["mechanisms"]["graph_candidate_passages"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        packet["memories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == note["id"])
    );
    compare(json!({"include":["file:hub.py"]}));
    compare(json!({"max_tokens":256,"max_chars":2000}));
    compare(json!({"graph_retrieval":true}));
    compare(json!({"graph_retrieval":true,"memory_retrieval":false}));
    compare(json!({"graph_retrieval":false}));
    compare(json!({"mode":"relationships","needs":["registry"]}));
    // A changed dependency must invalidate the memory even after layouts
    // have been saved; identical output alone would not detect stale recall.
    std::fs::write(h.app.join("hub.py"), HUB.replace("gauge", "counter")).unwrap();
    h.json(&["sync", "."], &[]);
    let changed = compare(json!({}));
    assert!(
        !changed["memories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == note["id"])
    );
}

#[test]
fn indexed_claims_keep_cited_quotes_and_recheck_connector_permissions() {
    use graphicle_runtime::{
        claims, local, memory,
        store::{Store, repo, s},
    };
    let quote = "Mira manages the midnight response.";
    let h = Home::new(&[(
        "notes.txt".into(),
        format!("{}\n{quote}", "Unrelated history.\n".repeat(500)),
    )]);
    h.json(&["sync", "."], &[]);
    let store = Store::open(&h.data).unwrap();
    let session = local::session(&store, false).unwrap();
    let repository: String = store
        .connect()
        .unwrap()
        .query_row("SELECT id FROM repo_projects", [], |r| r.get(0))
        .unwrap();
    let args = |mut value: Value| {
        value["repository_id"] = json!(repository);
        value
    };
    let connector = store.connector_add(&session.workspace, &session.capability,
        &args(json!({"name":"Private fixture", "kind":"jsonl", "config":{"acl":{"workspace_readers":true,"credentials":[]}}}))).unwrap();
    let private_quote = "Mira's access code is Copperfern.";
    store.import_documents(&session.workspace, &session.capability,
        &args(json!({"connector_id":connector["id"],"mode":"delta","documents":[{"id":"access","title":"Mira access","content":private_quote}]}))).unwrap();
    let connector_source = format!("connector:{}:access", s(&connector, "id"));
    for (source, statement, relation, object) in [
        ("file:notes.txt", quote, "manages", "midnight response"),
        (
            connector_source.as_str(),
            private_quote,
            "access_code",
            "Copperfern",
        ),
    ] {
        let proposed = store.authorized(&session.workspace, &session.capability, "propose", |db, principal| {
            let repository = repo(db, principal, &repository)?;
            let docs = memory::documents(db, &repository, principal)?;
            let sources: Vec<Value> = docs.iter().filter(|d| d["id"] == source)
                .map(|d| json!({"id":d["id"],"text":d["content"],"sha256":d["sha256"]})).collect();
            claims::propose(db, &repository, principal, &json!([{
                "statement":statement,"subject":"Mira","relation":relation,"object":object,
                "speaker":"user","modality":"report","conditions":[],"action":"ADD","target_id":null,
                "valid_at":null,"invalid_at":null,"time_basis":"unknown","anchor_id":null,
                "evidence":[{"id":source,"quote":statement}]
            }]), &sources, &[], &json!({"model":"fixture"}))
        }).unwrap();
        let draft = &proposed["drafts"][0];
        store
            .review_memory(
                &session.workspace,
                &session.capability,
                &args(json!({
                    "memory_id":draft["memory_id"],"expected_revision":draft["revision"],
                    "decision":"approve","maintainer":"Fixture","reason":"Checked original quote"
                })),
            )
            .unwrap();
    }
    let path = h.home.join("claims-question.json");
    let request = args(
        json!({"question":"What does Mira manage and what is her access code?", "debug_candidates":400}),
    );
    std::fs::write(&path, request.to_string()).unwrap();
    let call = ["call", "context", "--arguments", path.to_str().unwrap()];
    let mut indexed = h.json(&call, &[]);
    assert_eq!(indexed["passage_index"]["status"], "ready");
    assert!(indexed["markdown"].as_str().unwrap().contains(quote));
    assert!(indexed.to_string().contains("Copperfern"));
    indexed.as_object_mut().unwrap().remove("passage_index");
    assert_eq!(indexed, h.json(&call, &[("GRAPHICLE_PASSAGE_INDEX", "0")]));

    let reader = store
        .grant(
            &session.workspace,
            &session.capability,
            &json!({"label":"Reader","role":"reader"}),
        )
        .unwrap();
    let read = || {
        store
            .project_context(&session.workspace, s(&reader, "capability"), &request)
            .unwrap()
    };
    assert!(read().to_string().contains("Copperfern"));
    store.connector_sharing(&session.workspace, &session.capability,
        &args(json!({"connector_id":connector["id"],"acl":{"workspace_readers":false,"credentials":[]}}))).unwrap();
    let hidden = read();
    assert_eq!(hidden["passage_index"]["status"], "ready");
    assert!(!hidden.to_string().contains("Copperfern"));
    assert!(hidden["markdown"].as_str().unwrap().contains(quote));
    store
        .revoke(&session.workspace, &session.capability, s(&reader, "id"))
        .unwrap();
    assert!(
        store
            .project_context(&session.workspace, s(&reader, "capability"), &request)
            .is_err()
    );
}

#[test]
fn a_lift_past_what_rank_keeps_gives_the_full_paths_candidates() {
    let mut files = vec![("hub.py".to_string(), HUB.to_string())];
    for i in 0..150 {
        files.push((format!("spokes/spoke_{i:03}.py"), spoke(i)));
    }
    let h = Home::new(&files);
    let synced = h.json(&["sync", "."], &[]);
    assert!(
        ["built", "current"].contains(&synced["graph"]["status"].as_str().unwrap_or("")),
        "{synced:#}"
    );
    let arguments = h.home.join("question.json");
    std::fs::write(
        &arguments,
        json!({"question":"Where does hub_metric_register register the gauge metric in the registry?",
            "debug_candidates":400})
        .to_string(),
    )
    .unwrap();
    let call = [
        "call",
        "context",
        "--arguments",
        arguments.to_str().unwrap(),
    ];
    let (index_ranks, full_ranks) = (h.home.join("index.jsonl"), h.home.join("full.jsonl"));
    let indexed = h.json(
        &call,
        &[("GRAPHICLE_DEBUG_RANK", index_ranks.to_str().unwrap())],
    );
    let full = h.json(
        &call,
        &[
            ("GRAPHICLE_PASSAGE_INDEX", "0"),
            ("GRAPHICLE_DEBUG_RANK", full_ranks.to_str().unwrap()),
        ],
    );
    assert_eq!(indexed["passage_index"]["status"], "ready", "{indexed:#}");
    assert!(full.get("passage_index").is_none(), "{full:#}");
    // The hub leads, so the lift reaches every other file.
    let candidates = listed(&indexed, "candidates_debug", "identity");
    assert!(
        candidates[..4]
            .iter()
            .any(|id| id.starts_with("file:hub.py:")),
        "{candidates:?}"
    );
    assert_eq!(candidates, listed(&full, "candidates_debug", "identity"));
    assert_eq!(
        listed(&indexed, "evidence", "id"),
        listed(&full, "evidence", "id")
    );
    assert_eq!(indexed["markdown"], full["markdown"]);
    // The full path ranks every passage in one call. The index's first call
    // ranks the arms' passages; its second adds at most MAX_FUSED of the
    // lifted ones, where the lift reaches far more.
    let (index_calls, full_calls) = (ranks(&index_ranks), ranks(&full_ranks));
    assert_eq!((index_calls.len(), full_calls.len()), (2, 1));
    assert!(
        index_calls[1]["edges"].as_u64().unwrap() > 0,
        "{}",
        index_calls[1]
    );
    let rows = |call: &Value| call["rows"].as_u64().unwrap();
    let fused = graphicle_memory::retrieval::MAX_FUSED as u64;
    let (first, second, whole) = (
        rows(&index_calls[0]),
        rows(&index_calls[1]),
        rows(&full_calls[0]),
    );
    assert!(whole > first + fused + 100, "{first} {whole}");
    assert!(second <= first + fused, "{first} {second}");
}

/// Editing one file in sync after sync adds a small segment each time and
/// drops the one the file left empty: the index is merged only when the
/// files that changed hold more than `MAX_DEAD` of all its passages, never
/// for one small segment.
#[test]
fn editing_one_file_again_and_again_never_rewrites_the_index() {
    let files: Vec<(String, String)> = (0..10)
        .map(|i| {
            (
                format!("src/m{i}.py"),
                format!("def f{i}():\n    return {i}\n"),
            )
        })
        .collect();
    let h = Home::new(&files);
    let first = h.json(&["sync", "."], &[]);
    assert_eq!(first["passage_index"]["segments"], 1, "{first:#}");
    for (edits, word) in ["walrus", "narwhal", "axolotl"].iter().enumerate() {
        std::fs::write(
            h.app.join("src/m0.py"),
            format!("def f0():\n    return '{word}'\n"),
        )
        .unwrap();
        let synced = h.json(&["sync", "."], &[]);
        let index = &synced["passage_index"];
        assert_eq!(index["files"], 1, "{synced:#}");
        assert_eq!(index["merged"], false, "{synced:#}");
        // The segment the previous edit wrote holds no current file now.
        assert_eq!(index["emptied"], edits.min(1), "{synced:#}");
    }
    let arguments = h.home.join("edited.json");
    std::fs::write(
        &arguments,
        json!({"question":"Which function returns axolotl?","debug_candidates":20}).to_string(),
    )
    .unwrap();
    let packet = h.json(
        &[
            "call",
            "context",
            "--arguments",
            arguments.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(packet["passage_index"]["status"], "ready", "{packet:#}");
    let candidates = listed(&packet, "candidates_debug", "identity");
    assert!(
        candidates
            .first()
            .is_some_and(|id| id.starts_with("file:src/m0.py:")),
        "{candidates:?}"
    );
}
