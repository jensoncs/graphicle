use graphicle_memory::text::sha256;
use graphicle_runtime::{
    Error,
    store::{Store, s},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    workspace: String,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("pilot protocol").unwrap();
        Self {
            _dir: dir,
            store,
            workspace: s(&ws, "id").into(),
            token: s(&ws, "capability").into(),
        }
    }
    fn repo(&self, key: &str) -> String {
        s(
            &self
                .store
                .register(
                    &self.workspace,
                    &self.token,
                    &json!({"project_key":key,"name":key}),
                )
                .unwrap(),
            "id",
        )
        .into()
    }
    fn prepare(&self, repo: &str, extra: Value) -> Value {
        let current = self
            .store
            .project_status(
                &self.workspace,
                &self.token,
                &json!({"repository_id":repo,"compact":true}),
            )
            .unwrap();
        let mut args = extra;
        args["repository_id"] = json!(repo);
        args["expected_snapshot"] = current["repository"]["snapshot"].clone();
        self.store
            .sync_prepare(&self.workspace, &self.token, &args)
            .unwrap()
    }
    fn upload(&self, repo: &str, sync: &Value, files: Value) {
        self.store
            .sync_upload(
                &self.workspace,
                &self.token,
                &json!({"repository_id":repo,"sync_id":sync["sync_id"],"files":files}),
            )
            .unwrap();
    }
    fn commit(&self, repo: &str, sync: &Value) -> Value {
        self.store
            .sync_commit(
                &self.workspace,
                &self.token,
                &json!({"repository_id":repo,"sync_id":sync["sync_id"],"compact":true}),
            )
            .unwrap()
    }
}
fn item(path: &str, content: &[u8]) -> Value {
    json!({"path":path,"sha256":sha256(content),"bytes":content.len()})
}

#[test]
fn large_missing_pages_are_byte_bounded_and_survive_uploads() {
    let f = Fixture::new();
    let repo = f.repo("large");
    let manifest: Vec<_> = (0..3000)
        .map(|i| {
            item(
                &format!("{}/{i:05}.txt", "q\"".repeat(350)),
                format!("content {i}").as_bytes(),
            )
        })
        .collect();
    let old_size = serde_json::to_vec(
        &manifest
            .iter()
            .map(|i| i["path"].clone())
            .collect::<Vec<_>>(),
    )
    .unwrap()
    .len();
    assert!(old_size > 1024 * 1024);
    let first = f.prepare(&repo, json!({"manifest":manifest,"missing_limit":1024}));
    let mut page = first.clone();
    let mut names = BTreeSet::new();
    let (mut pages, mut largest) = (0, 0);
    loop {
        let encoded = serde_json::to_vec(&page).unwrap().len();
        largest = largest.max(encoded);
        assert!(encoded < 160 * 1024);
        pages += 1;
        for path in page["missing"].as_array().unwrap() {
            assert!(names.insert(path.as_str().unwrap().to_string()));
        }
        if pages == 1 {
            let files: Vec<_> = page["missing"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    let path = p.as_str().unwrap();
                    let i: usize = path
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .trim_end_matches(".txt")
                        .parse()
                        .unwrap();
                    json!({"path":path,"content":format!("content {i}")})
                })
                .collect();
            f.upload(&repo, &first, json!(files));
        }
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        page=f.store.sync_missing(&f.workspace,&f.token,&json!({"repository_id":repo,"sync_id":first["sync_id"],"cursor":cursor,"missing_limit":1024})).unwrap();
    }
    assert_eq!(names.len(), 3000);
    assert!(pages > 1);
    let other = f.repo("other");
    let next = f.prepare(&other, json!({"manifest":[item("x",b"x")]}));
    assert!(f.store.sync_missing(&f.workspace,&f.token,&json!({"repository_id":other,"sync_id":next["sync_id"],"cursor":first["next_cursor"]})).is_err());
    println!(
        "PILOT_MISSING_MEASUREMENT={}",
        json!({"paths":3000,"unpaged_bytes":old_size,"largest_page_bytes":largest,"pages":pages})
    );
}

#[test]
fn compact_status_and_commit_do_not_emit_large_manifests() {
    let f = Fixture::new();
    let repo = f.repo("status");
    let body =
        "import { value as other } from './0.js';\nexport function value() { return other(); }";
    let manifest: Vec<_> = (0..1400)
        .map(|i| {
            item(
                &format!("{}{i}.js", format!("{}/", "a".repeat(170)).repeat(5)),
                body.as_bytes(),
            )
        })
        .collect();
    let sync = f.prepare(&repo, json!({"manifest":manifest}));
    assert_eq!(sync["missing_total"], 1); // Deduplicates identical content within a side.
    f.upload(
        &repo,
        &sync,
        json!([{"path":sync["missing"][0],"content":body}]),
    );
    let committed = f.commit(&repo, &sync);
    assert!(committed.to_string().len() < 2048);
    assert_eq!(committed["changed_count"], 1400);
    assert!(committed.get("changed").is_none());
    graphicle_runtime::jobs::drain_projection(&f.store, &repo).unwrap();
    let full = f
        .store
        .project_status(&f.workspace, &f.token, &json!({"repository_id":repo}))
        .unwrap();
    let compact = f
        .store
        .project_status(
            &f.workspace,
            &f.token,
            &json!({"repository_id":repo,"compact":true}),
        )
        .unwrap();
    assert_eq!(
        compact["repository"]["snapshot"],
        full["repository"]["snapshot"]
    );
    assert_eq!(compact["files"], 1400);
    assert!(compact.get("source_state").is_none());
    println!(
        "PILOT_STATUS_MEASUREMENT={}",
        json!({"files":1400,"full_bytes":full.to_string().len(),"compact_bytes":compact.to_string().len()})
    );
    assert!(full.to_string().len() > 1024 * 1024);
    assert!(compact.to_string().len() < 2048);
}

#[test]
fn snapshot_reuse_preserves_bytes_and_rejects_cross_workspace_access() {
    let f = Fixture::new();
    let donor = f.repo("context");
    let target = f.repo("scratch");
    let first = f.prepare(
        &donor,
        json!({"manifest":[item("a.txt",b"hello"),item("raw.bin",&[0,255])]}),
    );
    f.upload(
        &donor,
        &first,
        json!([{"path":"a.txt","content":"hello"},{"path":"raw.bin","content_base64":"AP8="}]),
    );
    f.commit(&donor, &first);
    let copy=f.prepare(&target,json!({"manifest":[item("renamed.txt",b"hello"),item("raw.bin",&[0,255])],"reuse_sources":[{"repository_id":donor,"snapshot":first["snapshot"]}]}));
    assert_eq!(copy["missing_total"], 0);
    assert_eq!(copy["reused"], json!({"objects":2,"bytes":7}));
    f.commit(&target, &copy);
    let head=f.prepare(&target,json!({"manifest":[item("renamed.txt",b"hello"),item("raw.bin",&[0,255]),item("new.txt",b"new")]}));
    assert_eq!(head["missing"], json!(["new.txt"]));
    let foreign = f.store.create_workspace("foreign").unwrap();
    let foreign_repo = f
        .store
        .register(
            s(&foreign, "id"),
            s(&foreign, "capability"),
            &json!({"project_key":"scratch","name":"scratch"}),
        )
        .unwrap();
    assert!(f.store.sync_prepare(s(&foreign,"id"),s(&foreign,"capability"),&json!({"repository_id":foreign_repo["id"],"expected_snapshot":"","manifest":[item("stolen.txt",b"hello")],"reuse_sources":[{"repository_id":donor,"snapshot":first["snapshot"]}]})).is_err());
    // Objects uploaded to a donor but absent from the named snapshot are not reusable.
    let pending = f.prepare(
        &donor,
        json!({"manifest":[item("private.txt",b"uncommitted")]}),
    );
    f.upload(
        &donor,
        &pending,
        json!([{"path":"private.txt","content":"uncommitted"}]),
    );
    let fresh = f.repo("fresh");
    let refused=f.prepare(&fresh,json!({"manifest":[item("private.txt",b"uncommitted")],"reuse_sources":[{"repository_id":donor,"snapshot":first["snapshot"]}]}));
    assert_eq!(refused["missing_total"], 1);
    // A later donor failure rolls back bytes copied from an earlier donor.
    let rollback = f.repo("rollback");
    let collector = f
        .store
        .grant(
            &f.workspace,
            &f.token,
            &json!({"label":"restricted collector","role":"collector"}),
        )
        .unwrap();
    f.store
        .authorized(&f.workspace, &f.token, "admin", |db, _| {
            graphicle_runtime::graph_projection::set_repo_access(
                db,
                &f.workspace,
                &donor,
                s(&collector, "id"),
                "revoked",
            )
        })
        .unwrap();
    let denied = f
        .store
        .sync_prepare(
            &f.workspace,
            s(&collector, "capability"),
            &json!({
                "repository_id":rollback,"expected_snapshot":"","manifest":[item("a.txt",b"hello")],
                "reuse_sources":[{"repository_id":donor,"snapshot":first["snapshot"]}]
            }),
        )
        .unwrap_err();
    assert_eq!(denied.code(), "admission_scope");
    assert!(f.store.sync_prepare(&f.workspace,&f.token,&json!({
        "repository_id":rollback,"expected_snapshot":"","manifest":[item("a.txt",b"hello")],
        "reuse_sources":[{"repository_id":donor,"snapshot":first["snapshot"]},
                         {"repository_id":donor,"snapshot":"0".repeat(64)}]
    })).is_err());
    let after = f.prepare(&rollback, json!({"manifest":[item("a.txt",b"hello")]}));
    assert_eq!(after["missing_total"], 1);
    println!(
        "PILOT_REUSE_MEASUREMENT={}",
        json!({"source_objects":2,"source_bytes":7,"uploaded_bytes":0,"reused_bytes":copy["reused"]["bytes"]})
    );
}

#[test]
fn review_sides_upload_only_changed_objects_after_snapshot_reuse() {
    let f = Fixture::new();
    let donor = f.repo("large-context");
    let target = f.repo("large-scratch");
    let base: Vec<_> = (0..1000)
        .map(|i| {
            (
                format!("doc{i:04}.txt"),
                format!("Document {i}\n{}", "source content ".repeat(80)),
            )
        })
        .collect();
    let manifest = |files: &Vec<(String, String)>| {
        files
            .iter()
            .map(|(p, t)| item(p, t.as_bytes()))
            .collect::<Vec<_>>()
    };
    let wire = |files: &[(String, String)]| {
        files
            .iter()
            .map(|(p, t)| json!({"path":p,"content":t}))
            .collect::<Vec<_>>()
    };
    let admitted = f.prepare(&donor, json!({"manifest":manifest(&base)}));
    for batch in base.chunks(100) {
        f.upload(&donor, &admitted, json!(wire(batch)));
    }
    f.commit(&donor, &admitted);
    let reused = f.prepare(&target,json!({"manifest":manifest(&base),"reuse_sources":[{"repository_id":donor,"snapshot":admitted["snapshot"]}]}));
    assert_eq!(reused["missing_total"], 0);
    assert_eq!(reused["reused"]["objects"], 1000);
    f.commit(&target, &reused);
    let mut head = base.clone();
    for (_, body) in head.iter_mut().take(8) {
        body.push_str("Changed.\n");
    }
    let pending = f.prepare(&target, json!({"manifest":manifest(&head)}));
    assert_eq!(pending["missing"].as_array().unwrap().len(), 8);
    f.upload(&target, &pending, json!(wire(&head[..8])));
    f.commit(&target, &pending);
    let raw_full: usize = base.iter().chain(&head).map(|(_, t)| t.len()).sum();
    let raw_uploaded: usize = head[..8].iter().map(|(_, t)| t.len()).sum();
    let base64_full: usize = base
        .iter()
        .chain(&head)
        .map(|(_, t)| 4 * t.len().div_ceil(3))
        .sum();
    let base64_uploaded: usize = head[..8].iter().map(|(_, t)| 4 * t.len().div_ceil(3)).sum();
    assert!(raw_uploaded * 100 < raw_full);
    println!(
        "PILOT_ADMISSION_MEASUREMENT={}",
        json!({"files_per_side":1000,"changed_files":8,"raw_full_upload_bytes":raw_full,"raw_uploaded_bytes":raw_uploaded,"base64_full_upload_bytes":base64_full,"base64_uploaded_bytes":base64_uploaded,"reused_objects":reused["reused"]["objects"]})
    );
}

#[test]
fn structured_errors_are_short_and_classified_without_message_parsing() {
    for (error, code) in [
        (Error::transport_bound("arbitrary"), "transport_bound"),
        (Error::denied(), "admission_scope"),
        (Error::timeout("arbitrary"), "timeout"),
        (Error::conflict("arbitrary"), "conflict"),
    ] {
        assert_eq!(error.json()["code"], code);
        assert_eq!(error.json()["format"], "graphicle.error.v1");
    }
    assert!(Error::bad("x".repeat(10000)).json().to_string().len() < 2300);
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_graphicle"))
        .args(["--json-errors", "not-a-command"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["code"], "invalid_request");
}

#[tokio::test]
async fn http_extractor_and_body_limit_failures_use_structured_errors() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use graphicle_runtime::{
        api::{App, router},
        model,
    };
    use tower::ServiceExt;
    let f = Fixture::new();
    let app = router(
        App::new(
            f.store.clone(),
            model::Config::default(),
            vec!["localhost".into()],
        )
        .unwrap(),
    );
    for (body, status, code) in [
        ("{".to_string(), 400, "invalid_request"),
        ("x".repeat(25 * 1024 * 1024), 413, "transport_bound"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/workspaces/{}/repositories/command",
                        f.workspace
                    ))
                    .header("host", "localhost")
                    .header("authorization", format!("Bearer {}", f.token))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status);
        let body = to_bytes(response.into_body(), 16000).await.unwrap();
        let error: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(error["code"], code);
        assert_eq!(error["format"], "graphicle.error.v1");
    }
}

#[test]
fn context_exposes_real_scores_and_budget_losses() {
    let f = Fixture::new();
    let repo = f.repo("ranked");
    let files: Vec<_> = (0..12)
        .map(|i| {
            (
                format!("doc{i}.txt"),
                format!(
                    "Nebula document {i}.\n{}",
                    "Nebula routing retries improve delivery reliability. ".repeat(180)
                ),
            )
        })
        .collect();
    let sync = f.prepare(
        &repo,
        json!({"manifest":files.iter().map(|(p,t)|item(p,t.as_bytes())).collect::<Vec<_>>()}),
    );
    f.upload(
        &repo,
        &sync,
        json!(
            files
                .iter()
                .map(|(p, t)| json!({"path":p,"content":t}))
                .collect::<Vec<_>>()
        ),
    );
    f.commit(&repo, &sync);
    let packet=f.store.project_context(&f.workspace,&f.token,&json!({"repository_id":repo,"question":"Nebula routing retries reliability","max_tokens":512,"max_chars":2000,"limit":12,"memory_retrieval":false})).unwrap();
    let sources = packet["sources"].as_array().unwrap();
    assert!(!sources.is_empty());
    assert!(packet["packing"]["dropped"].as_u64().unwrap() > 0);
    assert_eq!(
        packet["packing"]["candidates"].as_u64().unwrap(),
        sources.len() as u64 + packet["packing"]["dropped"].as_u64().unwrap()
    );
    let scores: Vec<_> = sources
        .iter()
        .map(|v| {
            assert!(v["rank"].as_u64().unwrap() > 0);
            v["rank_score"].as_f64().expect("actual fusion score")
        })
        .collect();
    assert_eq!(
        packet["packing"]["lowest_score_kept"].as_f64(),
        scores.into_iter().min_by(f64::total_cmp)
    );
    assert!(
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["rank_score"].as_f64().is_some())
    );
    println!("PILOT_RANKING_MEASUREMENT={}", packet["packing"]);
}
