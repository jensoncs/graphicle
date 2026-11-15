//! A Go service and a Python agent in one repository (Siren and its alert
//! review agent): the agent's standard-library `http.server` endpoints are
//! routes, and its `urllib.request` calls reach the Go service's Gin routes.
use graphicle::extractors::routes;
use serde_json::Value;
use std::fs;

fn extract(files: &[(&str, &str)]) -> Value {
    routes::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths: Vec<_> = files
        .iter()
        .map(|(name, body)| {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            path
        })
        .collect();
    graphicle::api::extract(
        &paths,
        &graphicle::api::ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.join("out")),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap()
}

/// `METHOD path` of each route, with its handler's label.
fn routes_of(graph: &Value) -> Vec<(String, String)> {
    let nodes = graph["nodes"].as_array().unwrap();
    let label = |id: &Value| {
        nodes
            .iter()
            .find(|n| &n["id"] == id)
            .and_then(|n| n["label"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut out: Vec<(String, String)> = nodes
        .iter()
        .filter(|n| n["node_kind"] == "route")
        .map(|route| {
            let handler = graph["edges"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["relation"] == "handles" && e["target"] == route["id"])
                .map(|e| label(&e["source"]))
                .unwrap_or_default();
            (
                format!(
                    "{} {}",
                    route["route_method"].as_str().unwrap(),
                    route["route_path"].as_str().unwrap()
                ),
                handler,
            )
        })
        .collect();
    out.sort();
    out
}

/// (caller label, `METHOD path` of the route it calls).
fn http_calls(graph: &Value) -> Vec<(String, String)> {
    let nodes = graph["nodes"].as_array().unwrap();
    let node = |id: &Value| nodes.iter().find(|n| &n["id"] == id).unwrap();
    let mut out: Vec<(String, String)> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "http_calls")
        .map(|e| {
            let route = node(&e["target"]);
            (
                node(&e["source"])["label"].as_str().unwrap().to_string(),
                format!(
                    "{} {}",
                    route["route_method"].as_str().unwrap(),
                    route["route_path"].as_str().unwrap()
                ),
            )
        })
        .collect();
    out.sort();
    out
}

const ROUTER_GO: &str = "package httpserver\n\nimport \"github.com/gin-gonic/gin\"\n\nfunc Routes(handler *Handler) *gin.Engine {\n\trouter := gin.New()\n\tv1 := router.Group(\"/siren/v1\")\n\t{\n\t\talerts := v1.Group(\"/alerts\")\n\t\talerts.GET(\"/notifications/\", handler.ListNotifications)\n\t}\n\treturn router\n}\n";

const SERVER_PY: &str = "import urllib.request\nfrom http.server import BaseHTTPRequestHandler\n\n\nclass Handler(BaseHTTPRequestHandler):\n    def do_GET(self):\n        if self.path == \"/ping\":\n            return self.ok()\n        self.send_error(404)\n\n    def do_POST(self):\n        if self.path != \"/v1/review\":\n            return self.send_error(404)\n        return self.review()\n\n\nclass Siren:\n    def notifications(self, q):\n        with urllib.request.urlopen(f\"{self.url}/siren/v1/alerts/notifications?{q}\", timeout=5) as resp:\n            return resp.read()\n";

const CLIENT_PY: &str = "import urllib.request\n\n\ndef ask_review(base, payload):\n    req = urllib.request.Request(f\"{base}/v1/review\", data=payload, method=\"POST\")\n    return urllib.request.urlopen(req)\n\n\ndef ping(base):\n    return urllib.request.urlopen(base + \"/ping\")\n";

/// A dispatch table read with the request path: each key is a route, served
/// by the function the table maps it to.
const TABLE_PY: &str = "import http.server\n\n\ndef review_alert(body):\n    return body\n\n\ndef watch_alert(body):\n    return body\n\n\nENDPOINTS = {\"/v2/review\": review_alert, \"/v2/watch\": watch_alert}\n\n\nclass Handler(http.server.BaseHTTPRequestHandler):\n    def do_POST(self):\n        endpoint = ENDPOINTS.get(self.path)\n        return endpoint(self.body())\n\n    def do_PUT(self):\n        return ENDPOINTS[self.path](self.body())\n";

#[test]
fn http_server_endpoints_are_routes_and_urllib_requests_reach_routes() {
    let graph = extract(&[
        ("pkg/application/httpserver/router.go", ROUTER_GO),
        ("alert-review/server.py", SERVER_PY),
        ("alert-review/table.py", TABLE_PY),
        ("tools/client.py", CLIENT_PY),
    ]);
    let routes = routes_of(&graph);
    for want in [
        ("GET /ping", "do_GET()"),
        ("POST /v1/review", "do_POST()"),
        ("POST /v2/watch", "watch_alert()"),
        ("PUT /v2/review", "review_alert()"),
    ] {
        assert!(
            routes
                .iter()
                .any(|(r, h)| r == want.0 && h.ends_with(want.1)),
            "{want:?} in {routes:#?}"
        );
    }
    let calls = http_calls(&graph);
    for want in [
        // Python to Go: the agent reads Siren's notifications.
        ("notifications()", "GET /siren/v1/alerts/notifications/"),
        // `Request(url, data=..., method="POST")`.
        ("ask_review()", "POST /v1/review"),
    ] {
        assert!(
            calls
                .iter()
                .any(|(c, r)| c.ends_with(want.0) && r == want.1),
            "{want:?} in {calls:#?}"
        );
    }
    // `base + "/ping"` is not a literal path: no guessed link.
    assert!(
        !calls.iter().any(|(c, _)| c.ends_with("ping()")),
        "{calls:#?}"
    );
}

/// `data=None` sends no body: still a GET. A table handler resolves in its
/// own file even when another file defines a function of the same name.
#[test]
fn a_none_body_is_a_get_and_table_handlers_resolve_in_their_file() {
    let graph = extract(&[
        (
            "server.py",
            "import http.server\n\n\ndef status(body):\n    return body\n\n\nROUTES = {\"/v1/status\": status}\n\n\nclass H(http.server.BaseHTTPRequestHandler):\n    def do_GET(self):\n        return ROUTES[self.path](None)\n",
        ),
        ("other.py", "def status():\n    return 1\n"),
        (
            "client.py",
            "import urllib.request\n\n\ndef poll(base):\n    return urllib.request.urlopen(f\"{base}/v1/status\", None, 5)\n\n\ndef poll_kw(base):\n    return urllib.request.urlopen(f\"{base}/v1/status\", data=None)\n",
        ),
    ]);
    let routes = routes_of(&graph);
    assert!(
        routes
            .iter()
            .any(|(r, h)| r == "GET /v1/status" && h.ends_with("status()")),
        "{routes:#?}"
    );
    let nodes = graph["nodes"].as_array().unwrap();
    let route = nodes
        .iter()
        .find(|n| n["route_path"] == "/v1/status")
        .unwrap();
    let handler = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["relation"] == "handles" && e["target"] == route["id"])
        .and_then(|e| nodes.iter().find(|n| n["id"] == e["source"]))
        .unwrap();
    assert_eq!(handler["source_file"], "server.py", "{handler:#?}");
    let calls = http_calls(&graph);
    for caller in ["poll()", "poll_kw()"] {
        assert!(
            calls
                .iter()
                .any(|(c, r)| c.ends_with(caller) && r == "GET /v1/status"),
            "{caller} in {calls:#?}"
        );
    }
}

/// A `self.path` comparison outside a `do_<METHOD>` handler is not a route.
#[test]
fn a_path_comparison_elsewhere_is_not_a_route() {
    let graph = extract(&[(
        "util.py",
        "class Page:\n    def matches(self):\n        return self.path == \"/home\"\n\n\ndef do_GET(self):\n    if self.path == \"/top\":\n        return 1\n",
    )]);
    assert!(routes_of(&graph).is_empty(), "{:#?}", routes_of(&graph));
}
