//! Conservative HTTP linkage over route/request facts from the existing AST
//! extractor. A relative URL with one workspace route is an inference, not a
//! claim about a deployed host. Absolute URLs require a matching known origin.
use super::cross_repo_imports::{CrossEdge, EdgeEnd, ExportIndex, INFERRED, is_test_path};
use graphicle::{graph::Graph, record::Properties, serve_search::id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const RELATION: &str = "http_calls";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteExport {
    pub node: EdgeEnd,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub ambiguous: bool,
    /// Handler first, then middleware; all are explicit registration edges.
    pub related: Vec<EdgeEnd>,
}

fn text<'a>(a: &'a impl Properties, name: &str) -> &'a str {
    a.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

fn end(graph: &Graph, i: usize, repo: &str, episode: Option<&str>) -> EdgeEnd {
    let a = &graph.nodes()[i].attributes;
    EdgeEnd {
        repo: repo.into(),
        node_id: id(graph, i),
        stable_id: a
            .get("stable_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        episode: episode.map(str::to_string),
        relpath: ["relpath", "source_file"].iter().find_map(|k| {
            let value = text(a, k);
            (!value.is_empty()).then(|| value.to_string())
        }),
    }
}

pub fn exports(graph: &Graph, repo: &str, episode: Option<&str>) -> Vec<RouteExport> {
    let mut out = Vec::new();
    for (i, n) in graph.nodes().iter().enumerate() {
        if text(&n.attributes, "node_kind") != "route" {
            continue;
        }
        let route_end = end(graph, i, repo, episode);
        // Match the existing module-export policy. Test routers are local
        // fixtures, not independent workspace server destinations.
        if route_end.relpath.as_deref().is_some_and(is_test_path) {
            continue;
        }
        let mut handlers = BTreeSet::new();
        let mut middleware = BTreeSet::new();
        for (source, target, edge) in graph.in_edges(i) {
            if target != i {
                continue;
            }
            match text(&edge.attributes, "relation") {
                "handles" => {
                    handlers.insert(source);
                }
                "middleware_for" => {
                    middleware.insert(source);
                }
                _ => {}
            }
        }
        let related = handlers
            .iter()
            .chain(&middleware)
            .map(|&i| end(graph, i, repo, episode))
            .collect();
        out.push(RouteExport {
            node: route_end,
            method: text(&n.attributes, "route_method").into(),
            path: text(&n.attributes, "route_path").into(),
            origin: n
                .attributes
                .get("route_origin")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            ambiguous: handlers.len() > 1,
            related,
        });
    }
    out
}

fn origin(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !["http", "https"].contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

fn compatible(method: &str, request: &str, route: &RouteExport) -> bool {
    if method.is_empty() || !(route.method == method || route.method == "ANY") {
        return false;
    }
    // A relative root path has an unknown host. Protocol-relative and dynamic
    // bases cannot be resolved conservatively from these facts.
    if request.starts_with("//") || (!request.starts_with('/') && origin(request).is_none()) {
        return false;
    }
    if let Some(host) = origin(request)
        && route.origin.as_deref().and_then(origin).as_deref() != Some(host.as_str())
    {
        return false;
    }
    graphicle::extractors::routes::http_path_matches(&route.path, request)
}

pub fn link(
    graph: &Graph,
    repo: &str,
    episode: Option<&str>,
    indexes: &[ExportIndex],
) -> Vec<CrossEdge> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for n in graph.nodes() {
        let a = &n.attributes;
        if text(a, "node_kind") != "http_request" {
            continue;
        }
        let (method, path) = (text(a, "http_method"), text(a, "http_path"));
        let matches: Vec<_> = indexes
            .iter()
            .filter(|e| e.repo != repo)
            .flat_map(|e| &e.http_routes)
            .filter(|r| compatible(method, path, r))
            .collect();
        if matches.len() != 1 || matches[0].ambiguous {
            continue;
        }
        let Some(caller) = graph.node_index(&serde_json::json!(text(a, "http_caller"))) else {
            continue;
        };
        let route = matches[0];
        for target in std::iter::once(&route.node).chain(&route.related) {
            if !seen.insert((
                id(graph, caller),
                target.repo.clone(),
                target.node_id.clone(),
                path.to_string(),
            )) {
                continue;
            }
            out.push(CrossEdge {
                source: end(graph, caller, repo, episode),
                target: target.clone(),
                relation: RELATION.into(),
                confidence: INFERRED.into(),
                derivation: format!(
                    "http_route:{}:{}",
                    if origin(path).is_some() {
                        "origin_matched"
                    } else {
                        "host_unknown"
                    },
                    route.node.node_id
                ),
                module_path: format!("{method} {path}"),
                symbol: None,
                version_constraint: None,
                exporter_version: None,
                version_status: "not_applicable".into(),
                line: text(a, "source_location")
                    .trim_start_matches('L')
                    .split('-')
                    .next()
                    .and_then(|s| s.parse().ok()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> RouteExport {
        RouteExport {
            method: "DELETE".into(),
            path: "/nexus/v1/serviceconfig/:id".into(),
            ..Default::default()
        }
    }
    #[test]
    fn method_path_and_known_origins_are_independent_constraints() {
        let mut r = route();
        assert!(compatible(
            "DELETE",
            "/nexus/v1/serviceconfig/123?force=true",
            &r
        ));
        assert!(!compatible("GET", "/nexus/v1/serviceconfig/123", &r));
        assert!(!compatible(
            "DELETE",
            "https://external.example/nexus/v1/serviceconfig/123",
            &r
        ));
        r.origin = Some("https://nexus.example".into());
        assert!(compatible(
            "DELETE",
            "https://nexus.example:443/nexus/v1/serviceconfig/123",
            &r
        ));
        assert!(!compatible(
            "DELETE",
            "https://other.example/nexus/v1/serviceconfig/123",
            &r
        ));
        assert!(!compatible(
            "DELETE",
            "//nexus.example/nexus/v1/serviceconfig/123",
            &r
        ));
        assert!(!compatible("DELETE", "{}/nexus/v1/serviceconfig/123", &r));
    }
    #[test]
    fn ambiguous_routes_are_not_dropped_to_make_another_exporter_look_unique() {
        let graph = Graph::from_node_link(
            &serde_json::json!({"directed":true,"multigraph":false,"graph":{},
            "nodes":[{"id":"caller","source_file":"a.ts"},
                {"id":"request","node_kind":"http_request","http_method":"DELETE",
                 "http_path":"/nexus/v1/serviceconfig/123","http_caller":"caller"}],"links":[]}),
            "links",
        )
        .unwrap();
        let mut a = ExportIndex {
            repo: "a".into(),
            http_routes: vec![route()],
            ..Default::default()
        };
        a.http_routes[0].node.repo = "a".into();
        a.http_routes[0].node.node_id = "route-a".into();
        assert_eq!(
            link(&graph, "client", Some("current"), &[a.clone()]).len(),
            1
        );
        let mut b = a.clone();
        b.repo = "b".into();
        b.http_routes[0].ambiguous = true;
        assert!(link(&graph, "client", Some("current"), &[a, b.clone()]).is_empty());
        assert!(link(&graph, "client", Some("current"), &[b]).is_empty());
    }
    #[test]
    fn an_absolute_origin_can_disambiguate_but_a_relative_url_cannot() {
        let graph = |path| {
            Graph::from_node_link(
                &serde_json::json!({"directed":true,"multigraph":false,"graph":{},
            "nodes":[{"id":"caller","source_file":"a.ts"},
                {"id":"request","node_kind":"http_request","http_method":"DELETE",
                 "http_path":path,"http_caller":"caller"}],"links":[]}),
                "links",
            )
            .unwrap()
        };
        let indexes: Vec<_> = ["one", "two"]
            .iter()
            .map(|name| {
                let mut route = route();
                route.origin = Some(format!("https://{name}.example"));
                route.node.repo = name.to_string();
                route.node.node_id = "route".into();
                ExportIndex {
                    repo: name.to_string(),
                    http_routes: vec![route],
                    ..Default::default()
                }
            })
            .collect();
        assert!(
            link(
                &graph("/nexus/v1/serviceconfig/123"),
                "client",
                None,
                &indexes
            )
            .is_empty()
        );
        let found = link(
            &graph("https://one.example/nexus/v1/serviceconfig/123"),
            "client",
            None,
            &indexes,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].target.repo, "one");
    }
}
