//! YAML and JSON configuration: a Kubernetes object is a declaration and names
//! the objects it mounts, scales or runs as; a kustomization names its
//! resources and patches; a JSON schema names the files it `$ref`s. Other
//! configuration stays file level.
use super::basic_refs::{Decl, Facts, Ref, To};
use serde_json::Value;

const KUSTOMIZATION: &[&str] = &["kustomization.yaml", "kustomization.yml", "Kustomization"];

/// Each document of a YAML stream: its byte range.
fn documents(content: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = 0;
    let mut at = 0;
    for line in content.split_inclusive('\n') {
        let marker = line.trim_end();
        if marker == "---" || marker.starts_with("--- ") || marker == "..." {
            out.push((start, at));
            start = at + line.len();
        }
        at += line.len();
    }
    out.push((start, content.len()));
    out.into_iter()
        .filter(|(s, e)| {
            content[*s..*e].lines().any(|l| {
                let l = l.trim();
                !l.is_empty() && !l.starts_with('#')
            })
        })
        .collect()
}

pub(super) fn yaml(path: &str, content: &str) -> Facts {
    let name = path.rsplit('/').next().unwrap_or(path);
    if KUSTOMIZATION.contains(&name) {
        return kustomization(content);
    }
    let mut f = Facts::default();
    for (start, end) in documents(content) {
        let text = &content[start..end];
        let Ok(doc) = graphicle::extractors::yaml_frontmatter::parse(text) else {
            continue;
        };
        let (Some(kind), Some(name)) = (doc["kind"].as_str(), doc["metadata"]["name"].as_str())
        else {
            continue;
        };
        let header = text.find('\n').map_or(end, |n| start + n);
        let namespace = doc["metadata"]["namespace"].as_str();
        if let Some(ns) = namespace {
            f.namespaces.insert(format!("{kind}/{name}"), ns.to_owned());
        }
        f.decls.push(Decl {
            name: format!("{kind}/{name}"),
            kind: "kubernetes_object",
            start,
            body_start: start,
            end: start + text.trim_end().len(),
        });
        let mut named = vec![];
        object_refs(&doc, namespace, &mut named, 0);
        for (target, namespace) in named {
            // The first place the name is written; the span is a citation.
            let written = target.split_once('/').map_or("", |(_, n)| n);
            let (s, e) = text
                .find(&format!(" {written}"))
                .map_or((start, header), |i| {
                    (start + i + 1, start + i + 1 + written.len())
                });
            f.refs.push(Ref {
                to: To::Decl {
                    name: target,
                    wide: true,
                    namespace,
                },
                start: s,
                end: e,
                relation: "uses_value",
                evidence: "kubernetes_object_name",
            });
        }
    }
    f
}

/// The `Kind/name` of each object this one, in `namespace`, names by a
/// reference field, and the namespace that object is in when known.
fn object_refs(
    value: &Value,
    namespace: Option<&str>,
    out: &mut Vec<(String, Option<String>)>,
    depth: usize,
) {
    if depth > 64 {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                let named = |field: &str| v[field].as_str().filter(|n| !n.contains("{{"));
                let here = |target: String| (target, namespace.map(str::to_owned));
                let target = match key.as_str() {
                    "configMapRef" | "configMapKeyRef" | "configMap" => {
                        named("name").map(|n| here(format!("ConfigMap/{n}")))
                    }
                    "secretRef" | "secretKeyRef" => {
                        named("name").map(|n| here(format!("Secret/{n}")))
                    }
                    "secret" => named("secretName").map(|n| here(format!("Secret/{n}"))),
                    "persistentVolumeClaim" => {
                        named("claimName").map(|n| here(format!("PersistentVolumeClaim/{n}")))
                    }
                    "serviceAccountName" => v.as_str().map(|n| here(format!("ServiceAccount/{n}"))),
                    "scaleTargetRef" | "targetRef" | "roleRef" => {
                        named("kind").zip(named("name")).map(|(k, n)| {
                            // A cluster-scoped object is in no namespace.
                            let (target, ns) = here(format!("{k}/{n}"));
                            (target, ns.filter(|_| !k.starts_with("Cluster")))
                        })
                    }
                    // An Ingress or webhook backend.
                    "service" if v.get("port").is_some() => named("name").map(|n| {
                        let ns = named("namespace").or(namespace);
                        (format!("Service/{n}"), ns.map(str::to_owned))
                    }),
                    // An extensions/v1beta1 or networking.k8s.io/v1beta1 Ingress backend.
                    "serviceName" if map.contains_key("servicePort") => v
                        .as_str()
                        .filter(|n| !n.contains("{{"))
                        .map(|n| here(format!("Service/{n}"))),
                    _ => None,
                };
                // A binding's service accounts, in the namespace each states.
                let subjects = v
                    .as_array()
                    .filter(|_| key == "subjects")
                    .into_iter()
                    .flatten()
                    .filter(|s| s["kind"] == "ServiceAccount")
                    .filter_map(|s| {
                        let name = s["name"].as_str().filter(|n| !n.contains("{{"))?;
                        let ns = s["namespace"].as_str().map(str::to_owned);
                        Some((format!("ServiceAccount/{name}"), ns))
                    });
                for target in target.into_iter().chain(subjects) {
                    if !out.contains(&target) {
                        out.push(target);
                    }
                }
                object_refs(v, namespace, out, depth + 1);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|v| object_refs(v, namespace, out, depth + 1)),
        _ => {}
    }
}

fn kustomization(content: &str) -> Facts {
    let mut f = Facts::default();
    let Ok(doc) = graphicle::extractors::yaml_frontmatter::parse(content) else {
        return f;
    };
    let mut paths: Vec<&str> = vec![];
    for key in [
        "resources",
        "bases",
        "components",
        "crds",
        "patchesStrategicMerge",
        "generators",
        "transformers",
        "validators",
        "configurations",
    ] {
        paths.extend(
            doc[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
    }
    for key in ["patches", "patchesJson6902", "replacements"] {
        paths.extend(
            doc[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().or_else(|| p["path"].as_str())),
        );
    }
    for key in ["configMapGenerator", "secretGenerator"] {
        for generator in doc[key].as_array().into_iter().flatten() {
            let files = generator["files"].as_array().into_iter().flatten();
            let envs = generator["envs"].as_array().into_iter().flatten();
            paths.extend(
                files
                    .chain(envs)
                    .chain(std::iter::once(&generator["env"]))
                    .filter_map(Value::as_str)
                    // `key=path` names the file under another key.
                    .map(|p| p.rsplit_once('=').map_or(p, |(_, file)| file)),
            );
        }
    }
    let mut cursor = 0;
    for literal in paths {
        // A remote base is fetched, not in the snapshot.
        let remote = literal.contains("://")
            || literal.contains("?ref=")
            || literal.starts_with("github.com/")
            || literal.starts_with("git@");
        if remote || literal.contains('\n') {
            continue;
        }
        let at = content[cursor..]
            .find(literal)
            .map(|i| cursor + i)
            .or_else(|| content.find(literal))
            .unwrap_or(0);
        cursor = at;
        f.refs.push(Ref {
            to: To::Path {
                literal: literal.into(),
                index: KUSTOMIZATION,
                required: true,
                any_file: false,
            },
            start: at,
            end: at + literal.len(),
            relation: "potential_import",
            evidence: "kustomize_path",
        });
    }
    f
}

/// A JSON schema's relative `$ref` files.
pub(super) fn json(content: &str) -> Facts {
    let mut f = Facts::default();
    for (at, _) in content.match_indices("\"$ref\"") {
        let rest = &content[at + 6..];
        let Some(value) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(value) = value.trim_start().strip_prefix('"') else {
            continue;
        };
        let file = value.split(['"', '#']).next().unwrap_or("");
        if file.is_empty() || file.contains("://") || file.contains('\\') {
            continue;
        }
        let start = value.as_ptr() as usize - content.as_ptr() as usize;
        f.path(file, start, "json_schema_ref");
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn targets(f: &Facts) -> Vec<String> {
        f.refs
            .iter()
            .map(|r| match &r.to {
                To::Path { literal, .. } => literal.clone(),
                To::Decl { name, .. } => format!("#{name}"),
                To::Suffix(n) | To::Style(n) => n.join("|"),
                To::Include { name, .. } => format!("<{name}>"),
            })
            .collect()
    }

    #[test]
    fn kubernetes_documents_declare_objects_and_name_what_they_use() {
        let text = "# lead\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      serviceAccountName: runner\n      volumes:\n        - name: c\n          configMap:\n            name: api-config\n        - name: s\n          secret:\n            secretName: creds\n---\nkind: HorizontalPodAutoscaler\nmetadata:\n  name: api\nspec:\n  scaleTargetRef:\n    kind: Deployment\n    name: api\n";
        let f = yaml("repo/k/app.yaml", text);
        let decls: Vec<_> = f.decls.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(decls, ["Deployment/api", "HorizontalPodAutoscaler/api"]);
        assert!(text[f.decls[0].start..f.decls[0].end].starts_with("apiVersion"));
        assert!(text[f.decls[0].start..f.decls[0].end].ends_with("creds"));
        assert_eq!(
            targets(&f),
            [
                "#ServiceAccount/runner",
                "#ConfigMap/api-config",
                "#Secret/creds",
                "#Deployment/api"
            ]
        );
        assert_eq!(&text[f.refs[1].start..f.refs[1].end], "api-config");
    }

    #[test]
    fn bindings_name_service_accounts_and_old_ingresses_name_services_in_a_namespace() {
        let text = "kind: RoleBinding\nmetadata:\n  name: r\n  namespace: a\nroleRef:\n  kind: Role\n  name: reader\nsubjects:\n  - kind: ServiceAccount\n    name: bot\n    namespace: b\n  - kind: User\n    name: jane\n  - kind: ServiceAccount\n    name: own\n---\nkind: ClusterRoleBinding\nmetadata:\n  name: c\nroleRef:\n  kind: ClusterRole\n  name: view\n---\napiVersion: extensions/v1beta1\nkind: Ingress\nmetadata:\n  name: web\n  namespace: w\nspec:\n  backend:\n    serviceName: fallback\n    servicePort: 80\n  rules:\n    - http:\n        paths:\n          - backend:\n              serviceName: web\n              servicePort: http\n---\nkind: StatefulSet\nmetadata:\n  name: db\nspec:\n  serviceName: db\n";
        let f = yaml("repo/k/rbac.yaml", text);
        let wanted: BTreeSet<_> = f
            .refs
            .iter()
            .map(|r| match &r.to {
                To::Decl {
                    name, namespace, ..
                } => (name.as_str(), namespace.as_deref()),
                _ => ("", None),
            })
            .collect();
        let expected: BTreeSet<_> = [
            ("Role/reader", Some("a")),
            ("ServiceAccount/bot", Some("b")),
            ("ServiceAccount/own", None),
            ("ClusterRole/view", None),
            ("Service/fallback", Some("w")),
            ("Service/web", Some("w")),
        ]
        .into_iter()
        .collect();
        assert_eq!(wanted, expected);
        assert_eq!(f.namespaces["RoleBinding/r"], "a");
        assert!(!f.namespaces.contains_key("ClusterRoleBinding/c"));
    }

    #[test]
    fn kustomizations_name_local_resources_patches_and_generator_files() {
        let text = "resources:\n  - deploy.yaml\n  - ../base\n  - https://x.io/a.yaml\n  - github.com/o/r//b?ref=v1\npatches:\n  - path: p.yaml\n  - patch: |-\n      - op: add\nconfigMapGenerator:\n  - name: c\n    files:\n      - app.properties\n      - key=conf/b.txt\n    envs:\n      - .env\n";
        let f = yaml("repo/o/kustomization.yaml", text);
        assert_eq!(
            targets(&f),
            [
                "deploy.yaml",
                "../base",
                "p.yaml",
                "app.properties",
                "conf/b.txt",
                ".env"
            ]
        );
        assert!(
            f.refs
                .iter()
                .all(|r| matches!(r.to, To::Path { required: true, .. }))
        );
    }

    #[test]
    fn json_refs_to_files_are_paths_and_local_pointers_are_not() {
        let text = "{\"a\":{\"$ref\" : \"defs/m.json#/x\"},\"b\":{\"$ref\":\"#/y\"},\"c\":{\"$ref\":\"https://x.io/s.json\"}}";
        let f = json(text);
        assert_eq!(targets(&f), ["defs/m.json"]);
        assert_eq!(&text[f.refs[0].start..f.refs[0].end], "defs/m.json");
    }
}
