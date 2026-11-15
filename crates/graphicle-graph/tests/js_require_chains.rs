//! A CommonJS `require` whose result is called at once is still an import:
//! `const logger = require('pkg').logger({...})` and
//! `const req = require('pkg').initWrapper('svc')` import `pkg`.
use graphicle::extractors::{engine::extract_generic, engine_config::config};
use std::path::Path;

fn import_targets(source: &str) -> Vec<String> {
    let result = extract_generic(
        Path::new("driver/index.js"),
        &config("js").unwrap(),
        Some(source.as_bytes()),
    );
    result["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["relation"] == "imports_from")
        .filter_map(|e| e["target"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
}

#[test]
fn a_called_member_of_a_require_imports_the_package() {
    let targets = import_targets(
        "const scRequest = require('sharechat-http-request').initWrapper('user-service');\n\
         const logger = require(\"sharechat-sdk\").logger({ name: 'x' });\n\
         const helper = require('sharechat-sdk').esHelper(client).search;\n",
    );
    assert!(
        targets.contains(&"ref_sharechat_http_request".to_string()),
        "{targets:?}"
    );
    assert!(
        targets.contains(&"ref_sharechat_sdk".to_string()),
        "{targets:?}"
    );
}

#[test]
fn a_call_that_is_not_require_imports_nothing() {
    let targets = import_targets("const x = factory('pkg').build();\nconst y = load('a').b;\n");
    assert!(targets.is_empty(), "{targets:?}");
}

/// `const { a } = require('./m').load()` takes `a` from what `load` returns:
/// from `./m` it imports `load`, not `a`.
#[test]
fn a_destructured_chain_imports_the_called_member() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("m.js"),
        "exports.load = () => ({ a: 1 });\n",
    )
    .unwrap();
    let main = dir.path().join("main.js");
    std::fs::write(&main, "const { a } = require('./m').load();\n").unwrap();
    let result = extract_generic(&main, &config("js").unwrap(), None);
    let named: Vec<&str> = result["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["relation"] == "imports")
        .filter_map(|e| e["target"].as_str())
        .collect();
    assert!(named.iter().any(|t| t.ends_with("_load")), "{named:?}");
    assert!(!named.iter().any(|t| t.ends_with("_a")), "{named:?}");
}
