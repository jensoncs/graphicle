//! Deployment hosts and called hosts on file nodes: a deployment's values
//! file lists the hosts it serves, and a config or code file the hosts it calls.
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

fn file<'g>(graph: &'g Value, source: &str) -> &'g Value {
    let nodes = graph["nodes"].as_array().unwrap();
    nodes
        .iter()
        .find(|n| {
            n["source_file"]
                .as_str()
                .is_some_and(|s| s.ends_with(source))
                && (n["url_hosts"].is_array() || n["deploy_hosts"].is_array())
        })
        .unwrap_or_else(|| {
            panic!(
                "no file node with hosts for {source}: {:#?}",
                nodes
                    .iter()
                    .filter(|n| n["source_file"]
                        .as_str()
                        .is_some_and(|s| s.ends_with(source)))
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn deploy_values_serve_hosts_and_config_and_code_call_them() {
    let graph = extract(&[
        (
            "infra/production/values.yaml",
            "applicationName: siren\nenv:\n  - name: GRAFANA_URL\n    value: http://grafana9.sharechat.internal\ningress:\n  hosts:\n    - siren.sharechat.internal\n",
        ),
        (
            "config/prod.json",
            "{\n  \"reviewUrl\": \"http://alert-review-agent.sharechat.internal/v1/review\"\n}\n",
        ),
        (
            "client.go",
            "package client\n\n// http://comment.sharechat.internal\nconst base = \"https://user-service.sharechat.internal/v1\"\n",
        ),
        ("README.md", "See http://docs.sharechat.internal\n"),
    ]);
    let values = file(&graph, "infra/production/values.yaml");
    assert_eq!(
        values["deploy_hosts"],
        serde_json::json!(["siren.sharechat.internal"])
    );
    assert_eq!(
        values["url_hosts"],
        serde_json::json!([["grafana9.sharechat.internal", 4]])
    );
    assert_eq!(
        file(&graph, "config/prod.json")["url_hosts"],
        serde_json::json!([["alert-review-agent.sharechat.internal", 2]])
    );
    assert_eq!(
        file(&graph, "client.go")["url_hosts"],
        serde_json::json!([["user-service.sharechat.internal", 4]])
    );
    assert!(!graph.to_string().contains("docs.sharechat.internal"));
}
