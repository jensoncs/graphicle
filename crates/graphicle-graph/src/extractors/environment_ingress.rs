//! Ingress hosts of k8s-application chart values: which Service a host reaches.
use super::{INGRESS, document, line_starts, resource_name, span};
use serde_json::Value;

/// One backend an ingress host routes to, as the chart renders it.
pub struct IngressRoute {
    pub host: String,
    pub path: Option<String>,
    /// The Kubernetes Service: a path's `serviceName`, else `applicationName`;
    /// `serviceDuplication` paths reach `<applicationName>-dupe`.
    pub service: String,
    /// The `hosts[i]` entry declaring the host.
    pub span: std::ops::Range<usize>,
}
pub struct ChartIngress {
    pub application: String,
    pub namespace: String,
    pub routes: Vec<IngressRoute>,
}

/// The application a chart Service belongs to: `<app>-dupe` is the canary
/// copy and `shadow-<app>` the shadow copy of the same application.
pub fn application(service: &str) -> &str {
    let service = service.strip_prefix("shadow-").unwrap_or(service);
    service.strip_suffix("-dupe").unwrap_or(service)
}

/// A literal DNS host name with at least two labels.
pub fn host_name(value: &str) -> bool {
    resource_name(value)
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
        && !value.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// The ingress hosts k8s-application values declare (`ingress`, `ingressGrpc`;
/// k8s-application templates/ingress.yaml). Templated or invalid hosts are
/// left out; a document that is not chart values is an error.
pub fn chart_ingress(content: &str) -> Result<ChartIngress, &'static str> {
    let (document, guard) = document(content)?;
    let application = document["applicationName"]
        .as_str()
        .filter(|s| {
            resource_name(s)
                && document.get("apiVersion").is_none()
                && document.get("kind").is_none()
        })
        .ok_or("environment_chart_unsupported")?
        .to_owned();
    let namespace = document["namespace"]
        .as_str()
        .filter(|s| resource_name(s))
        .ok_or("environment_namespace_missing")?
        .to_owned();
    let duplication = &document["serviceDuplication"];
    let duplicated: &[Value] = match duplication["paths"].as_array() {
        Some(paths) if duplication["enabled"] == true => paths,
        _ => &[],
    };
    let offsets = line_starts(content);
    let mut routes = vec![];
    for key in INGRESS {
        let block = &document[key];
        if block["enabled"] != true {
            continue;
        }
        for (index, entry) in block["hosts"].as_array().into_iter().flatten().enumerate() {
            let Some(host) = entry["host"].as_str().filter(|h| host_name(h)) else {
                continue;
            };
            let path = [key.to_owned(), "hosts".into(), index.to_string()];
            let span = span(content, &offsets, &guard, &path)?;
            let own = entry["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| (p, application.clone()));
            let dupe = duplicated
                .iter()
                .map(|p| (p, format!("{application}-dupe")));
            for (route, default) in own.chain(dupe) {
                let service = route["serviceName"].as_str().map_or(default, str::to_owned);
                if resource_name(&service) {
                    routes.push(IngressRoute {
                        host: host.to_owned(),
                        path: route["path"].as_str().map(str::to_owned),
                        service,
                        span: span.clone(),
                    });
                }
            }
        }
    }
    Ok(ChartIngress {
        application,
        namespace,
        routes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const VALUES: &str = "applicationName: agent\nnamespace: siren\nserviceDuplication:\n  enabled: true\n  paths:\n    - path: /v1/canary\ningress:\n  enabled: true\n  hosts:\n    - host: agent.company.internal\n      paths:\n        - path: /*\n        - path: /v1/batch\n          serviceName: agent-go\n    - host: \"{{ .Values.host }}\"\n      paths:\n        - path: /*\ningressGrpc:\n  enabled: false\n  hosts:\n    - host: grpc.company.internal\n      paths:\n        - path: /*\n";

    #[test]
    fn routes_follow_the_chart_template() {
        let parsed = chart_ingress(VALUES).unwrap();
        assert_eq!(
            (parsed.application.as_str(), parsed.namespace.as_str()),
            ("agent", "siren")
        );
        let routes: Vec<_> = parsed
            .routes
            .iter()
            .map(|r| (r.host.as_str(), r.path.as_deref(), r.service.as_str()))
            .collect();
        assert_eq!(
            routes,
            [
                ("agent.company.internal", Some("/*"), "agent"),
                ("agent.company.internal", Some("/v1/batch"), "agent-go"),
                ("agent.company.internal", Some("/v1/canary"), "agent-dupe"),
            ]
        );
        let cited = &VALUES[parsed.routes[0].span.clone()];
        assert!(
            cited.starts_with("host: agent.company.internal"),
            "{cited:?}"
        );
        assert!(!cited.contains("{{"), "{cited:?}");
        assert_eq!(application("agent-dupe"), "agent");
        assert_eq!(application("shadow-agent"), "agent");
    }

    #[test]
    fn other_documents_and_hosts_are_not_routes() {
        assert!(chart_ingress("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n").is_err());
        assert!(chart_ingress("applicationName: agent\n").is_err());
        for host in ["localhost", "10.0.0.1", "-a.b", "a..b", "A.b"] {
            assert!(!host_name(host), "{host}");
        }
        assert!(host_name("agent.sharechat.internal"));
    }
}
