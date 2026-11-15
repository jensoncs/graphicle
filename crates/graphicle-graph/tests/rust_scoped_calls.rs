// Adapted from Graphify 0.9.101 (07f61e1b, afb17791, fdb288fd, bce0d562),
// Apache-2.0; see NOTICE.md.
//! Rust module-qualified calls (`m::f()`, `crate::m::f()`) resolve cross-file
//! by verifying the qualifier against the candidate's module path, and calls
//! written inside expression-macro arguments (`println!("{}", f())`) are
//! linked. Ported from upstream's `test_rust_scoped_call_resolution.py` and
//! `test_rust_macro_argument_calls.py`.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;
use std::collections::BTreeSet;

/// `(source file:label, target file:label, confidence)` of every `calls` edge,
/// with labels stripped of their call parens.
fn calls(files: &[(&str, &str)]) -> BTreeSet<(String, String, String)> {
    graphicle::rcfile::set_language_overrides(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let paths = files
        .iter()
        .map(|(rel, text)| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let result = extract(
        &paths,
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.join("graphify-out")),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    let name = |id: &Value| {
        nodes
            .iter()
            .find(|n| n["id"] == *id)
            .map(|n| {
                let file = n["source_file"].as_str().unwrap_or("");
                let file = file
                    .strip_prefix(root.to_str().unwrap())
                    .unwrap_or(file)
                    .trim_start_matches('/');
                let label = n["label"].as_str().unwrap_or("").trim_matches(['(', ')']);
                format!("{file}:{label}")
            })
            .unwrap_or_default()
    };
    result["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "calls")
        .map(|e| {
            (
                name(&e["source"]),
                name(&e["target"]),
                e["confidence"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn has(calls: &BTreeSet<(String, String, String)>, from: &str, to: &str, confidence: &str) -> bool {
    calls.contains(&(from.into(), to.into(), confidence.into()))
}

fn from(calls: &BTreeSet<(String, String, String)>, caller: &str) -> bool {
    calls.iter().any(|(s, _, _)| s == caller)
}

// -- module-qualified calls ----------------------------------------------

#[test]
fn module_qualified_call_resolves_to_the_named_module() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { b::helper(); }\n"),
        ("src/b.rs", "pub fn helper() {}\n"),
        ("src/c.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        has(&c, "src/a.rs:run", "src/b.rs:helper", "EXTRACTED"),
        "{c:?}"
    );
}

#[test]
fn mod_rs_form_resolves() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { util::helper(); }\n"),
        ("src/util/mod.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        has(&c, "src/a.rs:run", "src/util/mod.rs:helper", "EXTRACTED"),
        "{c:?}"
    );
}

#[test]
fn two_matching_module_files_emit_nothing() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { b::helper(); }\n"),
        ("src/b.rs", "pub fn helper() {}\n"),
        ("src/b/mod.rs", "pub fn helper() {}\n"),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn uppercase_qualifier_is_a_type_not_a_module() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { Helper::helper(); }\n"),
        (
            "src/helper.rs",
            "pub struct Helper;\nimpl Helper { pub fn helper() {} }\npub fn helper() {}\n",
        ),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn crate_qualified_path_verifies_against_directories() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { crate::util::helper(); }\n"),
        ("src/util.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        has(&c, "src/a.rs:run", "src/util.rs:helper", "EXTRACTED"),
        "{c:?}"
    );
}

#[test]
fn bare_super_and_self_are_skipped() {
    let c = calls(&[
        (
            "src/a.rs",
            "pub fn run() { super::helper(); self::helper2(); }\n",
        ),
        ("src/b.rs", "pub fn helper() {}\npub fn helper2() {}\n"),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn std_qualified_call_does_not_bind_to_a_local_shadow() {
    let c = calls(&[
        (
            "src/a.rs",
            "pub fn run() { let _ = std::fs::read(\"x\"); }\n",
        ),
        (
            "src/fs.rs",
            "pub fn read(p: &str) -> Vec<u8> { Vec::new() }\n",
        ),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn use_bound_qualifier_from_std_is_skipped() {
    let c = calls(&[
        (
            "src/a.rs",
            "use std::fs;\npub fn run() { let _ = fs::read(\"x\"); }\n",
        ),
        (
            "src/fs.rs",
            "pub fn read(p: &str) -> Vec<u8> { Vec::new() }\n",
        ),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn use_bound_qualifier_from_std_group_is_skipped() {
    let c = calls(&[
        (
            "src/a.rs",
            "use std::{fs, io};\npub fn run() { let _ = fs::read(\"x\"); }\n",
        ),
        (
            "src/fs.rs",
            "pub fn read(p: &str) -> Vec<u8> { Vec::new() }\n",
        ),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn use_bound_qualifier_from_the_crate_still_resolves() {
    let c = calls(&[
        (
            "src/a.rs",
            "use crate::util;\npub fn run() { util::helper(); }\n",
        ),
        ("src/util.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        has(&c, "src/a.rs:run", "src/util.rs:helper", "EXTRACTED"),
        "{c:?}"
    );
}

#[test]
fn scoped_call_cannot_bind_across_languages() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { b::helper(); }\n"),
        ("src/b.py", "def helper():\n    pass\n"),
    ]);
    assert!(!from(&c, "src/a.rs:run"), "{c:?}");
}

#[test]
fn bare_name_calls_keep_their_existing_resolution() {
    let c = calls(&[
        ("src/a.rs", "pub fn run() { helper(); }\n"),
        ("src/b.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        c.iter()
            .any(|(s, t, _)| s == "src/a.rs:run" && t == "src/b.rs:helper"),
        "{c:?}"
    );
}

#[test]
fn crate_path_does_not_cross_into_another_workspace_crate() {
    let c = calls(&[
        ("crate_a/Cargo.toml", "[package]\nname = \"a\"\n"),
        (
            "crate_a/src/main.rs",
            "mod util;\nfn run() { crate::util::f(); }\n",
        ),
        ("crate_a/src/util.rs", "pub fn g() {}\n"),
        ("crate_b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ("crate_b/src/util.rs", "pub fn f() {}\n"),
    ]);
    assert!(
        !c.iter().any(|(_, t, _)| t.starts_with("crate_b/")),
        "{c:?}"
    );
}

#[test]
fn module_path_never_names_an_impl_method() {
    let c = calls(&[
        ("Cargo.toml", "[package]\nname = \"s\"\n"),
        (
            "src/store.rs",
            "pub struct Db;\nimpl Db { pub fn open(&self) {} }\n",
        ),
        ("src/main.rs", "mod store;\nfn run() { store::open(); }\n"),
    ]);
    assert!(!from(&c, "src/main.rs:run"), "{c:?}");
}

#[test]
fn underscore_module_resolves() {
    let c = calls(&[
        ("Cargo.toml", "[package]\nname = \"u\"\n"),
        ("src/_internal.rs", "pub fn helper() {}\n"),
        (
            "src/main.rs",
            "mod _internal;\nfn run() { crate::_internal::helper(); }\n",
        ),
    ]);
    assert!(
        has(
            &c,
            "src/main.rs:run",
            "src/_internal.rs:helper",
            "EXTRACTED"
        ),
        "{c:?}"
    );
}

#[test]
fn path_call_does_not_bind_to_a_same_named_local_function() {
    let c = calls(&[
        ("Cargo.toml", "[package]\nname = \"p\"\n"),
        (
            "src/a.rs",
            "fn helper() {}\nfn run() { crate::b::helper(); }\n",
        ),
        ("src/b.rs", "pub fn helper() {}\n"),
    ]);
    assert!(
        has(&c, "src/a.rs:run", "src/b.rs:helper", "EXTRACTED"),
        "{c:?}"
    );
    assert!(
        !c.iter()
            .any(|(s, t, _)| s == "src/a.rs:run" && t == "src/a.rs:helper"),
        "{c:?}"
    );
}

#[test]
fn inline_module_path_binds_in_file() {
    let c = calls(&[(
        "src/a.rs",
        "mod inner { pub fn helper() {} }\nfn run() { inner::helper(); }\n",
    )]);
    assert!(
        c.iter()
            .any(|(s, t, _)| s == "src/a.rs:run" && t == "src/a.rs:helper"),
        "{c:?}"
    );
}

// -- calls inside macro arguments ----------------------------------------

#[test]
fn call_inside_macro_arguments_links_in_file() {
    let c = calls(&[(
        "lib.rs",
        "fn width() -> usize { 3 }\n\
         fn height() -> usize { 4 }\n\
         fn report() { println!(\"{} {}\", width(), height()); }\n\
         fn check() { assert_eq!(width(), 3); }\n\
         fn many() -> Vec<usize> { vec![width(), 1] }\n",
    )]);
    assert!(
        has(&c, "lib.rs:report", "lib.rs:width", "INFERRED"),
        "{c:?}"
    );
    assert!(
        has(&c, "lib.rs:report", "lib.rs:height", "INFERRED"),
        "{c:?}"
    );
    assert!(has(&c, "lib.rs:check", "lib.rs:width", "INFERRED"), "{c:?}");
    assert!(has(&c, "lib.rs:many", "lib.rs:width", "INFERRED"), "{c:?}");
}

#[test]
fn nested_token_trees_are_scanned() {
    let c = calls(&[(
        "lib.rs",
        "fn inner() -> u8 { 1 }\nfn outer() { println!(\"{:?}\", (Some(inner()), [inner()])); }\n",
    )]);
    assert!(has(&c, "lib.rs:outer", "lib.rs:inner", "INFERRED"), "{c:?}");
}

#[test]
fn call_inside_macro_reaches_a_function_in_another_file() {
    let c = calls(&[
        (
            "src/formatter.rs",
            "pub fn format_line() -> String { String::new() }\n",
        ),
        (
            "src/main.rs",
            "mod formatter;\nuse crate::formatter::format_line;\nfn main() { println!(\"{}\", format_line()); }\n",
        ),
    ]);
    assert!(
        c.iter()
            .any(|(s, t, _)| s == "src/main.rs:main" && t == "src/formatter.rs:format_line"),
        "{c:?}"
    );
}

#[test]
fn method_path_and_constructor_tokens_are_not_bare_calls() {
    let c = calls(&[(
        "lib.rs",
        "struct Wrap(u8);\n\
         fn len() -> usize { 0 }\n\
         fn new() -> u8 { 0 }\n\
         fn show(v: Vec<u8>) { println!(\"{} {} {:?}\", v.len(), Vec::<u8>::new().len(), Wrap(1).0); }\n",
    )]);
    assert!(!from(&c, "lib.rs:show"), "{c:?}");
}

#[test]
fn nested_macro_name_is_not_a_call() {
    let c = calls(&[(
        "lib.rs",
        "fn format() -> String { String::new() }\nfn show() { println!(\"{}\", format!(\"x\")); }\n",
    )]);
    assert!(
        !has(&c, "lib.rs:show", "lib.rs:format", "INFERRED"),
        "{c:?}"
    );
}

#[test]
fn macro_rules_body_is_not_scanned() {
    let c = calls(&[(
        "lib.rs",
        "fn helper() {}\nmacro_rules! twice { ($e:expr) => { helper(); helper(); }; }\nfn idle() {}\n",
    )]);
    assert!(!c.iter().any(|(_, t, _)| t == "lib.rs:helper"), "{c:?}");
}

#[test]
fn a_parsed_call_keeps_extracted_when_the_macro_comes_first() {
    let c = calls(&[(
        "lib.rs",
        "fn compute(x: u8) -> u8 { x }\nfn run() { println!(\"{}\", compute(1)); let _y = compute(2); }\n",
    )]);
    assert!(
        has(&c, "lib.rs:run", "lib.rs:compute", "EXTRACTED"),
        "{c:?}"
    );
    assert!(
        !has(&c, "lib.rs:run", "lib.rs:compute", "INFERRED"),
        "{c:?}"
    );
}

#[test]
fn dsl_macros_are_not_scanned() {
    let c = calls(&[(
        "lib.rs",
        "fn users() {}\n\
         fn name() {}\n\
         table! { users (id) { id -> Int4, } }\n\
         fn gen() { quote! { #name(1) }; html! { <div>{ users() }</div> }; }\n",
    )]);
    assert!(
        !c.iter()
            .any(|(_, t, _)| t == "lib.rs:users" || t == "lib.rs:name"),
        "{c:?}"
    );
}

#[test]
fn a_path_qualified_log_macro_is_scanned() {
    let c = calls(&[(
        "lib.rs",
        "fn state() -> u8 { 0 }\nfn tick() { log::info!(\"{}\", state()); }\n",
    )]);
    assert!(has(&c, "lib.rs:tick", "lib.rs:state", "INFERRED"), "{c:?}");
}
