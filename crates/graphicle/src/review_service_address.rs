//! Which application an environment value's URL addresses.
//!
//! A host resolves from k8s-application chart values in an admitted pin
//! (`ingress.hosts`, cited at the host entry), else from an admitted host map
//! (`graphicle-host-map.json`, `scripts/host_map.py`, rebuilt daily from
//! weaver and local clones). Several applications behind one host, or two
//! maps, resolve nothing. Neither source proves runtime routing.
use super::*;
use graphicle::extractors::environment as extract;

pub(super) const HOST_MAP: &str = "graphicle-host-map.json";
const HOST_MAP_FORMAT: &str = "graphicle.host-map.v1";
const MAX_HOST_MAP_BYTES: usize = 8 * 1024 * 1024;

struct Route {
    application: String,
    namespace: Option<String>,
    /// `Input::sources` key of the cited file and the cited bytes.
    file: String,
    span: std::ops::Range<usize>,
    source: &'static str,
    observed: Option<bool>,
}

#[derive(Default)]
pub(super) struct Hosts {
    chart: BTreeMap<String, Vec<Route>>,
    map: BTreeMap<String, Vec<Route>>,
    /// Host suffixes the admitted map claims to cover.
    suffixes: Vec<String>,
}

fn chart_values(path: &str) -> bool {
    let path = Path::new(path);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("values")
        && (name.ends_with(".yaml") || name.ends_with(".yml"))
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "infra")
}

/// The JSON key of `host` in the map, exactly once.
/// ponytail: a text search over the writer's `json.dumps` output, not a
/// position-aware parse; a host spelled twice cites nothing.
fn host_key(text: &str, host: &str) -> Option<std::ops::Range<usize>> {
    let key = format!("{}:", serde_json::to_string(host).ok()?);
    let mut found = text.match_indices(&key);
    let (start, _) = found.next()?;
    found.next().is_none().then(|| start..start + key.len() - 1)
}

fn map_routes(hosts: &mut Hosts, file: &str, text: &str) -> std::result::Result<(), &'static str> {
    let document: Value = serde_json::from_str(text).map_err(|_| "environment_host_map_invalid")?;
    if document["format"] != HOST_MAP_FORMAT || !document["hosts"].is_object() {
        return Err("environment_host_map_invalid");
    }
    hosts.suffixes = document["suffixes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| s.starts_with('.') && extract::host_name(&s[1..]))
        .map(str::to_owned)
        .collect();
    for (host, entry) in document["hosts"].as_object().unwrap() {
        if !extract::host_name(host) {
            continue;
        }
        let Some(span) = host_key(text, host) else {
            continue;
        };
        let observed: BTreeSet<&str> = entry["observed_applications"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let namespace = entry["routes"]
            .as_array()
            .into_iter()
            .flatten()
            .find_map(|r| r["namespace"].as_str())
            .map(str::to_owned);
        for application in entry["applications"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            hosts.map.entry(host.clone()).or_default().push(Route {
                application: application.to_owned(),
                namespace: namespace.clone(),
                file: file.to_owned(),
                span: span.clone(),
                source: "host_map",
                observed: Some(observed.contains(application)),
            });
        }
    }
    Ok(())
}

pub(super) fn index(input: &Input, issues: &mut Vec<Value>) -> Hosts {
    let mut hosts = Hosts::default();
    let admitted = |path: &str| {
        input
            .pins
            .iter()
            .any(|pin| path.starts_with(&format!("{}/", pin.directory)))
    };
    let mut maps = vec![];
    for (path, source) in input.sources.iter().filter(|(path, _)| admitted(path)) {
        let Some(text) = source.content.text() else {
            continue;
        };
        if path.ends_with(&format!("/{HOST_MAP}")) {
            maps.push((path, text));
        } else if chart_values(path)
            && let Ok(chart) = extract::chart_ingress(text)
        {
            for route in chart.routes {
                hosts.chart.entry(route.host).or_default().push(Route {
                    application: extract::application(&route.service).to_owned(),
                    namespace: Some(chart.namespace.clone()),
                    file: path.clone(),
                    span: route.span,
                    source: "chart_values",
                    observed: None,
                });
            }
        }
    }
    match maps.as_slice() {
        [] => {}
        [(file, text)] if text.len() <= MAX_HOST_MAP_BYTES => {
            if let Err(reason) = map_routes(&mut hosts, file, text) {
                hosts.map.clear();
                hosts.suffixes.clear();
                issues.push(issue(reason, json!({"file":file})));
            }
        }
        _ => issues.push(issue(
            "environment_host_map_ambiguous",
            json!({"files":maps.iter().map(|(f, _)| f).collect::<Vec<_>>()}),
        )),
    }
    hosts
}

/// The host of a URL-shaped value (`http://h/...`, `h:80`, `grpc://h`).
fn url_host(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2048 || value.contains(char::is_whitespace) {
        return None;
    }
    let url = if value.contains("://") {
        url::Url::parse(value).ok()?
    } else {
        url::Url::parse(&format!("http://{value}")).ok()?
    };
    matches!(url.scheme(), "http" | "https" | "grpc" | "grpcs")
        .then(|| url.host_str().map(str::to_ascii_lowercase))
        .flatten()
        .filter(|host| extract::host_name(host))
}

/// One cited address: an env input's value names a host that resolves to
/// exactly one application.
pub(super) struct Address<'a> {
    pub input: &'a str,
    pub world: &'a str,
    pub key: &'a str,
    pub value: &'a str,
    pub scope: &'a str,
}

pub(super) fn link(
    input: &Input,
    hosts: &Hosts,
    a: &mut Analysis,
    links: &mut services::Links,
    at: Address,
) {
    let Some(host) = url_host(at.value) else {
        return;
    };
    let detail = json!({"scope":at.scope,"key":at.key,"host":host});
    let Some(routes) = hosts.chart.get(&host).or_else(|| hosts.map.get(&host)) else {
        if hosts
            .suffixes
            .iter()
            .any(|suffix| host.ends_with(suffix.as_str()))
        {
            links
                .issues
                .push(issue("environment_host_unmapped", detail));
        }
        return;
    };
    let applications: BTreeSet<&str> = routes.iter().map(|r| r.application.as_str()).collect();
    if applications.len() != 1 {
        let mut detail = detail;
        detail["applications"] = json!(applications);
        links
            .issues
            .push(issue("environment_host_ambiguous", detail));
        return;
    }
    let route = routes
        .iter()
        .min_by_key(|r| (&r.file, r.span.start))
        .unwrap();
    let source = &input.sources[&route.file];
    let content = source.content.text().unwrap();
    let pin = input
        .pins
        .iter()
        .find(|p| route.file.starts_with(&format!("{}/", p.directory)))
        .unwrap();
    let id = crate::digest(&json!(["service-address-v1", host]));
    let cited = &content[route.span.clone()];
    let provenance = json!({"host":host,"application":route.application,"namespace":route.namespace,
        "source_kind":route.source,"observed_in_mesh":route.observed,"runtime_observed":false,
        "source":{"repository_id":source.repository,"file":source.path,"file_sha256":source.sha,
            "byte_start":route.span.start,"byte_end":route.span.end,
            "commit":pin.commit,"snapshot":pin.snapshot,"policy_sha256":pin.policy_sha256}});
    if !a.symbols.contains_key(&id) {
        let line = |at: usize| content[..at].bytes().filter(|b| *b == b'\n').count() + 1;
        a.symbols.insert(
            id.clone(),
            (
                at.world.to_owned(),
                Symbol {
                    key: id.clone(),
                    file: route.file.clone(),
                    package: route.file.clone(),
                    name: format!("{}@{host}", route.application),
                    kind: "service_address".into(),
                    parent: String::new(),
                    start: route.span.start,
                    end: route.span.end,
                    body_start: route.span.start,
                    line: line(route.span.start),
                    end_line: line(route.span.end),
                    revision: crate::digest(&json!([
                        host,
                        route.application,
                        route.namespace,
                        cited
                    ])),
                    signature: crate::digest(&json!([host, route.application])),
                    implementation: text::sha256(cited.as_bytes()),
                    correspondence: id.clone(),
                },
            ),
        );
        links.nodes.insert(id.clone(), provenance.clone());
    }
    links.edges.push(services::Edge {
        from: at.input.to_owned(),
        to: id,
        relation: "addresses_service",
        evidence: json!({"evidence":"configured_static","key":at.key,"host":host,"address":provenance}),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_hosts_are_read_from_url_shaped_values_only() {
        for (value, host) in [
            (
                "http://agent.company.internal/v1/review",
                Some("agent.company.internal"),
            ),
            (
                "https://Agent.Company.Internal:8443",
                Some("agent.company.internal"),
            ),
            ("agent.company.internal:80", Some("agent.company.internal")),
            (
                "grpc://rank.company.internal:9000",
                Some("rank.company.internal"),
            ),
            ("http://localhost:8080", None),
            ("http://10.0.0.1/x", None),
            ("redis://cache.company.internal:6379", None),
            ("10", None),
            ("a b.c", None),
        ] {
            assert_eq!(url_host(value).as_deref(), host, "{value}");
        }
    }

    #[test]
    fn a_host_key_is_cited_only_when_unique() {
        let text = "{\n \"hosts\": {\n  \"a.b.internal\": {}\n }\n}\n";
        let span = host_key(text, "a.b.internal").unwrap();
        assert_eq!(&text[span], "\"a.b.internal\"");
        assert!(host_key(&format!("{text}\"a.b.internal\": 1"), "a.b.internal").is_none());
        assert!(chart_values("app/infra/production/values.yaml"));
        assert!(chart_values("app/svc/infra/staging/values-mum.yml"));
        assert!(!chart_values("app/docs/values.yaml"));
    }
}
