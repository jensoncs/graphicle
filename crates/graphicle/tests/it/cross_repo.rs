//! End-to-end cross-repository import linkage: two
//! independently indexed repositories in one workspace, linked by package
//! identity, and hidden again when the principal loses read access to a
//! repository. How the PR change-impact report surfaces these links is tested
//! with the edition that serves that report.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use graphicle_runtime::{
    api::{App, router},
    jobs, model,
    store::{Store, s},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

const A_PYPROJECT: &str = "[project]\nname = \"alib\"\nversion = \"1.2.0\"\n";
const A_CORE_V1: &str =
    "def process(items):\n    return [i for i in items]\n\ndef helper():\n    return 1\n";
const A_CORE_V2: &str = "def process(items):\n    total = 0\n    for i in items:\n        total += i\n    return total\n";
const B_PYPROJECT: &str =
    "[project]\nname = \"bapp\"\nversion = \"0.1.0\"\ndependencies = [\"alib>=1.0\"]\n";
const B_INGEST: &str = "from alib.core import process, helper\n\ndef ingest(rows):\n    helper()\n    return process(rows)\n";
const A_SHA1: &str = "aaa1110000000000000000000000000000000000";
const A_SHA2: &str = "aaa2220000000000000000000000000000000000";
const B_SHA1: &str = "bbb1110000000000000000000000000000000000";

fn a_files(core: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("pyproject.toml", A_PYPROJECT),
        ("alib/__init__.py", ""),
        ("alib/core.py", core),
    ]
}
fn b_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("pyproject.toml", B_PYPROJECT),
        ("bapp/__init__.py", ""),
        ("bapp/ingest.py", B_INGEST),
    ]
}

struct Fixture {
    _dir: TempDir,
    store: Store,
    workspace: String,
    key: String,
    a: String,
    b: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("Cross repo").unwrap();
        let workspace = s(&ws, "id").to_string();
        let key = s(&ws, "capability").to_string();
        let register = |project_key: &str, name: &str| {
            s(
                &store
                    .register(
                        &workspace,
                        &key,
                        &json!({"project_key":project_key,"name":name}),
                    )
                    .unwrap(),
                "id",
            )
            .to_string()
        };
        let a = register("alib", "Library A");
        let b = register("bapp", "Application B");
        Self {
            _dir: dir,
            store,
            workspace,
            key,
            a,
            b,
        }
    }
    fn commit(&self, repo: &str, files: &[(&str, &str)], sha: &str) {
        let args = |mut v: Value| {
            v["repository_id"] = json!(repo);
            v
        };
        let state = self
            .store
            .project_status(&self.workspace, &self.key, &args(json!({})))
            .unwrap();
        let begin = self
            .store
            .sync_prepare(
                &self.workspace,
                &self.key,
                &args(json!({
                    "expected_snapshot": state["repository"]["snapshot"],
                    "metadata": {"commit": sha},
                    "manifest": files.iter().map(|(name, text)| json!({
                        "path": name,
                        "sha256": graphicle_memory::text::sha256(text.as_bytes()),
                        "bytes": text.len(),
                    })).collect::<Vec<_>>(),
                })),
            )
            .unwrap();
        self.store
            .sync_upload(
                &self.workspace,
                &self.key,
                &args(json!({
                    "sync_id": begin["sync_id"],
                    "files": files.iter().map(|(name, text)| json!({"path": name, "content": text})).collect::<Vec<_>>(),
                })),
            )
            .unwrap();
        self.store
            .sync_commit(
                &self.workspace,
                &self.key,
                &args(json!({"sync_id": begin["sync_id"]})),
            )
            .unwrap();
    }
    /// Run every queued job: projections first (each one queues a workspace
    /// `link` job), then the link jobs they left behind. Returns
    /// `(kind, result)` in execution order.
    fn drain_jobs(&self) -> Vec<(String, Value)> {
        let mut results = Vec::new();
        while let Some(job) = self.store.lease_job().unwrap() {
            let result = match job.kind.as_str() {
                "project" => jobs::run_projection(&self.store, &job),
                "link" => jobs::run_link(&self.store, &job),
                other => panic!("unexpected job kind {other}"),
            };
            let value = result.as_ref().ok().cloned().unwrap_or(Value::Null);
            self.store.finish_job(&job, result).unwrap();
            results.push((job.kind.clone(), value));
        }
        results
    }
    fn episode(&self, repo: &str) -> String {
        let listing = self
            .store
            .graph_episodes(&self.workspace, &self.key, repo)
            .unwrap();
        s(&listing, "current_episode").to_string()
    }
    fn app(&self) -> Router {
        router(
            App::new(
                self.store.clone(),
                model::Config::default(),
                vec!["localhost".into()],
            )
            .unwrap(),
        )
    }
}

async fn call(
    app: Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("host", "localhost");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[test]
fn linking_persists_edges_with_both_episodes_and_relinks_after_a_new_episode() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    let results = f.drain_jobs();
    // Two projections, then exactly one `link` job: the second projection
    // found the first one's link job still queued and replayed it.
    let kinds: Vec<&str> = results.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, vec!["project", "project", "link"], "{kinds:?}");
    assert_eq!(results[0].1["cross_repo_link"]["replayed"], false);
    assert_eq!(results[1].1["cross_repo_link"]["replayed"], true);
    assert_eq!(
        results[0].1["cross_repo_link"]["id"],
        results[1].1["cross_repo_link"]["id"]
    );
    // The link job saw both repositories and linked them.
    let link = &results[2].1;
    assert!(link["edges"].as_u64().unwrap() >= 2, "{link}");
    assert_eq!(link["repositories"], 2);

    let a1 = f.episode(&f.a);
    let b1 = f.episode(&f.b);
    let edges = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.a)
        .unwrap();
    let incoming = edges["incoming"].as_array().unwrap();
    let targets: Vec<&str> = incoming.iter().map(|e| s(e, "target_node_id")).collect();
    assert!(targets.contains(&"alib_core_process"), "{targets:?}");
    assert!(targets.contains(&"alib_core_helper"), "{targets:?}");
    for edge in incoming {
        assert_eq!(s(edge, "source_repository_id"), f.b);
        assert_eq!(s(edge, "source_episode"), b1);
        assert_eq!(s(edge, "target_episode"), a1);
        assert_eq!(s(edge, "source_node_id"), "bapp_ingest");
        assert_eq!(s(edge, "relation"), "imports");
        assert_eq!(s(edge, "confidence"), "EXTRACTED");
        assert_eq!(s(edge, "version_constraint"), ">=1.0");
        assert_eq!(s(edge, "exporter_version"), "1.2.0");
        assert_eq!(s(edge, "version_status"), "satisfied");
    }
    assert!(edges["unresolved"].as_array().unwrap().is_empty());

    // Re-running on unchanged episodes is idempotent.
    let again = f.store.graph_link(&f.workspace, &f.key).unwrap();
    assert!(
        again["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["unchanged"] == true),
        "{again}"
    );
    let count = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.a)
        .unwrap()["incoming"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(count, incoming.len());

    // A new episode of A: `helper` disappears, `process` changes.
    f.commit(&f.a, &a_files(A_CORE_V2), A_SHA2);
    f.drain_jobs();
    let a2 = f.episode(&f.a);
    assert_ne!(a1, a2);
    let edges = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.a)
        .unwrap();
    let live: Vec<&Value> = edges["incoming"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["valid_to_episode"].is_null())
        .collect();
    // The surviving symbol edge plus the module-level fallback edge that the
    // missing `helper` import leaves behind (INFERRED, to the file node).
    assert_eq!(live.len(), 2, "{edges}");
    let symbol_edges: Vec<&&Value> = live
        .iter()
        .filter(|e| s(e, "relation") == "imports")
        .collect();
    assert_eq!(symbol_edges.len(), 1);
    assert_eq!(s(symbol_edges[0], "target_node_id"), "alib_core_process");
    for edge in &live {
        assert_eq!(s(edge, "target_episode"), a2);
        assert_eq!(s(edge, "source_episode"), b1);
    }
    let module_edge = live
        .iter()
        .find(|e| s(e, "relation") == "imports_from")
        .unwrap();
    assert_eq!(s(module_edge, "target_node_id"), "alib_core");
    assert_eq!(s(module_edge, "confidence"), "INFERRED");
    // The import of the vanished symbol is now an unresolved cross import naming A.
    let unresolved = edges["unresolved"].as_array().unwrap();
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert_eq!(s(&unresolved[0], "reason"), "symbol_not_exported");
    assert_eq!(s(&unresolved[0], "symbol"), "helper");
    assert!(s(&unresolved[0], "candidates").contains(&f.a));
}

#[test]
fn repository_revocation_blocks_direct_reads_and_outgoing_links() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    f.drain_jobs();
    let reader = f
        .store
        .grant(
            &f.workspace,
            &f.key,
            &json!({"role":"reader","label":"restricted"}),
        )
        .unwrap();
    let token = s(&reader, "capability");
    f.store
        .graph_access(
            &f.workspace,
            &f.key,
            &json!({"repository_id": f.a, "credential_id": reader["id"], "state":"revoked"}),
        )
        .unwrap();

    assert!(
        f.store.graph_episodes(&f.workspace, token, &f.a).is_err(),
        "revoked repository remains directly readable"
    );
    assert!(
        f.store
            .project_read(&f.workspace, token, "files", &json!({"repository_id":f.a}))
            .is_err()
    );
    let visible = f.store.projects(&f.workspace, token).unwrap();
    assert!(visible.as_array().unwrap().iter().all(|r| r["id"] != f.a));
    let edges = f
        .store
        .graph_cross_edges(&f.workspace, token, &f.b)
        .unwrap();
    assert_eq!(
        edges["outgoing"],
        json!([]),
        "hidden exporter leaked: {edges}"
    );
    let original = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.b)
        .unwrap();
    let restricted = f.store.graph_link(&f.workspace, token).unwrap();
    assert_eq!(restricted["persisted"], false);
    assert_eq!(
        f.store
            .graph_cross_edges(&f.workspace, &f.key, &f.b)
            .unwrap(),
        original,
        "a restricted link pass overwrote shared graph evidence"
    );
    assert!(f.store.graph_episodes(&f.workspace, &f.key, &f.a).is_ok());
}

#[tokio::test]
async fn repository_revocation_is_enforced_over_http_and_mcp() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.drain_jobs();
    let reader = f
        .store
        .grant(
            &f.workspace,
            &f.key,
            &json!({"role":"reader","label":"restricted"}),
        )
        .unwrap();
    let token = s(&reader, "capability");
    f.store
        .graph_access(
            &f.workspace,
            &f.key,
            &json!({"repository_id": f.a, "credential_id":reader["id"], "state":"revoked"}),
        )
        .unwrap();
    let (status, _) = call(
        f.app(),
        "GET",
        &format!(
            "/api/workspaces/{}/repositories/{}/graph/stats",
            f.workspace, f.a
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(
        f.app(),
        "POST",
        &format!("/mcp/{}/{}", f.workspace, f.a),
        token,
        Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"graph_episodes","arguments":{}}})),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN
            || body["result"]["isError"] == true
            || body["error"].is_object(),
        "MCP did not enforce the revoked repository: {status} {body}"
    );
}

#[tokio::test]
async fn http_route_and_mcp_tool_expose_linking() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    f.drain_jobs();
    let (status, body) = call(
        f.app(),
        "POST",
        &format!("/api/workspaces/{}/graph/link", f.workspace),
        &f.key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["edges"].as_u64().unwrap() >= 2, "{body}");
    assert_eq!(body["repositories"].as_array().unwrap().len(), 2);

    let (status, body) = call(
        f.app(),
        "GET",
        &format!(
            "/api/workspaces/{}/repositories/{}/graph/cross_edges",
            f.workspace, f.a
        ),
        &f.key,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["incoming"].as_array().unwrap().is_empty());

    let (status, body) = call(
        f.app(),
        "POST",
        &format!("/mcp/{}/{}", f.workspace, f.a),
        &f.key,
        Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"graph_link"), "{names:?}");
    // Linking persists edges, so the tool is not read-only.
    let link_tool = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "graph_link")
        .unwrap();
    assert_eq!(
        link_tool["annotations"]["readOnlyHint"], false,
        "{link_tool}"
    );
    let (status, body) = call(
        f.app(),
        "POST",
        &format!("/mcp/{}/{}", f.workspace, f.a),
        &f.key,
        Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"graph_link","arguments":{}}})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["result"]["isError"], false, "{body}");
    assert!(
        body["result"]["structuredContent"]["edges"]
            .as_u64()
            .unwrap()
            >= 2
    );
}

#[test]
fn linking_a_single_repository_workspace_is_skipped_and_records_nothing() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    let results = f.drain_jobs();
    let kinds: Vec<&str> = results.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, vec!["project", "link"], "{kinds:?}");
    let link = &results[1].1;
    assert_eq!(link["skipped"], "fewer_than_two_repositories", "{link}");
    assert_eq!(link["pairs"], 0);
    assert_eq!(link["edges"], 0);
    assert_eq!(link["repositories"], 1);
    let direct = f.store.graph_link(&f.workspace, &f.key).unwrap();
    assert_eq!(direct["skipped"], "fewer_than_two_repositories", "{direct}");
    assert_eq!(direct["pairs"], json!([]));
    assert_eq!(direct["repositories"].as_array().unwrap().len(), 1);
    let db = f.store.connect().unwrap();
    for table in ["gf_cross_edges", "gf_cross_packages", "gf_cross_unresolved"] {
        let count: i64 = db
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE workspace_id=?"),
                [&f.workspace],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table} must stay empty for one repository");
    }
    // Once a second repository is projected the pair links as before.
    f.commit(&f.b, &b_files(), B_SHA1);
    f.drain_jobs();
    let edges = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.a)
        .unwrap();
    assert!(edges["incoming"].as_array().unwrap().len() >= 2, "{edges}");
}

/// A `pr_scratch` copy of the library (the project an
/// earlier `graphicle pr` registered for the same checkout) no longer makes
/// the application's imports ambiguous. It is skipped while the synced
/// library exports the same package, listed with its role, and old stores
/// mark such `git:` projects when they open. Without the role the same
/// workspace leaves the imports unresolved as `ambiguous_exporters`.
#[test]
fn a_pr_scratch_copy_does_not_make_imports_ambiguous() {
    let f = Fixture::new();
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    // The project key `graphicle pr` used: `git:` and a sha256.
    let copy = s(
        &f.store
            .register(
                &f.workspace,
                &f.key,
                &json!({"project_key":format!("git:{}", "ab".repeat(32)),"name":"alib (pr)"}),
            )
            .unwrap(),
        "id",
    )
    .to_string();
    f.commit(&copy, &a_files(A_CORE_V1), A_SHA1);
    f.drain_jobs();

    // Unmarked (a store written before PR scratch marks): the copy is a second
    // exporter of `alib`, so B's imports of it are ambiguous.
    let db = f.store.connect().unwrap();
    db.execute("DELETE FROM repo_project_roles", []).unwrap();
    let ambiguous = f.store.graph_link(&f.workspace, &f.key).unwrap();
    assert_eq!(ambiguous["edges"], 0, "{ambiguous}");
    let reasons: Vec<String> = db
        .prepare("SELECT reason FROM gf_cross_unresolved WHERE source_repository_id=? AND valid_to_episode IS NULL")
        .unwrap()
        .query_map([&f.b], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        reasons.iter().any(|r| r == "ambiguous_exporters"),
        "{reasons:?}"
    );

    // Reopening the store marks the legacy `git:` project as `pr_scratch`.
    let reopened = Store::open(f._dir.path()).unwrap();
    let projects = reopened.projects(&f.workspace, &f.key).unwrap();
    let role = |id: &str| {
        projects
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p.get("role").cloned())
    };
    assert_eq!(role(&copy), Some(Some(json!("pr_scratch"))), "{projects}");
    assert_eq!(role(&f.a), Some(None), "{projects}");

    let linked = reopened.graph_link(&f.workspace, &f.key).unwrap();
    assert_eq!(linked["pr_scratch_skipped"], json!([copy]), "{linked}");
    assert!(linked["edges"].as_u64().unwrap() >= 2, "{linked}");
    let targets: Vec<String> = db
        .prepare("SELECT DISTINCT target_repository_id FROM gf_cross_edges WHERE source_repository_id=? AND valid_to_episode IS NULL")
        .unwrap()
        .query_map([&f.b], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(targets, vec![f.a.clone()], "{linked}");
    let live_ambiguous: i64 = db
        .query_row(
            "SELECT count(*) FROM gf_cross_unresolved WHERE source_repository_id=? AND valid_to_episode IS NULL AND reason='ambiguous_exporters'",
            [&f.b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live_ambiguous, 0);
    let copy_live: i64 = db
        .query_row(
            "SELECT count(*) FROM gf_cross_edges WHERE (source_repository_id=?1 OR target_repository_id=?1) AND valid_to_episode IS NULL",
            [&copy],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(copy_live, 0);
    // Nothing was deleted: the copy and its graph stay reachable.
    assert!(reopened.graph_episodes(&f.workspace, &f.key, &copy).is_ok());

    // A scratch project alone (its folder never synced) still links: B's
    // imports resolve into it while no other project exports `alib`.
    let alone = Fixture::new();
    alone.commit(&alone.b, &b_files(), B_SHA1);
    alone.commit(&alone.a, &a_files(A_CORE_V1), A_SHA1);
    alone
        .store
        .mark_pr_scratch(&alone.workspace, &alone.key, &alone.a)
        .unwrap();
    alone.drain_jobs();
    let linked = alone
        .store
        .graph_link(&alone.workspace, &alone.key)
        .unwrap();
    assert!(linked.get("pr_scratch_skipped").is_none(), "{linked}");
    assert!(linked["edges"].as_u64().unwrap() >= 2, "{linked}");
}

const C_PYPROJECT: &str =
    "[project]\nname = \"capp\"\nversion = \"0.1.0\"\ndependencies = [\"alib>=1.0\"]\n";
const C_MAIN_V1: &str =
    "from alib.core import process\n\ndef main(rows):\n    return process(rows)\n";
const C_MAIN_V2: &str = "from alib.core import process\n\ndef main(rows):\n    rows = list(rows)\n    return process(rows)\n";

fn c_files(main: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("pyproject.toml", C_PYPROJECT),
        ("capp/__init__.py", ""),
        ("capp/main.py", main),
    ]
}

/// The link tables and the state the incremental pass keeps.
const LINK_TABLES: &[&str] = &[
    "gf_cross_edges",
    "gf_cross_unresolved",
    "gf_cross_packages",
    "gf_cross_link_state",
];

/// Every row of the cross-repository link tables, live and closed, without
/// row ids and timestamps, sorted.
fn link_rows(db: &rusqlite::Connection) -> Vec<String> {
    let mut out = Vec::new();
    for table in ["gf_cross_edges", "gf_cross_unresolved", "gf_cross_packages"] {
        let mut statement = db.prepare(&format!("SELECT * FROM {table}")).unwrap();
        let columns: Vec<String> = statement.column_names().map(|c| c.to_string()).collect();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let mut fields = vec![table.to_string()];
            for (i, column) in columns.iter().enumerate() {
                if column == "id" || column == "created_at" {
                    continue;
                }
                let value: rusqlite::types::Value = row.get(i).unwrap();
                fields.push(format!("{column}={value:?}"));
            }
            out.push(fields.join("|"));
        }
    }
    out.sort();
    out
}

impl Fixture {
    fn register(&self, project_key: &str, name: &str) -> String {
        s(
            &self
                .store
                .register(
                    &self.workspace,
                    &self.key,
                    &json!({"project_key":project_key,"name":name}),
                )
                .unwrap(),
            "id",
        )
        .to_string()
    }
    /// Run the queued jobs as `drain_jobs` does, checking each `link` job
    /// against a full pass: from the same rows, a full pass runs first and
    /// its rows are kept; the rows are put back, the incremental `link` job
    /// runs, and both must leave the same rows. Returns the `link` jobs'
    /// results.
    fn drain_checking_links(&self) -> Vec<Value> {
        let mut links = Vec::new();
        while let Some(job) = self.store.lease_job().unwrap() {
            let result = match job.kind.as_str() {
                "project" => jobs::run_projection(&self.store, &job),
                "link" => {
                    let db = self.store.connect().unwrap();
                    for table in LINK_TABLES {
                        db.execute_batch(&format!(
                            "DROP TABLE IF EXISTS snap_{table}; CREATE TABLE snap_{table} AS SELECT * FROM {table};"
                        ))
                        .unwrap();
                    }
                    let full = self
                        .store
                        .graph_link_full(&self.workspace, &self.key)
                        .unwrap();
                    // A full pass reuses nothing.
                    assert!(
                        full["reused"].as_array().is_none_or(Vec::is_empty),
                        "{full}"
                    );
                    let after_full = link_rows(&db);
                    for table in LINK_TABLES {
                        db.execute_batch(&format!(
                            "DELETE FROM {table}; INSERT INTO {table} SELECT * FROM snap_{table}; DROP TABLE snap_{table};"
                        ))
                        .unwrap();
                    }
                    let result = jobs::run_link(&self.store, &job);
                    assert_eq!(
                        link_rows(&db),
                        after_full,
                        "the incremental pass must leave the rows of a full pass"
                    );
                    links.push(result.as_ref().unwrap().clone());
                    result
                }
                other => panic!("unexpected job kind {other}"),
            };
            self.store.finish_job(&job, result).unwrap();
        }
        links
    }
}

/// Each `link` job relinks only the projects whose episode, or whose
/// partners' export surface, changed; the others' links are reused, and
/// every pass leaves the rows a full pass leaves (checked at every step).
#[test]
fn incremental_linking_relinks_only_changed_projects_and_matches_a_full_pass() {
    let f = Fixture::new();
    let c = f.register("capp", "Application C");
    let ids = |list: &Value| -> Vec<String> {
        let mut ids: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    let sorted = |mut list: Vec<String>| {
        list.sort();
        list
    };
    let all = sorted(vec![f.a.clone(), f.b.clone(), c.clone()]);

    // First link: nothing stored yet, every project is linked.
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    f.commit(
        &c,
        &c_files(C_MAIN_V1),
        "ccc1110000000000000000000000000000000000",
    );
    let links = f.drain_checking_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(ids(&links[0]["relinked"]), all, "{}", links[0]);
    assert!(links[0]["edges"].as_u64().unwrap() >= 3, "{}", links[0]);

    // Nothing changed: every link is reused and every pair is unchanged.
    let again = f.store.graph_link(&f.workspace, &f.key).unwrap();
    assert_eq!(ids(&again["reused"]), all, "{again}");
    assert!(again["relinked"].as_array().unwrap().is_empty(), "{again}");
    assert!(
        again["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["unchanged"] == true),
        "{again}"
    );

    // C changes a function body: only C is linked again.
    f.commit(
        &c,
        &c_files(C_MAIN_V2),
        "ccc2220000000000000000000000000000000000",
    );
    let links = f.drain_checking_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(ids(&links[0]["relinked"]), vec![c.clone()], "{}", links[0]);
    assert_eq!(
        ids(&links[0]["reused"]),
        sorted(vec![f.a.clone(), f.b.clone()]),
        "{}",
        links[0]
    );

    // A gains a file that exports nothing: A has a new episode with the same
    // export surface. Only A is linked again; B's and C's links into A are
    // reused and move to A's new episode.
    let mut a_readme = a_files(A_CORE_V1);
    a_readme.push(("README.md", "# alib\n"));
    f.commit(&f.a, &a_readme, A_SHA2);
    let links = f.drain_checking_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        ids(&links[0]["relinked"]),
        vec![f.a.clone()],
        "{}",
        links[0]
    );
    let a2 = f.episode(&f.a);
    let incoming = f
        .store
        .graph_cross_edges(&f.workspace, &f.key, &f.a)
        .unwrap();
    let live: Vec<&Value> = incoming["incoming"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["valid_to_episode"].is_null())
        .collect();
    assert!(live.len() >= 3, "{incoming}");
    assert!(
        live.iter().all(|e| s(e, "target_episode") == a2),
        "{incoming}"
    );

    // A stops exporting `helper`: its export surface changed, so every
    // importer is linked again.
    let mut a_v2 = a_files(A_CORE_V2);
    a_v2.push(("README.md", "# alib\n"));
    f.commit(&f.a, &a_v2, "aaa3330000000000000000000000000000000000");
    let links = f.drain_checking_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(ids(&links[0]["relinked"]), all, "{}", links[0]);
    assert!(
        links[0]["unresolved"].as_u64().unwrap() >= 1,
        "{}",
        links[0]
    );

    // A full pass from here changes nothing either.
    let db = f.store.connect().unwrap();
    let before = link_rows(&db);
    let full = f.store.graph_link_full(&f.workspace, &f.key).unwrap();
    assert!(
        full["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["unchanged"] == true),
        "{full}"
    );
    assert_eq!(link_rows(&db), before);
}

/// Projects that export nothing (conversation-only projects, for example)
/// gain no link state.
#[test]
fn linking_projects_that_export_nothing_stores_no_link_state() {
    let f = Fixture::new();
    let notes = |text: &'static str| vec![("notes.md", text)];
    f.commit(&f.a, &notes("# Notes A\n"), A_SHA1);
    f.commit(&f.b, &notes("# Notes B\n"), B_SHA1);
    f.drain_jobs();
    let linked = f.store.graph_link(&f.workspace, &f.key).unwrap();
    assert_eq!(linked["edges"], 0, "{linked}");
    let db = f.store.connect().unwrap();
    for table in LINK_TABLES {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table} must stay empty: {linked}");
    }
}

const D_PYPROJECT: &str = "[project]\nname = \"dlib\"\nversion = \"2.0.0\"\n";
const D_UTIL_V1: &str = "def scale(x):\n    return x * 2\n";
const D_UTIL_V2: &str = "def scale(x):\n    return x * 2\n\ndef shift(x):\n    return x + 1\n";
const E_PYPROJECT: &str =
    "[project]\nname = \"eapp\"\nversion = \"0.1.0\"\ndependencies = [\"dlib>=2.0\"]\n";
const E_RUN: &str = "from dlib.util import scale\n\ndef run(x):\n    return scale(x)\n";

fn d_files(util: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("pyproject.toml", D_PYPROJECT),
        ("dlib/__init__.py", ""),
        ("dlib/util.py", util),
    ]
}

/// A project relinks only when a project it can import from (one claiming a
/// module it imports) joins, leaves or changes what it exports. A project
/// nobody imports joining the workspace relinks itself alone, so a large
/// workspace does not relink every project on every sync. Every pass leaves
/// the rows of a full pass.
#[test]
fn linking_relinks_only_the_projects_a_change_can_reach() {
    let f = Fixture::new();
    let c = f.register("capp", "Application C");
    let ids = |list: &Value| -> Vec<String> {
        let mut ids: Vec<String> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    let sorted = |mut list: Vec<String>| {
        list.sort();
        list
    };
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(&f.b, &b_files(), B_SHA1);
    f.commit(
        &c,
        &c_files(C_MAIN_V1),
        "ccc1110000000000000000000000000000000000",
    );
    f.drain_checking_links();

    // D exports dlib, which nobody imports yet: only D is linked.
    let d = f.register("dlib", "Library D");
    f.commit(
        &d,
        &d_files(D_UTIL_V1),
        "ddd1110000000000000000000000000000000000",
    );
    let links = f.drain_checking_links();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(ids(&links[0]["relinked"]), vec![d.clone()], "{}", links[0]);

    // E imports dlib: E is linked; A, B, C and D are not.
    let e = f.register("eapp", "Application E");
    f.commit(
        &e,
        &[
            ("pyproject.toml", E_PYPROJECT),
            ("eapp/__init__.py", ""),
            ("eapp/run.py", E_RUN),
        ],
        "eee1110000000000000000000000000000000000",
    );
    let links = f.drain_checking_links();
    assert_eq!(ids(&links[0]["relinked"]), vec![e.clone()], "{}", links[0]);
    assert!(links[0]["edges"].as_u64().unwrap() >= 4, "{}", links[0]);

    // D exports one more function: D itself and E, its only importer.
    f.commit(
        &d,
        &d_files(D_UTIL_V2),
        "ddd2220000000000000000000000000000000000",
    );
    let links = f.drain_checking_links();
    assert_eq!(
        ids(&links[0]["relinked"]),
        sorted(vec![d.clone(), e.clone()]),
        "{}",
        links[0]
    );
    assert_eq!(
        ids(&links[0]["reused"]),
        sorted(vec![f.a.clone(), f.b.clone(), c.clone()]),
        "{}",
        links[0]
    );
}

const G_PYPROJECT: &str = "[project]\nname = \"gapp\"\nversion = \"0.1.0\"\ndependencies = [\"alib>=1.0\", \"notindexed>=1\"]\n";
const G_RUN: &str = "from alib.core import process\nimport notindexed\n\ndef run(rows):\n    notindexed.go()\n    return process(rows)\n";

/// An importer's unresolved imports name only the exporters it can read:
/// a project it cannot import from joining or changing neither rewrites
/// them nor grows its stored history.
#[test]
fn an_unrelated_project_leaves_an_importers_unresolved_rows_alone() {
    let f = Fixture::new();
    let g = f.register("gapp", "Application G");
    f.commit(&f.a, &a_files(A_CORE_V1), A_SHA1);
    f.commit(
        &g,
        &[
            ("pyproject.toml", G_PYPROJECT),
            ("gapp/__init__.py", ""),
            ("gapp/run.py", G_RUN),
        ],
        "ggg1110000000000000000000000000000000000",
    );
    f.drain_checking_links();
    let rows = |repo: &str| -> (i64, i64, Vec<String>) {
        f.store
            .read(|db| {
                let (all, live): (i64, i64) = db.query_row(
                    "SELECT count(*), sum(valid_to_episode IS NULL) FROM gf_cross_unresolved WHERE source_repository_id=?",
                    [repo],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let mut read = db.prepare(
                    "SELECT DISTINCT e.key FROM gf_cross_unresolved u, json_each(u.exporter_episodes) e \
                     WHERE u.source_repository_id=? AND u.valid_to_episode IS NULL ORDER BY 1",
                )?;
                let keys = read
                    .query_map([repo], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<String>, _>>()?;
                Ok((all, live, keys))
            })
            .unwrap()
    };
    let before = rows(&g);
    assert!(before.1 >= 1, "gapp has an unresolved import: {before:?}");
    assert_eq!(before.2, vec![f.a.clone()], "only alib is read");

    // dlib joins and changes; gapp cannot import from it.
    let d = f.register("dlib", "Library D");
    f.commit(
        &d,
        &d_files(D_UTIL_V1),
        "ddd1110000000000000000000000000000000000",
    );
    f.drain_checking_links();
    f.commit(
        &d,
        &d_files(D_UTIL_V2),
        "ddd2220000000000000000000000000000000000",
    );
    f.drain_checking_links();
    assert_eq!(rows(&g), before, "gapp's unresolved rows were rewritten");
}

const STUB_GOMOD: &str = "module github.com/acme/protos\n\ngo 1.22\n";
const STUB_PB: &str = "package userpb\n\ntype UserServiceClient interface{ Get() }\n\ntype UserServiceServer interface{ Get() }\n\nfunc NewUserServiceClient(cc any) UserServiceClient { return nil }\n\nfunc RegisterUserServiceServer(s any, srv UserServiceServer) {}\n";
const USERS_GOMOD: &str =
    "module github.com/acme/users\n\ngo 1.22\n\nrequire github.com/acme/protos v1.0.0\n";
const USERS_SERVE: &str = "package server\n\nimport userpb \"github.com/acme/protos/userpb\"\n\nfunc Serve(s any, impl userpb.UserServiceServer) {\n\tuserpb.RegisterUserServiceServer(s, impl)\n}\n";
const USERS_IDLE: &str = "package server\n\nimport userpb \"github.com/acme/protos/userpb\"\n\nfunc Serve(s any, impl userpb.UserServiceServer) {\n\t_ = impl\n}\n";
const FEED_GOMOD: &str =
    "module github.com/acme/feed\n\ngo 1.22\n\nrequire github.com/acme/protos v1.0.0\n";
const FEED_DIAL: &str = "package client\n\nimport \"github.com/acme/protos/userpb\"\n\nfunc Dial(conn any) userpb.UserServiceClient {\n\treturn userpb.NewUserServiceClient(conn)\n}\n";
const FEED_FAKE: &str = "package fake\n\nimport \"github.com/acme/protos/userpb\"\n\nfunc Fake(s any, impl userpb.UserServiceServer) {\n\tuserpb.RegisterUserServiceServer(s, impl)\n}\n";

fn grpc_rows(
    db: &rusqlite::Connection,
) -> Vec<(String, String, String, String, String, Option<String>)> {
    let mut stmt = db
        .prepare(
            "SELECT source_repository_id,source_node_id,target_repository_id,target_node_id,symbol,valid_to_repository_id \
             FROM gf_cross_edges WHERE relation='grpc_calls' ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A client creating `pb.NewUserServiceClient` links to the project calling
/// `pb.RegisterUserServiceServer` for the same generated package; a fake
/// server in the client's own tests does not count. Incremental passes keep
/// the rows a full pass leaves, and the link closes when the server stops
/// registering the service.
#[test]
fn a_grpc_client_links_to_the_project_serving_its_service() {
    let f = Fixture::new();
    let protos = f.register("protos", "Protos");
    let users = f.register("users", "Users");
    let feed = f.register("feed", "Feed");
    f.commit(
        &protos,
        &[("go.mod", STUB_GOMOD), ("userpb/user.pb.go", STUB_PB)],
        "ppp1110000000000000000000000000000000000",
    );
    f.commit(
        &users,
        &[("go.mod", USERS_GOMOD), ("server/serve.go", USERS_SERVE)],
        "uuu1110000000000000000000000000000000000",
    );
    f.commit(
        &feed,
        &[
            ("go.mod", FEED_GOMOD),
            ("client/dial.go", FEED_DIAL),
            ("tests/fake.go", FEED_FAKE),
        ],
        "fff1110000000000000000000000000000000000",
    );
    f.drain_checking_links();
    let db = f.store.connect().unwrap();
    let live: Vec<_> = grpc_rows(&db)
        .into_iter()
        .filter(|r| r.5.is_none())
        .collect();
    assert_eq!(live.len(), 1, "{live:?}");
    let (source, source_node, target, target_node, service, _) = &live[0];
    assert_eq!((source, target), (&feed, &users));
    assert!(source_node.contains("dial"), "{source_node}");
    assert!(target_node.contains("serve"), "{target_node}");
    assert_eq!(service, "UserService");
    let (confidence, derivation): (String, String) = db
        .query_row(
            "SELECT confidence,derivation FROM gf_cross_edges WHERE relation='grpc_calls' AND valid_to_episode IS NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(confidence, "INFERRED");
    assert_eq!(
        derivation,
        "grpc_service:github.com/acme/protos/userpb#UserService"
    );

    // The server stops registering the service: the link closes on its side.
    f.commit(
        &users,
        &[("go.mod", USERS_GOMOD), ("server/serve.go", USERS_IDLE)],
        "uuu2220000000000000000000000000000000000",
    );
    f.drain_checking_links();
    let rows = grpc_rows(&db);
    assert!(rows.iter().all(|r| r.5.is_some()), "{rows:?}");
    assert_eq!(rows[0].5.as_deref(), Some(users.as_str()), "{rows:?}");
}
