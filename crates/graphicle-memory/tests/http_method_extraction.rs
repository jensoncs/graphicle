//! Synthetic acceptance set for the route extractor and real workspace HTTP
//! linker. Caller declarations are seeded: this measures HTTP extraction and
//! linkage, not declaration discovery or deployed service identity.
use graphicle::{extractors::routes, graph::Graph};
use graphicle_memory::graph::{cross_repo_http, cross_repo_imports::ExportIndex};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

fn source(root: &Path, name: &str, body: &str, symbol: &str) -> (std::path::PathBuf, Value) {
    let file = root.join(name);
    std::fs::write(&file, body).unwrap();
    let node = json!({"id":symbol,"label":format!("{symbol}()"),"file_type":"code",
        "source_file":file,"source_location":"L1"});
    (file, node)
}

fn graph(nodes: Vec<Value>, edges: Vec<Value>) -> Graph {
    Graph::from_node_link(
        &json!({"directed":true,"multigraph":false,
        "graph":{},"nodes":nodes,"links":edges}),
        "links",
    )
    .unwrap()
}

#[test]
fn http_method_extraction_acceptance() {
    // Expected targets are specified independently of extractor output. A
    // single POST server makes unknown methods an effective negative control:
    // the old local matcher can otherwise treat them as a wildcard.
    let cases = [
        (
            "existing axios.post",
            "axios.post('/orders', {})",
            Some("POST"),
            false,
        ),
        ("fetch default", "fetch('/orders')", Some("GET"), false),
        (
            "fetch empty options",
            "fetch('/orders', {})",
            Some("GET"),
            false,
        ),
        (
            "fetch headers only",
            "fetch('/orders', {headers: {'x-client':'test'}})",
            Some("GET"),
            false,
        ),
        (
            "fetch post",
            "fetch('/orders', {method:'POST'})",
            Some("POST"),
            false,
        ),
        (
            "fetch lowercase",
            "fetch('/orders', {method:'delete'})",
            Some("DELETE"),
            false,
        ),
        (
            "fetch multiline",
            "fetch('/orders', {\n method: 'PATCH'\n})",
            Some("PATCH"),
            false,
        ),
        (
            "fetch quoted key",
            "fetch('/orders', {'method':'HEAD'})",
            Some("HEAD"),
            false,
        ),
        (
            "fetch duplicate key",
            "fetch('/orders', {method:'GET', method:'PUT'})",
            Some("PUT"),
            false,
        ),
        ("axios default", "axios('/orders')", Some("GET"), false),
        (
            "axios url options",
            "axios('/orders', {method:'POST'})",
            Some("POST"),
            false,
        ),
        (
            "axios config",
            "axios({url:'/orders', method:'DELETE'})",
            Some("DELETE"),
            false,
        ),
        (
            "dynamic method",
            "fetch('/orders', {method: verb})",
            None,
            false,
        ),
        ("dynamic options", "fetch('/orders', options)", None, false),
        (
            "shorthand method",
            "fetch('/orders', {method})",
            None,
            false,
        ),
        (
            "spread options",
            "fetch('/orders', {method:'GET', ...options})",
            None,
            false,
        ),
        (
            "computed key",
            "fetch('/orders', {[key]:'POST'})",
            None,
            false,
        ),
        (
            "method getter",
            "fetch('/orders', {get method() { return verb; }})",
            None,
            false,
        ),
        (
            "inherited method",
            "fetch('/orders', {__proto__: options})",
            None,
            false,
        ),
        (
            "axios dynamic method",
            "axios('/orders', {method: verb})",
            None,
            false,
        ),
        (
            "axios dynamic url",
            "axios({url: target, method:'POST'})",
            None,
            false,
        ),
        (
            "duplicate services",
            "fetch('/orders', {method:'POST'})",
            None,
            true,
        ),
    ];
    let mut rows = vec![];
    let (mut expected_links, mut correct, mut false_links, mut local_false_links) = (0, 0, 0, 0);
    for (name, call, expected, ambiguous) in cases {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (client, caller) = source(
            &root,
            "client.ts",
            &format!(
                "async function caller(verb, options, method, key, target) {{ return {call}; }}"
            ),
            "caller",
        );
        let methods: &[&str] = if expected.is_some() {
            &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"]
        } else {
            &["POST"]
        };
        let registrations = methods
            .iter()
            .map(|m| format!("app.{}('/orders', handler);\n", m.to_lowercase()))
            .collect::<String>();
        let (server, handler) = source(
            &root,
            "server.ts",
            &format!("function handler() {{}}\nconst app = express();\n{registrations}"),
            "handler",
        );
        let mut server_nodes = vec![handler.clone()];
        let mut server_edges = vec![];
        routes::resolve(
            std::slice::from_ref(&server),
            &mut server_nodes,
            &mut server_edges,
            &[],
            &root,
        );
        let server_graph = graph(server_nodes, server_edges);
        let exports = cross_repo_http::exports(&server_graph, "api", Some("server-revision"));
        let mut indexes = vec![ExportIndex {
            repo: "api".into(),
            http_routes: exports,
            ..Default::default()
        }];
        if ambiguous {
            let mut duplicate = indexes[0].clone();
            duplicate.repo = "other-api".into();
            for route in &mut duplicate.http_routes {
                route.node.repo = duplicate.repo.clone();
            }
            indexes.push(duplicate);
        }
        let mut client_nodes = vec![caller.clone()];
        let mut client_edges = vec![];
        routes::resolve(
            std::slice::from_ref(&client),
            &mut client_nodes,
            &mut client_edges,
            &[],
            &root,
        );
        let requests = client_nodes
            .iter()
            .filter(|n| n["node_kind"] == "http_request")
            .count();
        let links = cross_repo_http::link(
            &graph(client_nodes, client_edges),
            "web",
            Some("client-revision"),
            &indexes,
        );
        let actual: BTreeSet<_> = links
            .iter()
            .filter_map(|e| {
                indexes
                    .iter()
                    .flat_map(|i| &i.http_routes)
                    .find(|r| r.node == e.target)
                    .map(|r| format!("{} {}", r.node.repo, r.method))
            })
            .collect();
        let wanted: BTreeSet<_> = expected.map(|m| format!("api {m}")).into_iter().collect();
        expected_links += wanted.len();
        correct += actual.intersection(&wanted).count();
        false_links += actual.difference(&wanted).count();
        // Also ensure unknown methods don't gain a local edge just because a
        // server happens to have only one endpoint at the requested path.
        let mut nodes = vec![caller, handler];
        let mut edges = vec![];
        routes::resolve(&[client, server], &mut nodes, &mut edges, &[], &root);
        let local: Vec<_> = edges
            .iter()
            .filter(|e| e["relation"] == "http_calls")
            .collect();
        if expected.is_none() && !ambiguous {
            local_false_links += local.len();
        }
        rows.push(json!({"case":name,"expected":wanted,"actual":actual,
            "request_facts":requests,"local_links":local.len()}));
    }
    println!(
        "HTTP_METHOD_EVALUATION={}",
        json!({"cases":rows,"expected_links":expected_links,
        "correct_links":correct,"false_links":false_links,"local_false_links":local_false_links})
    );
    assert_eq!(
        (correct, false_links, local_false_links),
        (expected_links, 0, 0)
    );
}
