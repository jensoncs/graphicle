//! Native extraction through the real workspace linker: no seeded symbols.
use graphicle::{
    api::{ExtractOptions, extract},
    extractors::routes,
    graph::Graph,
};
use graphicle_memory::graph::{cross_repo_http, cross_repo_imports::ExportIndex};
use serde_json::json;
use std::{fs, path::Path};

fn extract_graph(root: &Path, files: &[(&str, &str)]) -> Graph {
    let paths = files
        .iter()
        .map(|(name, body)| {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let result = extract(
        &paths,
        &ExtractOptions {
            root: Some(root.into()),
            cache_root: Some(root.join("out")),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    Graph::from_node_link(&json!({"directed":true,"multigraph":false,"graph":{},"nodes":result["nodes"],"links":result["edges"]}), "links").unwrap()
}

#[test]
fn framework_routes_link_across_repos_without_guessing_deployment() {
    routes::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let servers = [
        (
            "next",
            "app/api/items/route.ts",
            "export async function GET() {}",
            "/api/items",
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController class Items {\n@GetMapping(\"/spring\")\npublic Object list() {return null;}\n}",
            "/spring",
        ),
        (
            "actix",
            "main.rs",
            "#[actix_web::get(\"/actix\")]\nasync fn handler() {}",
            "/actix",
        ),
        (
            "axum",
            "main.rs",
            "use axum::{Router,routing::get};\nfn app() { Router::new().route(\"/axum\",get(handler)); }\nasync fn handler() {}",
            "/axum",
        ),
    ];
    for (repo, file, source, url) in servers {
        let server = extract_graph(
            &root.join(repo),
            &[
                ("package.json", r#"{"dependencies":{"next":"15"}}"#),
                (file, source),
            ],
        );
        let index = ExportIndex {
            repo: repo.into(),
            http_routes: cross_repo_http::exports(&server, repo, Some("server-revision")),
            ..Default::default()
        };
        assert_eq!(index.http_routes.len(), 1, "{repo}");
        assert_eq!(
            index.http_routes[0].related.len(),
            1,
            "actual handler: {repo}"
        );
        let client_source = format!("function load() {{ return fetch('{url}'); }}");
        let client = extract_graph(
            &root.join(format!("client-{repo}")),
            &[("client.ts", &client_source)],
        );
        let links = cross_repo_http::link(
            &client,
            "client",
            Some("client-revision"),
            std::slice::from_ref(&index),
        );
        assert_eq!(links.len(), 2, "route and actual handler: {repo}");
        assert!(
            links
                .iter()
                .all(|e| e.target.repo == repo && e.confidence == "INFERRED")
        );
        let mut duplicate = index.clone();
        duplicate.repo = "duplicate".into();
        assert!(
            cross_repo_http::link(&client, "client", None, &[index.clone(), duplicate]).is_empty(),
            "ambiguity: {repo}"
        );
        let absolute = extract_graph(
            &root.join(format!("absolute-{repo}")),
            &[(
                "client.ts",
                &format!("function load() {{ return fetch('https://unproven.example{url}'); }}"),
            )],
        );
        assert!(
            cross_repo_http::link(&absolute, "client", None, &[index]).is_empty(),
            "origin: {repo}"
        );
    }
    let firebase = extract_graph(
        &root.join("firebase"),
        &[(
            "functions.ts",
            "import {onCall} from 'firebase-functions/v2/https'; export const checkout=onCall(() => {});",
        )],
    );
    assert!(cross_repo_http::exports(&firebase, "firebase", None).is_empty());
}
