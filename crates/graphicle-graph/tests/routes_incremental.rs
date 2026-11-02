//! `--routes` (beyond the upstream port): an incremental rebuild keeps route paths
//! exact when the prefix a router is mounted with lives in another file.
//! Its own test binary, since `--routes` is process-wide.
use graphicle::watch::{RebuildOptions, rebuild};
use serde_json::Value;
use std::{fs, path::Path, sync::atomic::Ordering};

fn write(root: &Path, name: &str, body: &str) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn routes(root: &Path) -> Vec<String> {
    let text = fs::read_to_string(root.join(".graphicle/graph.json")).unwrap();
    let graph: Value = serde_json::from_str(&text).unwrap();
    let mut out = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["node_kind"] == "route")
        .map(|n| n["label"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    out.sort();
    out
}

fn changed(root: &Path, name: &str) -> RebuildOptions {
    RebuildOptions {
        changed_paths: Some(vec![root.join(name)]),
        ..Default::default()
    }
}

const MAIN: &str = "from fastapi import FastAPI\nfrom .routers import items\n\napp = FastAPI()\n\
                    app.include_router(items.router, prefix=\"/v1\")\n";
const ITEMS: &str = "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n\
                     @router.get(\"/items\")\ndef list_items():\n    return []\n";
const MORE: &str =
    "\n\n@router.get(\"/items/{item_id}\")\ndef read_item(item_id):\n    return item_id\n";

/// Extraction keeps process-wide caches: one run at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `canonicalize` in the engine's plain spelling: on Windows without the
/// verbatim prefix (`\\?\C:\x` is `C:\x`), as the command line resolves a
/// root; elsewhere `canonicalize` itself.
fn canonical(path: &Path) -> std::path::PathBuf {
    let path = path.canonicalize().unwrap();
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) if cfg!(windows) && plain.as_bytes().get(1) == Some(&b':') => plain.into(),
        _ => path,
    }
}

fn corpus() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = canonical(dir.path());
    write(&root, "app/__init__.py", "");
    write(&root, "app/main.py", MAIN);
    write(&root, "app/routers/__init__.py", "");
    write(&root, "app/routers/items.py", ITEMS);
    (dir, root)
}

#[test]
fn incremental_rebuilds_keep_route_prefixes_from_other_files() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    graphicle::extractors::routes::ENABLED.store(true, Ordering::Relaxed);
    let (_dir, root) = corpus();
    assert!(rebuild(&root, &RebuildOptions::default()).unwrap().success);
    assert_eq!(routes(&root), ["GET /v1/items"]);

    // Only main.py changed, but the route in items.py moves: every file is
    // rebuilt.
    write(&root, "app/main.py", &MAIN.replace("/v1", "/v2"));
    let result = rebuild(&root, &changed(&root, "app/main.py")).unwrap();
    assert!(result.success, "{:?}", result.messages);
    assert!(
        result
            .messages
            .iter()
            .any(|m| m.contains("Route mounts changed")),
        "{:?}",
        result.messages
    );
    assert_eq!(routes(&root), ["GET /v2/items"]);

    // Only the router changed: an incremental rebuild that still applies the
    // prefix main.py gives (main.py is read as context).
    write(&root, "app/routers/items.py", &format!("{ITEMS}{MORE}"));
    let result = rebuild(&root, &changed(&root, "app/routers/items.py")).unwrap();
    assert!(result.success, "{:?}", result.messages);
    assert!(
        !result
            .messages
            .iter()
            .any(|m| m.contains("Route mounts changed")),
        "{:?}",
        result.messages
    );
    let incremental = routes(&root);
    assert_eq!(incremental, ["GET /v2/items", "GET /v2/items/{item_id}"]);

    // A full build from scratch agrees.
    fs::remove_dir_all(root.join(".graphicle")).unwrap();
    assert!(rebuild(&root, &RebuildOptions::default()).unwrap().success);
    assert_eq!(routes(&root), incremental);
}

fn extract(root: &Path) -> String {
    let out = graphicle::cli::run(&[
        "extract".to_string(),
        root.to_string_lossy().into_owned(),
        "--code-only".to_string(),
        "--routes".to_string(),
    ]);
    assert_eq!(out.exit_code, 0, "{}{}", out.stdout, out.stderr);
    out.stdout
}

#[test]
fn incremental_extract_keeps_route_prefixes_from_other_files() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_dir, root) = corpus();
    extract(&root);
    assert_eq!(routes(&root), ["GET /v1/items"]);
    write(&root, "app/main.py", &MAIN.replace("/v1", "/v2"));
    let stdout = extract(&root);
    assert!(stdout.contains("route mounts changed"), "{stdout}");
    assert_eq!(routes(&root), ["GET /v2/items"]);
    write(&root, "app/routers/items.py", &format!("{ITEMS}{MORE}"));
    let stdout = extract(&root);
    assert!(stdout.contains("changed (incremental)"), "{stdout}");
    assert!(!stdout.contains("route mounts changed"), "{stdout}");
    assert_eq!(routes(&root), ["GET /v2/items", "GET /v2/items/{item_id}"]);
}

#[test]
fn incremental_requests_replace_links_after_url_and_helper_edits() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = canonical(dir.path());
    let client = "function load(path) { return fetch('/inventory'+path); }\nfunction caller() { const endpoint='/orders'; return load(endpoint); }\n";
    write(&root, "client.ts", client);
    write(
        &root,
        "server.ts",
        "const app=express();\napp.get('/inventory/orders',handler);\napp.get('/inventory/items',handler);\napp.get('/archive/items',handler);\nfunction handler() {}\n",
    );
    let destinations = || {
        let graph: Value =
            serde_json::from_str(&fs::read_to_string(root.join(".graphicle/graph.json")).unwrap())
                .unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let caller = nodes
            .iter()
            .find(|n| n["label"] == "caller()")
            .expect("real caller declaration");
        let mut paths: Vec<_> = graph["links"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["source"] == caller["id"] && e["relation"] == "http_calls")
            .map(|e| {
                nodes.iter().find(|n| n["id"] == e["target"]).unwrap()["route_path"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        paths.sort();
        paths
    };
    extract(&root);
    assert_eq!(destinations(), ["/inventory/orders"]);
    write(&root, "client.ts", &client.replace("'/orders'", "'/items'"));
    extract(&root);
    assert_eq!(destinations(), ["/inventory/items"]);
    write(
        &root,
        "client.ts",
        &client
            .replace("'/orders'", "'/items'")
            .replace("'/inventory'", "'/archive'"),
    );
    extract(&root);
    assert_eq!(destinations(), ["/archive/items"]);
    // Making the argument dynamic removes the previously resolved link.
    write(
        &root,
        "client.ts",
        &client.replace("const endpoint='/orders'", "const endpoint=unknownPath"),
    );
    extract(&root);
    assert!(destinations().is_empty());
}

#[test]
fn next_manifest_changes_enable_and_disable_routes() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = canonical(dir.path());
    write(&root, "package.json", "{}");
    write(&root, "app/api/items/route.ts", "export function GET() {}");
    extract(&root);
    assert!(routes(&root).is_empty());
    write(&root, "package.json", r#"{"dependencies":{"next":"15"}}"#);
    extract(&root);
    assert_eq!(routes(&root), ["GET /api/items"]);
    write(&root, "app/api/items/route.ts", "export function POST() {}");
    extract(&root);
    assert_eq!(routes(&root), ["POST /api/items"]);
    write(&root, "package.json", "{}");
    extract(&root);
    assert!(routes(&root).is_empty());
    write(&root, "package.json", r#"{"dependencies":{"next":"15"}}"#);
    extract(&root);
    assert_eq!(routes(&root), ["POST /api/items"]);
    fs::remove_file(root.join("package.json")).unwrap();
    extract(&root);
    assert!(routes(&root).is_empty());
}

#[test]
fn firebase_edits_replace_boundary_facts() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let root = canonical(dir.path());
    let kinds = || {
        let graph: Value =
            serde_json::from_str(&fs::read_to_string(root.join(".graphicle/graph.json")).unwrap())
                .unwrap();
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["boundary_kind"].as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    write(
        &root,
        "functions.ts",
        "import {onMessagePublished} from 'firebase-functions/v2/pubsub'; export const orders=onMessagePublished('orders', () => {});",
    );
    extract(&root);
    assert_eq!(kinds(), ["pubsub"]);
    write(
        &root,
        "functions.ts",
        "import {onCall} from 'firebase-functions/v2/https'; export const orders=onCall(() => {});",
    );
    extract(&root);
    assert_eq!(kinds(), ["callable"]);
    write(
        &root,
        "functions.ts",
        "import {onCall} from 'firebase-functions/v2/https'; const orders=onCall(() => {});",
    );
    extract(&root);
    assert!(kinds().is_empty());
}
