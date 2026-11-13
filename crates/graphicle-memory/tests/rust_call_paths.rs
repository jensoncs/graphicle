//! Rust calls in a workspace crate: `crate::` and the crate's own name
//! resolve from the crate's root (not the repository's), a `use`d module
//! qualifies its calls, and calls inside macro arguments are calls.
use graphicle_memory::structure::{analyze, graph};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

fn calls(files: &[(&str, &str)]) -> BTreeSet<[String; 4]> {
    let mut analyses = Map::new();
    for (path, content) in files {
        analyses.insert((*path).to_string(), analyze(path, content).unwrap());
    }
    let raw = graph(&Value::Object(analyses)).unwrap();
    raw["symbol_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            ["path", "caller", "callee_path", "callee"].map(|k| r[k].as_str().unwrap().to_string())
        })
        .collect()
}

fn call(a: &str, b: &str, c: &str, d: &str) -> [String; 4] {
    [a.into(), b.into(), c.into(), d.into()]
}

const CRATE: [(&str, &str); 5] = [
    (
        "crates/mem/src/lib.rs",
        "pub mod cache;\npub mod ingest;\npub mod text;\nmod util;\nuse crate::util::helper;\npub fn run() -> u32 {\n    helper();\n    let v = vec![text::prefix(), 2];\n    let _ = format!(\"{}\", util::other());\n    v[0]\n}\n",
    ),
    (
        "crates/mem/src/cache.rs",
        "pub fn stats() -> u32 {\n    0\n}\n",
    ),
    (
        "crates/mem/src/text.rs",
        "pub fn prefix() -> u32 {\n    0\n}\n",
    ),
    (
        "crates/mem/src/util.rs",
        "pub fn helper() -> u32 {\n    0\n}\npub fn other() -> u32 {\n    0\n}\n",
    ),
    (
        "crates/mem/src/ingest.rs",
        "use crate::cache;\npub fn layout() -> u32 {\n    cache::stats()\n}\n",
    ),
];

#[test]
fn crate_paths_resolve_from_the_crate_root_in_a_workspace() {
    let got = calls(&CRATE);
    let lib = "crates/mem/src/lib.rs";
    assert!(
        got.contains(&call(lib, "run", "crates/mem/src/util.rs", "helper")),
        "{got:?}"
    );
    assert!(
        got.contains(&call(
            "crates/mem/src/ingest.rs",
            "layout",
            "crates/mem/src/cache.rs",
            "stats"
        )),
        "{got:?}"
    );
}

#[test]
fn calls_inside_macro_arguments_are_calls() {
    let got = calls(&CRATE);
    let lib = "crates/mem/src/lib.rs";
    assert!(
        got.contains(&call(lib, "run", "crates/mem/src/text.rs", "prefix")),
        "{got:?}"
    );
    assert!(
        got.contains(&call(lib, "run", "crates/mem/src/util.rs", "other")),
        "{got:?}"
    );
}

#[test]
fn an_integration_test_reaches_its_crate_by_name() {
    let mut files = CRATE.to_vec();
    files.push((
        "crates/mem/tests/cache_policy.rs",
        "use mem::cache;\nuse mem::run;\n#[test]\nfn keeps_entries() {\n    cache::stats();\n    run();\n}\n",
    ));
    let got = calls(&files);
    let test = "crates/mem/tests/cache_policy.rs";
    assert!(
        got.contains(&call(
            test,
            "keeps_entries",
            "crates/mem/src/cache.rs",
            "stats"
        )),
        "{got:?}"
    );
    assert!(
        got.contains(&call(test, "keeps_entries", "crates/mem/src/lib.rs", "run")),
        "{got:?}"
    );
}

#[test]
fn a_macro_metavariable_is_not_a_call() {
    let got = calls(&[(
        "src/lib.rs",
        "pub fn f() {}\nmacro_rules! each {\n    ($f:ident) => { $f() };\n}\npub fn g() {\n    each!(f);\n}\n",
    )]);
    assert!(
        !got.iter().any(|c| c[1] == "each" || c[3] == "$f"),
        "{got:?}"
    );
}
