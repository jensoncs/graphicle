//! Answers that list a symbol's callers or references say when an
//! unresolved call of the same episode could name it, instead of a
//! confident "0": `get_neighbors` and `get_node` (in `structuredContent`
//! and the citations block) and the identifier route of `context`.
//!
//! `measure(x)` calls `x.size()`; `Box` and `Ring` both define `size`, so
//! receiver resolution records the call as unresolved rather than guess.
use graphicle_runtime::{
    graph_tools, jobs,
    store::{Store, s},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const SHAPES: &str = "class Box:\n    def size(self):\n        return 1\n\n\nclass Ring:\n    def size(self):\n        return 2\n\n\ndef measure(x):\n    return x.size()\n";
const USES: &str = "from shapes import measure, Box\n\n\ndef run():\n    return measure(Box())\n";

struct Fixture {
    _dir: TempDir,
    store: Store,
    workspace: String,
    key: String,
    repo: String,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        crate::edition_setup();
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("Incomplete").unwrap();
        let workspace = s(&ws, "id").to_string();
        let key = s(&ws, "capability").to_string();
        let registered = store
            .register(
                &workspace,
                &key,
                &json!({"project_key":"incomplete","name":"Incomplete"}),
            )
            .unwrap();
        let repo = s(&registered, "id").to_string();
        let f = Self {
            _dir: dir,
            store,
            workspace,
            key,
            repo,
        };
        let args = |mut args: Value| {
            args["repository_id"] = json!(f.repo);
            args
        };
        let status = f
            .store
            .project_status(&f.workspace, &f.key, &args(json!({})))
            .unwrap();
        let manifest: Vec<Value> = files
            .iter()
            .map(|(name, text)| json!({"path":name,"sha256":graphicle_memory::text::sha256(text.as_bytes()),"bytes":text.len()}))
            .collect();
        let begin = f
            .store
            .sync_prepare(
                &f.workspace,
                &f.key,
                &args(json!({"expected_snapshot":status["repository"]["snapshot"],"manifest":manifest})),
            )
            .unwrap();
        let upload: Vec<Value> = files
            .iter()
            .map(|(name, text)| json!({"path":name,"content":text}))
            .collect();
        f.store
            .sync_upload(
                &f.workspace,
                &f.key,
                &args(json!({"sync_id":begin["sync_id"],"files":upload})),
            )
            .unwrap();
        f.store
            .sync_commit(
                &f.workspace,
                &f.key,
                &args(json!({"sync_id":begin["sync_id"]})),
            )
            .unwrap();
        jobs::drain_projection(&f.store, &f.repo).unwrap();
        f
    }
    fn tool(&self, tool: &str, args: Value) -> Value {
        let mut args = args;
        args["tool"] = json!(tool);
        args["repository_id"] = json!(self.repo);
        graph_tools::call(&self.store, &self.workspace, &self.key, &args).unwrap()
    }
    fn context(&self, question: &str) -> Value {
        self.store
            .project_context(
                &self.workspace,
                &self.key,
                &json!({"repository_id":self.repo,"question":question,"identifier_route":true}),
            )
            .unwrap()
    }
}

#[test]
fn get_neighbors_names_the_unresolved_calls_that_could_reach_the_symbol() {
    let f = Fixture::new(&[("shapes.py", SHAPES), ("uses.py", USES)]);
    let reply = f.tool("get_neighbors", json!({"node_id":"shapes_box_size"}));
    let found = &reply["possibly_incomplete"]["unresolved_calls"];
    assert_eq!(found[0]["name"], "size", "{reply:#}");
    assert_eq!(found[0]["count"], 1, "{reply:#}");
    assert_eq!(found[0]["sites"][0]["file"], "shapes.py", "{reply:#}");
    assert_eq!(found[0]["sites"][0]["line"], 12, "{reply:#}");
    let text = graph_tools::citations_text(&reply);
    assert!(
        text.contains("Possibly incomplete: 1 unresolved call names size ("),
        "{text}"
    );
    assert!(text.contains("shapes.py:12"), "{text}");
    // get_node says it too; a symbol no unresolved call could name does not.
    let node = f.tool("get_node", json!({"node_id":"shapes_box_size"}));
    assert_eq!(
        node["possibly_incomplete"]["unresolved_calls"][0]["count"], 1,
        "{node:#}"
    );
    // The project's file-relationship view carries the same calls.
    let status = f
        .store
        .project_status(&f.workspace, &f.key, &json!({"repository_id":f.repo}))
        .unwrap();
    let calls = status["graph"]["unresolved_calls"].as_array().unwrap();
    assert!(
        calls.iter().any(|c| c["callee"] == "size"),
        "{:#}",
        status["graph"]
    );
    let measure = f.tool("get_neighbors", json!({"node_id":"shapes_measure"}));
    assert!(measure.get("possibly_incomplete").is_none(), "{measure:#}");
    assert!(!graph_tools::citations_text(&measure).contains("Possibly incomplete"));
}

#[test]
fn the_identifier_route_says_callers_may_be_missing() {
    let f = Fixture::new(&[("shapes.py", SHAPES), ("uses.py", USES)]);
    let packet = f.context("who calls `Box.size`");
    assert_eq!(packet["route"]["path"], "identifier", "{packet:#}");
    let markdown = s(&packet, "markdown");
    assert!(
        markdown.contains("Possibly incomplete: 1 unresolved call names size ("),
        "{markdown}"
    );
    let found = &packet["possibly_incomplete"]["unresolved_calls"][0];
    assert_eq!(found["name"], "size", "{packet:#}");
    assert_eq!(found["sites"][0]["file"], "shapes.py", "{packet:#}");
    // A name no unresolved call could reach: no line, no field.
    let run = f.context("who calls `measure`");
    assert!(
        !s(&run, "markdown").contains("Possibly incomplete"),
        "{run:#}"
    );
    assert!(run.get("possibly_incomplete").is_none(), "{run:#}");
}

/// The measured case: `main.go` imports a module that is not available, so
/// its package does not type-check and `server.RunAthenaServer()` stays
/// unresolved. The definition's neighbours list no caller; the answers say
/// the call at main.go:10 may be one.
#[test]
fn a_go_call_in_a_package_that_did_not_load_is_named() {
    let main = "package main\n\nimport (\n\t\"example.com/missing/telemetry\"\n\t\"example.com/rbac/server\"\n)\n\nfunc main() {\n\ttelemetry.Start()\n\tserver.RunAthenaServer()\n}\n";
    let server = "package server\n\nfunc RunAthenaServer() {\n\tlisten()\n}\n\nfunc listen() {}\n";
    let f = Fixture::new(&[
        ("go.mod", "module example.com/rbac\n\ngo 1.23\n"),
        ("main.go", main),
        ("server/server.go", server),
    ]);
    let reply = f.tool("get_neighbors", json!({"label":"RunAthenaServer"}));
    assert!(!s(&reply, "text").contains("main"), "{reply:#}");
    let site = &reply["possibly_incomplete"]["unresolved_calls"][0]["sites"][0];
    assert_eq!(
        (
            site["file"].as_str(),
            site["line"].as_u64(),
            site["callee"].as_str()
        ),
        (Some("main.go"), Some(10), Some("server.RunAthenaServer")),
        "{reply:#}"
    );
    assert!(
        graph_tools::citations_text(&reply)
            .contains("Possibly incomplete: 1 unresolved call names RunAthenaServer ("),
        "{reply:#}"
    );
    let packet = f.context("who calls `RunAthenaServer`");
    let markdown = s(&packet, "markdown");
    assert!(
        markdown.contains("`RunAthenaServer`: 1 definition, 0 references"),
        "{markdown}"
    );
    assert!(markdown.contains(") at main.go:10."), "{markdown}");
}
