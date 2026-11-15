//! Hosts a deployment serves and hosts a file calls.
//!
//! An Argus deployment's `infra/<env>/values.yaml` lists the ingress hosts its
//! application answers on: `deploy_hosts` on that file's node. Every file's
//! `http(s)://host` literals outside comment lines are hosts it is coded or
//! configured to call: `url_hosts`, each `[host, first line]`. Both depend on
//! the file alone, so an incremental run that reads only changed files keeps
//! them right; matching a caller's host to the deployment serving it happens
//! when projects are linked.
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use yaml_rust2::{Yaml, YamlLoader};

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://([A-Za-z0-9][A-Za-z0-9.-]*)").unwrap());

/// Prose: a URL in documentation is not a call.
const DOCS: &[&str] = &["md", "mdx", "rst", "txt", "adoc", "html", "htm"];

/// Folders whose values files are samples or a chart's defaults, not a deployment.
const NOT_DEPLOYED: &[&str] = &[
    "examples",
    "example",
    "test",
    "tests",
    "testdata",
    "fixtures",
    "charts",
    "templates",
];

/// A deployment's Helm values, `<dir>/<env>/values[-variant].yaml` (or `.yml`):
/// the Argus layout `<app>/infra/<env>/values.yaml` and its variants
/// (`values-go.yaml`, `sharechat/<env>/`, `manifest/<env>/`), never under a
/// sample, test or chart folder. Its ingress hosts are what it serves.
pub fn is_deploy_values(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let name = parts[parts.len() - 1];
    let stem = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"));
    parts.len() >= 3
        && stem.is_some_and(|s| s == "values" || s.starts_with("values-"))
        && !parts[..parts.len() - 1]
            .iter()
            .any(|d| NOT_DEPLOYED.contains(d))
}

fn comment(line: &str) -> bool {
    let t = line.trim_start();
    ["//", "#", "*", "/*", "--", "<!--"]
        .iter()
        .any(|m| t.starts_with(m))
}

/// Distinct hosts named by `http(s)://` literals, each with its first line.
pub fn url_hosts(text: &str) -> Vec<(String, usize)> {
    let mut seen = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if comment(line) {
            continue;
        }
        for m in URL.captures_iter(line) {
            let host = m[1].trim_end_matches(['.', '-']).to_ascii_lowercase();
            if host.contains('.') {
                seen.entry(host).or_insert(i + 1);
            }
        }
    }
    let mut out: Vec<_> = seen.into_iter().collect();
    out.sort_by_key(|(_, line)| *line);
    out
}

/// The ingress hosts of a deployment values file: every `host`/`hosts` string
/// under `ingress`, without templated or wildcard names.
pub fn deploy_hosts(text: &str) -> Vec<String> {
    fn walk(y: &Yaml, under_host: bool, out: &mut Vec<String>) {
        match y {
            Yaml::Hash(h) => {
                for (k, v) in h {
                    let host = matches!(k.as_str(), Some("host" | "hosts"));
                    walk(v, host, out);
                }
            }
            Yaml::Array(a) => a.iter().for_each(|v| walk(v, under_host, out)),
            Yaml::String(s)
                if under_host
                    && !s.contains(['*', '{', ' '])
                    && s.contains('.')
                    && !s.ends_with(".local")
                    && !s.contains("example") =>
            {
                out.push(s.to_ascii_lowercase())
            }
            _ => {}
        }
    }
    let Ok(docs) = YamlLoader::load_from_str(text) else {
        return vec![];
    };
    let mut out = vec![];
    if let Some(doc) = docs.first() {
        walk(&doc["ingress"], false, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Set `deploy_hosts` and `url_hosts` on the file node of each of `paths`.
pub fn annotate(paths: &[PathBuf], nodes: &mut Vec<Value>, root: &Path) {
    // Relative path -> (path as given, hosts served, hosts called).
    type Facts<'a> = (&'a PathBuf, Vec<String>, Vec<(String, usize)>);
    let mut facts: BTreeMap<String, Facts> = BTreeMap::new();
    for path in paths {
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DOCS.contains(&e))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = super::resolution::source_key(&path.to_string_lossy(), root);
        let served = if is_deploy_values(&rel) {
            deploy_hosts(&text)
        } else {
            vec![]
        };
        let called = url_hosts(&text);
        if !served.is_empty() || !called.is_empty() {
            facts.insert(rel, (path, served, called));
        }
    }
    if facts.is_empty() {
        return;
    }
    let mut annotated = std::collections::BTreeSet::new();
    for node in nodes.iter_mut() {
        let (label, source) = (
            node.get("label").and_then(Value::as_str).unwrap_or(""),
            node.get("source_file")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        if !crate::build_labels::is_file_node_label_str(label, source) {
            continue;
        }
        let rel = super::resolution::source_key(source, root);
        let Some((_, served, called)) = facts.get(&rel) else {
            continue;
        };
        set(node, served, called);
        annotated.insert(rel);
    }
    // A file no extractor reads (most JSON configuration) gets a file node
    // carrying just its hosts.
    for (rel, (path, served, called)) in &facts {
        if annotated.contains(rel) {
            continue;
        }
        let source = path.to_string_lossy();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let mut node = json!({"id": crate::extract_merge::file_node_id(path), "label": name,
            "file_type": "code", "source_file": source, "source_location": "L1", "_origin": "ast"});
        set(&mut node, served, called);
        nodes.push(node);
    }
}

fn set(node: &mut Value, served: &[String], called: &[(String, usize)]) {
    if !served.is_empty() {
        node["deploy_hosts"] = json!(served);
    }
    if !called.is_empty() {
        node["url_hosts"] = json!(called);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_layout_and_ingress_hosts() {
        assert!(is_deploy_values("infra/production/values.yaml"));
        assert!(is_deploy_values("alert-review/infra/staging/values.yml"));
        assert!(is_deploy_values(
            "deployment/manifest/production/values.yaml"
        ));
        assert!(is_deploy_values("infra/production/values-go.yaml"));
        assert!(is_deploy_values("app/sharechat/staging/values.yaml"));
        assert!(!is_deploy_values("infra/values.yaml"));
        assert!(!is_deploy_values("charts/app/values.yaml"));
        assert!(!is_deploy_values("examples/production/values.yaml"));
        assert!(!is_deploy_values("infra/production/valuesfile.yaml"));
        assert!(
            deploy_hosts("ingress:\n  hosts: [chart-example.local, app.example.com]\n").is_empty()
        );
        let values = "applicationName: alert-review-agent\ningress:\n  enabled: true\n  hosts:\n    - Alert-Review-Agent.sharechat.internal\n    - '*.sharechat.internal'\n  tls:\n    - hosts: [alert-review-agent.sharechat.internal]\nservice:\n  host: not-ingress.internal\n";
        assert_eq!(
            deploy_hosts(values),
            vec!["alert-review-agent.sharechat.internal"]
        );
        let rules = "ingress:\n  rules:\n    - host: siren.sharechat.internal\n      paths: [/]\n";
        assert_eq!(deploy_hosts(rules), vec!["siren.sharechat.internal"]);
        assert!(deploy_hosts("ingress: [unclosed").is_empty());
    }

    #[test]
    fn url_hosts_skip_comment_lines_and_keep_first_line() {
        let text = "# http://commented.sharechat.internal/x\nbase := \"http://siren.sharechat.internal\" // http://trailing.example\nurl: https://SIREN.sharechat.internal/siren/v1\n  // see http://doc.example\nlocal = \"http://localhost:8080\"\n";
        assert_eq!(
            url_hosts(text),
            vec![
                ("siren.sharechat.internal".to_string(), 2),
                ("trailing.example".to_string(), 2)
            ]
        );
    }
}
