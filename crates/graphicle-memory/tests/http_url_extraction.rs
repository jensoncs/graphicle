//! URL propagation acceptance set. Seeds only the selected caller declaration;
//! measures its route facts and actual workspace links, not symbol discovery.
use graphicle::{extractors::routes, graph::Graph};
use graphicle_memory::graph::{cross_repo_http, cross_repo_imports::ExportIndex};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn graph(nodes: Vec<Value>, edges: Vec<Value>) -> Graph {
    Graph::from_node_link(
        &json!({"directed":true,"multigraph":false,"graph":{},
        "nodes":nodes,"links":edges}),
        "links",
    )
    .unwrap()
}

#[test]
fn http_url_extraction_acceptance() {
    // prelude, caller body, expected method. Every server exposes the same
    // literal path. No expected answer is derived from extractor output.
    let cases = [
        (
            "direct control",
            "",
            "return fetch('/inventory/orders');",
            Some("GET"),
        ),
        (
            "const URL",
            "",
            "const url='/inventory/orders'; return fetch(url);",
            Some("GET"),
        ),
        (
            "const alias",
            "",
            "const path='/inventory/orders'; const url=path; return fetch(url);",
            Some("GET"),
        ),
        (
            "module prefix",
            "const prefix='/inventory';",
            "const url=prefix+'/orders'; return fetch(url);",
            Some("GET"),
        ),
        (
            "template prefix",
            "",
            "const prefix='/inventory'; const url=`${prefix}/orders`; return axios.get(url);",
            Some("GET"),
        ),
        (
            "inline concatenation",
            "",
            "return fetch('/inventory'+'/orders');",
            Some("GET"),
        ),
        (
            "Axios config variable",
            "",
            "const url='/inventory/orders'; return axios({url:url,method:'POST'});",
            Some("POST"),
        ),
        (
            "named helper",
            "function load(path) { return fetch('/inventory'+path); }",
            "return load('/orders');",
            Some("GET"),
        ),
        (
            "arrow helper",
            "const load=(path)=>fetch(`/inventory${path}`,{method:'POST'});",
            "return load('/orders');",
            Some("POST"),
        ),
        (
            "typed helper",
            "const load=(path: string)=>axios.get('/inventory'+path);",
            "return load('/orders');",
            Some("GET"),
        ),
        (
            "local helper",
            "",
            "const load=(path)=>fetch('/inventory'+path); return load('/orders');",
            Some("GET"),
        ),
        (
            "mutable URL",
            "",
            "let url='/inventory/orders'; return fetch(url);",
            None,
        ),
        (
            "reassigned URL",
            "",
            "let url='/inventory/orders'; url=value; return fetch(url);",
            None,
        ),
        (
            "parameter shadow",
            "const value='/inventory/orders';",
            "return fetch(value);",
            None,
        ),
        (
            "block shadow",
            "",
            "const url='/inventory/orders'; { const url=value; return fetch(url); }",
            None,
        ),
        (
            "use before initialization",
            "",
            "return fetch(url); const url='/inventory/orders';",
            None,
        ),
        (
            "dynamic initializer",
            "",
            "const url=makeUrl(); return fetch(url);",
            None,
        ),
        (
            "dynamic concatenation",
            "",
            "const url='/inventory/'+value; return fetch(url);",
            None,
        ),
        (
            "dynamic template",
            "",
            "const url=`/inventory/${value}`; return fetch(url);",
            None,
        ),
        (
            "member URL",
            "",
            "const options={url:'/inventory/orders'}; return fetch(options.url);",
            None,
        ),
        (
            "escaped URL",
            "",
            r"const url='/inventory/\u006frders'; return fetch(url);",
            None,
        ),
        (
            "reassigned helper",
            "function load(path) { return fetch('/inventory'+path); } load=other;",
            "return load('/orders');",
            None,
        ),
        (
            "helper shadow",
            "function load(path) { return fetch('/inventory'+path); }",
            "const load=value; return load('/orders');",
            None,
        ),
        (
            "conditional helper",
            "function load(path) { if(flag) return fetch('/inventory'+path); return fetch('/other'); }",
            "return load('/orders');",
            None,
        ),
        (
            "recursive helper",
            "function load(path) { return load(path); }",
            "return load('/orders');",
            None,
        ),
        (
            "dynamic helper argument",
            "function load(path) { return fetch('/inventory'+path); }",
            "return load(value);",
            None,
        ),
        (
            "imported helper",
            "import {load} from './client';",
            "return load('/orders');",
            None,
        ),
        (
            "dynamic helper method",
            "function load(path) { return fetch('/inventory'+path,{method:verb}); }",
            "return load('/orders');",
            None,
        ),
        (
            "multiple statements",
            "function load(path) { audit(); return fetch('/inventory'+path); }",
            "return load('/orders');",
            None,
        ),
    ];
    let (mut expected_links, mut correct, mut false_links) = (0, 0, 0);
    let mut rows = vec![];
    for (name, prelude, body, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let client = root.join("client.ts");
        std::fs::write(
            &client,
            format!("{prelude}\nfunction caller(value) {{ {body} }}"),
        )
        .unwrap();
        let mut nodes = vec![json!({"id":"caller","label":"caller()","file_type":"code",
            "source_file":client,"source_location":"L2"})];
        let mut edges = vec![];
        routes::resolve(&[client], &mut nodes, &mut edges, &[], &root);
        let request_facts = nodes
            .iter()
            .filter(|n| n["node_kind"] == "http_request")
            .count();
        let client_graph = graph(nodes, edges);
        let server = root.join("server.ts");
        std::fs::write(&server, "const app=express();\napp.get('/inventory/orders', handler);\napp.post('/inventory/orders', handler);").unwrap();
        let mut nodes = vec![];
        let mut edges = vec![];
        routes::resolve(&[server], &mut nodes, &mut edges, &[], &root);
        let server_graph = graph(nodes, edges);
        let indexes = [ExportIndex {
            repo: "inventory".into(),
            http_routes: cross_repo_http::exports(
                &server_graph,
                "inventory",
                Some("server-revision"),
            ),
            ..Default::default()
        }];
        let links = cross_repo_http::link(&client_graph, "web", Some("client-revision"), &indexes);
        let actual: BTreeSet<_> = links
            .iter()
            .filter_map(|edge| {
                indexes[0]
                    .http_routes
                    .iter()
                    .find(|route| route.node == edge.target)
                    .map(|route| format!("{} {} {}", route.node.repo, route.method, route.path))
            })
            .collect();
        let wanted: BTreeSet<_> = expected
            .map(|method| format!("inventory {method} /inventory/orders"))
            .into_iter()
            .collect();
        expected_links += wanted.len();
        correct += actual.intersection(&wanted).count();
        false_links += actual.difference(&wanted).count();
        rows.push(
            json!({"case":name,"expected":wanted,"actual":actual,"request_facts":request_facts}),
        );
    }
    println!(
        "HTTP_URL_EVALUATION={}",
        json!({"cases":rows,"expected_links":expected_links,
        "correct_links":correct,"false_links":false_links})
    );
    assert_eq!((correct, false_links), (expected_links, 0));
}
