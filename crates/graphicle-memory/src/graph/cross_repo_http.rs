//! Conservative HTTP linkage over route/request facts from the existing AST
//! extractor. A relative URL with one workspace route is an inference, not a
//! claim about a deployed host. Absolute URLs require a matching known origin.
use super::cross_repo_imports::{CrossEdge, EdgeEnd, ExportIndex, INFERRED, is_test_path};
use graphicle::{graph::Graph, record::Properties, serve_search::id};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// A host a deployment answers on, from its values file (`deploy_hosts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExport {
    pub host: String,
    /// The deployment's values file.
    pub node: EdgeEnd,
}

fn strings(a: &impl Properties, name: &str) -> Vec<Value> {
    a.get(name)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The ingress hosts this graph's deployments serve; test fixtures excluded.
pub fn host_exports(graph: &Graph, repo: &str, episode: Option<&str>) -> Vec<HostExport> {
    let mut out = Vec::new();
    for (i, n) in graph.nodes().iter().enumerate() {
        let hosts = strings(&n.attributes, "deploy_hosts");
        if hosts.is_empty() {
            continue;
        }
        let node = end(graph, i, repo, episode);
        if node.relpath.as_deref().is_some_and(is_test_path) {
            continue;
        }
        for host in hosts.iter().filter_map(Value::as_str) {
            out.push(HostExport {
                host: host.to_string(),
                node: node.clone(),
            });
        }
    }
    out
}

/// `(file node index, host, line)` of every URL host a non-test file names.
pub fn called_hosts(graph: &Graph) -> Vec<(usize, String, Option<usize>)> {
    let mut out = Vec::new();
    for (i, n) in graph.nodes().iter().enumerate() {
        if is_test_path(text(&n.attributes, "source_file")) {
            continue;
        }
        for pair in strings(&n.attributes, "url_hosts") {
            if let Some(host) = pair.get(0).and_then(Value::as_str) {
                let line = pair.get(1).and_then(Value::as_u64).map(|l| l as usize);
                out.push((i, host.to_string(), line));
            }
        }
    }
    out
}

/// Calls by host: a file naming a host that one other project serves links to
/// each of its deployments serving it (two during a migration from one to the
/// other). A host this project serves itself, or several projects serve,
/// links nowhere.
fn host_links(
    graph: &Graph,
    repo: &str,
    episode: Option<&str>,
    indexes: &[&ExportIndex],
) -> Vec<CrossEdge> {
    let own: BTreeSet<String> = host_exports(graph, repo, episode)
        .into_iter()
        .map(|h| h.host)
        .collect();
    let mut out = Vec::new();
    for (file, host, line) in called_hosts(graph) {
        if own.contains(&host) {
            continue;
        }
        let owners: Vec<&HostExport> = indexes
            .iter()
            .filter(|e| e.repo != repo)
            .flat_map(|e| &e.app_hosts)
            .filter(|h| h.host == host)
            .collect();
        let repos: BTreeSet<&str> = owners.iter().map(|h| h.node.repo.as_str()).collect();
        if repos.len() != 1 {
            continue;
        }
        for owner in owners {
            out.push(CrossEdge {
                source: end(graph, file, repo, episode),
                target: owner.node.clone(),
                relation: RELATION.into(),
                confidence: INFERRED.into(),
                derivation: format!("ingress_host:{host}"),
                module_path: host.clone(),
                symbol: None,
                version_constraint: None,
                exporter_version: None,
                version_status: "not_applicable".into(),
                line,
            });
        }
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

pub(crate) fn compatible(method: &str, request: &str, route: &RouteExport) -> bool {
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
    indexes: &[impl std::borrow::Borrow<ExportIndex>],
) -> Vec<CrossEdge> {
    let indexes: Vec<&ExportIndex> = indexes.iter().map(std::borrow::Borrow::borrow).collect();
    let mut out = host_links(graph, repo, episode, &indexes);
    let mut seen = BTreeSet::new();
    for n in graph.nodes() {
        let a = &n.attributes;
        if text(a, "node_kind") != "http_request" || is_test_path(text(a, "source_file")) {
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
    fn hosts(name: &str, served: &[&str]) -> ExportIndex {
        ExportIndex {
            repo: name.into(),
            app_hosts: served
                .iter()
                .map(|h| HostExport {
                    host: h.to_string(),
                    node: EdgeEnd {
                        repo: name.into(),
                        node_id: format!("{name}_values"),
                        ..Default::default()
                    },
                })
                .collect(),
            ..Default::default()
        }
    }
    /// A config file naming `siren.sharechat.internal` links to the one
    /// deployment serving it; a host two deployments serve, the caller's own
    /// host, and a URL in a test file link nowhere.
    #[test]
    fn a_called_host_links_to_the_one_deployment_serving_it() {
        let graph = Graph::from_node_link(
            &serde_json::json!({"directed":true,"multigraph":false,"graph":{},
            "nodes":[
                {"id":"cfg","label":"prod.json","source_file":"config/prod.json",
                 "url_hosts":[["siren.sharechat.internal",3],["shared.sharechat.internal",4],["agent.sharechat.internal",5]]},
                {"id":"values","label":"values.yaml","source_file":"infra/production/values.yaml",
                 "deploy_hosts":["agent.sharechat.internal"]},
                {"id":"fixture","label":"client_test.go","source_file":"client_test.go",
                 "url_hosts":[["siren.sharechat.internal",1]]}],"links":[]}),
            "links",
        )
        .unwrap();
        let indexes = [
            hosts(
                "siren",
                &["siren.sharechat.internal", "shared.sharechat.internal"],
            ),
            hosts("copy", &["shared.sharechat.internal"]),
            hosts("agent", &["agent.sharechat.internal"]),
        ];
        let found = link(&graph, "agent", Some("e1"), &indexes);
        assert_eq!(found.len(), 1, "{found:#?}");
        // A second deployment of the same project serving the host (a migration): a link to each.
        let mut migrating = indexes.clone();
        let mut second = migrating[0].app_hosts[0].clone();
        second.node.node_id = "siren_node_values".into();
        migrating[0].app_hosts.push(second);
        let both = link(&graph, "agent", Some("e1"), &migrating);
        assert_eq!(
            both.iter()
                .map(|e| e.target.node_id.as_str())
                .collect::<Vec<_>>(),
            ["siren_values", "siren_node_values"]
        );
        let edge = &found[0];
        assert_eq!(
            (edge.source.node_id.as_str(), edge.target.repo.as_str()),
            ("cfg", "siren")
        );
        assert_eq!(edge.target.node_id, "siren_values");
        assert_eq!(edge.derivation, "ingress_host:siren.sharechat.internal");
        assert_eq!(edge.line, Some(3));
        let interest = super::super::cross_repo_imports::link_interest(&graph);
        assert!(interest.hosts_fit(&indexes[0]));
        assert!(!interest.hosts_fit(&hosts("other", &["other.sharechat.internal"])));
    }
}
