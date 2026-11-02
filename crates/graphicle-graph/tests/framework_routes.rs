//! Independent synthetic endpoint/boundary inventory, with before/after receipts.
use graphicle::extractors::routes;
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn framework_inventory_acceptance() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // family, file, source, expected inventory. Empty expectations are abstention controls.
    let cases: &[(&str, &str, &str, &[&str])] = &[
        (
            "next",
            "app/api/items/route.ts",
            "export async function GET() {}",
            &["route GET /api/items"],
        ),
        (
            "next",
            "src/app/(public)/api/items/[id]/route.ts",
            "export const POST = async () => {};",
            &["route POST /api/items/{id}"],
        ),
        (
            "next",
            "app/api/files/[...path]/route.js",
            "function list() {} export { list as GET };",
            &["route GET /api/files/{path:path}"],
        ),
        (
            "next",
            "pages/api/items/index.ts",
            "export default function handler(req,res) {}",
            &["route ANY /api/items"],
        ),
        (
            "next",
            "pages/api/items/[id].ts",
            "const handler = (req,res) => {}; export default handler;",
            &["route ANY /api/items/{id}"],
        ),
        (
            "next",
            "app/api/route.ts",
            "export { GET } from './other';",
            &[],
        ),
        ("next", "app/api/page.ts", "export function GET() {}", &[]),
        (
            "next",
            "app/_private/route.ts",
            "export function GET() {}",
            &[],
        ),
        (
            "next",
            "app/(.)photos/route.ts",
            "export function GET() {}",
            &[],
        ),
        (
            "next",
            "app/api/route.ts",
            "const GET = something; export {GET};",
            &[],
        ),
        (
            "lookalike",
            "app/api/route.ts",
            "export function GET() {}",
            &[],
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController\n@RequestMapping(\"/api\")\nclass Items {\n@GetMapping(\"/items\")\npublic Object list() {return null;}\n}",
            &["route GET /api/items"],
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController class Items {\n@RequestMapping(path={\"/a\",\"/b\"},method={RequestMethod.GET,RequestMethod.POST})\npublic Object list() {return null;}\n}",
            &[
                "route GET /a",
                "route GET /b",
                "route POST /a",
                "route POST /b",
            ],
        ),
        (
            "spring",
            "Items.java",
            "@org.springframework.web.bind.annotation.RestController class Items {\n@org.springframework.web.bind.annotation.PostMapping(\"/items\")\npublic Object create() {return null;}\n}",
            &["route POST /items"],
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController @RequestMapping(PREFIX) class Items { @GetMapping(\"/items\") public void list() {} }",
            &[],
        ),
        (
            "spring",
            "Items.java",
            "import custom.GetMapping; class Items { @GetMapping(\"/fake\") public void list() {} }",
            &[],
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController class Items { @GetMapping(PATH) public void list() {} }",
            &[],
        ),
        (
            "spring",
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\nclass Items { @GetMapping(\"/not-controller\") public void list() {} }",
            &[],
        ),
        (
            "actix",
            "server.rs",
            "#[actix_web::get(\"/items\")]\nasync fn list() {}",
            &["route GET /items"],
        ),
        (
            "actix",
            "server.rs",
            "use actix_web::{post as create};\n#[create(\"/items\")]\nasync fn list() {}",
            &["route POST /items"],
        ),
        (
            "actix",
            "server.rs",
            "#[get(\"/fake\")]\nasync fn list() {}",
            &[],
        ),
        (
            "actix",
            "server.rs",
            "#[actix_web::get(PATH)]\nasync fn list() {}",
            &[],
        ),
        (
            "axum",
            "server.rs",
            "use axum::{Router, routing::{get, post}};\nfn app() { let app = Router::new().route(\"/items\", get(list).post(create)); }\nasync fn list() {}\nasync fn create() {}",
            &["route GET /items", "route POST /items"],
        ),
        (
            "axum",
            "server.rs",
            "fn app() { let app = axum::Router::new().route(\"/items/{id}\", axum::routing::delete(remove)); }\nasync fn remove() {}",
            &["route DELETE /items/{id}"],
        ),
        (
            "axum",
            "server.rs",
            "fn app() { thing.route(\"/fake\", get(handler)); }",
            &[],
        ),
        (
            "axum",
            "server.rs",
            "use axum::{Router,routing::get}; fn app() { Router::new().route(PATH,get(handler)); }",
            &[],
        ),
        (
            "axum",
            "server.rs",
            "use axum::{Router,routing::get}; fn app(get: X) { Router::new().route(\"/fake\",get(handler)); }",
            &[],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onCall} from 'firebase-functions/v2/https'; export const checkout = onCall(async (req) => {});",
            &["cloud_function callable checkout"],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onRequest as request} from 'firebase-functions/v2/https'; export const webhook = request((req,res) => {});",
            &["cloud_function http webhook"],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onSchedule} from 'firebase-functions/v2/scheduler'; export const cleanup = onSchedule('every 24 hours', () => {});",
            &["cloud_trigger schedule cleanup"],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onDocumentCreated} from 'firebase-functions/v2/firestore'; export const created = onDocumentCreated('items/{id}', () => {});",
            &["cloud_trigger firestore created"],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onMessagePublished} from 'firebase-functions/v2/pubsub'; export const received = onMessagePublished('orders', () => {});",
            &["cloud_trigger pubsub received"],
        ),
        (
            "firebase",
            "functions.ts",
            "import * as functions from 'firebase-functions'; export const checkout = functions.https.onCall(() => {});",
            &["cloud_function callable checkout"],
        ),
        (
            "firebase",
            "client.ts",
            "import {httpsCallable} from 'firebase/functions'; function checkout() { return httpsCallable(functions, 'checkout'); }",
            &["cloud_call callable checkout"],
        ),
        (
            "firebase",
            "functions.ts",
            "function onCall(x) {return x;} export const fake = onCall(() => {});",
            &[],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onCall} from 'other'; export const fake = onCall(() => {});",
            &[],
        ),
        (
            "firebase",
            "functions.ts",
            "import {onCall} from 'firebase-functions/v2/https'; const internal = onCall(() => {});",
            &[],
        ),
        (
            "firebase",
            "client.ts",
            "import {httpsCallable} from 'firebase/functions'; function checkout(httpsCallable) { return httpsCallable(functions, 'checkout'); }",
            &[],
        ),
        (
            "firebase",
            "client.ts",
            "import {httpsCallable} from 'firebase/functions'; function checkout() { return httpsCallable(functions, name); }",
            &[],
        ),
    ];
    let baseline = std::env::var_os("GRAPHICLE_FRAMEWORK_BASELINE").is_some();
    let mut receipts = vec![];
    for (index, (family, name, body, expected)) in cases.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        let manifest = root.join("package.json");
        fs::write(
            &manifest,
            if *family == "next" {
                r#"{"dependencies":{"next":"15.0.0"}}"#
            } else {
                "{}"
            },
        )
        .unwrap();
        let mut nodes = vec![];
        let mut edges = vec![];
        routes::resolve(&[path, manifest], &mut nodes, &mut edges, &[], &root);
        let got: BTreeSet<String> = nodes
            .iter()
            .filter_map(|n| {
                let kind = n["node_kind"].as_str()?;
                if kind == "route" {
                    Some(format!(
                        "route {} {}",
                        n["route_method"].as_str()?,
                        n["route_path"].as_str()?
                    ))
                } else if kind.starts_with("cloud_") {
                    Some(format!(
                        "{kind} {} {}",
                        n["boundary_kind"].as_str()?,
                        n["boundary_name"].as_str()?
                    ))
                } else {
                    None
                }
            })
            .collect();
        let want: BTreeSet<String> = expected.iter().map(|s| (*s).into()).collect();
        receipts.push(json!({"case":index,"family":family,"file":name,"expected":want,"actual":got,"correct":got.intersection(&want).count(),"unexpected":got.difference(&want).count()}));
        if !baseline {
            assert_eq!(got, want, "case {index} {family}: {body}");
        }
        assert!(
            !edges.iter().any(|e| e["relation"] == "http_calls"),
            "inventory must not invent deployment links"
        );
    }
    println!("FRAMEWORK_EVALUATION={}", Value::Array(receipts));
}

#[test]
fn framework_boundaries_and_unsupported_composition() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "app/files/[[...path]]/route.ts",
            "export function GET() {}",
            &["GET /files", "GET /files/{path:path}"],
        ),
        (
            "app/files/[...path]/extra/route.ts",
            "export function GET() {}",
            &[],
        ),
        (
            "app/files/[[...path]]/extra/route.ts",
            "export function GET() {}",
            &[],
        ),
        (
            "pages/api/anon.ts",
            "export default (req,res) => {};",
            &["ANY /api/anon"],
        ),
        (
            "pages/api/anon.ts",
            "export default function(req,res) {};",
            &["ANY /api/anon"],
        ),
        (
            "app/api/route.ts",
            "function GET() {} export type { GET };",
            &[],
        ),
        (
            "app/api/route.ts",
            "export const GET = () => {}; GET = other;",
            &[],
        ),
        (
            "server.rs",
            "use axum::{Router,routing::get}; fn app() { Router::new().nest(\"/api\", Router::new().route(\"/wrong\", get(handler))); }",
            &[],
        ),
        (
            "server.rs",
            "use actix_web::{get,web}; #[get(\"/wrong\")] async fn handler() {} fn app() { web::scope(\"/api\").service(handler); }",
            &[],
        ),
        (
            "server.rs",
            "use axum::{Router,routing::get}; fn app() { Router::new().route(\"/files/{*path}\",get(handler)); }",
            &["GET /files/{path:path}"],
        ),
        (
            "Items.java",
            "import org.springframework.web.bind.annotation.*; @RestController class Items { @GetMapping(path=\"/x\",headers=\"X-Version=2\") public void item() {} }",
            &[],
        ),
        (
            "Items.java",
            "import org.springframework.web.bind.annotation.*; @interface GetMapping {} @RestController class Items { @GetMapping(\"/fake\") public void item() {} }",
            &[],
        ),
        (
            "functions.ts",
            "import type {onCall} from 'firebase-functions/v2/https'; export const fake = onCall(() => {});",
            &[],
        ),
        (
            "functions.ts",
            "import {onCall} from 'firebase-functions/v2/https'; export const fake = onCall();",
            &[],
        ),
        (
            "functions.ts",
            "import * as f from 'firebase-functions'; f.https.onCall=custom; export const fake=f.https.onCall(() => {});",
            &[],
        ),
        (
            "client.ts",
            "import {httpsCallable} from 'firebase/functions'; const f = function httpsCallable() { return httpsCallable(functions, 'wrong'); };",
            &[],
        ),
        (
            "functions.ts",
            "import * as f from 'firebase-functions'; export const cleanup=f.pubsub.schedule('every 24 hours').onRun(() => {});",
            &["schedule cleanup"],
        ),
        (
            "functions.ts",
            "import * as f from 'firebase-functions/v1'; export const created=f.firestore.document('items/{id}').onCreate(() => {});",
            &["firestore created"],
        ),
        (
            "functions.ts",
            "import * as f from 'firebase-functions'; export const received=f.pubsub.topic('orders').onPublish(() => {});",
            &["pubsub received"],
        ),
        (
            "functions.ts",
            "import {onValueCreated} from 'firebase-functions/v2/database'; export const created=onValueCreated('/items/{id}', () => {});",
            &["database created"],
        ),
        (
            "functions.ts",
            "import {onObjectFinalized} from 'firebase-functions/v2/storage'; export const uploaded=onObjectFinalized({bucket:'uploads'}, () => {});",
            &["storage uploaded"],
        ),
        (
            "functions.ts",
            "import {onCall} from 'firebase-functions/v2/https'; const fn=onCall(() => {}); export {fn as renamed};",
            &["callable renamed"],
        ),
    ];
    for (name, source, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
        let manifest = root.join("package.json");
        fs::write(&manifest, r#"{"dependencies":{"next":"15"}}"#).unwrap();
        let mut nodes = vec![];
        routes::resolve(&[path, manifest], &mut nodes, &mut vec![], &[], &root);
        let got = nodes
            .iter()
            .map(|n| n["label"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(got, expected.iter().copied().collect(), "{source}");
    }
    // A manifest outside the admitted file set cannot enable extraction;
    // a nested non-Next package also blocks the parent's framework identity.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("nested/app/api/route.ts");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "export function GET() {}").unwrap();
    let outer = root.join("package.json");
    let inner = root.join("nested/package.json");
    fs::write(&outer, r#"{"dependencies":{"next":"15"}}"#).unwrap();
    fs::write(&inner, "{}").unwrap();
    for paths in [vec![path.clone()], vec![path, inner, outer]] {
        let mut nodes = vec![];
        routes::resolve(&paths, &mut nodes, &mut vec![], &[], &root);
        assert!(nodes.is_empty());
    }
}

#[test]
fn native_extraction_links_actual_framework_handlers() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    routes::ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let files = [
        ("package.json", r#"{"dependencies":{"next":"15"}}"#),
        ("app/api/next/route.ts", "export async function GET() {}"),
        (
            "Items.java",
            "import org.springframework.web.bind.annotation.*;\n@RestController class Items {\n@GetMapping(\n\n\n\"/spring\"\n)\npublic Object list() {return null;}\n}",
        ),
        (
            "actix.rs",
            "#[actix_web::get(\"/actix\")]\nasync fn actix_handler() {}",
        ),
        (
            "axum.rs",
            "use axum::{Router,routing::get};\nfn app() { Router::new().route(\"/axum\",get(axum_handler)); }\nasync fn axum_handler() {}",
        ),
        (
            "client.ts",
            "function next() {return fetch('/api/next');}\nfunction spring() {return fetch('/spring');}\nfunction actix() {return fetch('/actix');}\nfunction axum() {return fetch('/axum');}",
        ),
    ];
    let paths = files
        .iter()
        .map(|(name, body)| {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let graph = graphicle::api::extract(
        &paths,
        &graphicle::api::ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.join("out")),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    let nodes = graph["nodes"].as_array().unwrap();
    let edges = graph["edges"].as_array().unwrap();
    for path in ["/api/next", "/spring", "/actix", "/axum"] {
        let route = nodes
            .iter()
            .find(|n| n["node_kind"] == "route" && n["route_path"] == path)
            .unwrap();
        assert_eq!(
            edges
                .iter()
                .filter(|e| e["target"] == route["id"] && e["relation"] == "handles")
                .count(),
            1,
            "missing real handler: {path}"
        );
        assert_eq!(
            edges
                .iter()
                .filter(|e| e["target"] == route["id"] && e["relation"] == "http_calls")
                .count(),
            1,
            "missing real caller: {path}"
        );
    }
}
