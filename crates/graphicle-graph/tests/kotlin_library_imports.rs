//! `import androidx.compose.foundation.layout.Column` names a library class:
//! neither the import nor a `Column { }` call binds to a repository class
//! that is also called `Column`. An import of a scanned package, nested
//! types included, still does.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;
use std::collections::BTreeSet;

/// `(source file:label, target file:label)` of every `calls` edge.
fn calls(files: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
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
            cache_root: Some(root.join("cache")),
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
                format!("{file}:{}", n["label"].as_str().unwrap_or(""))
            })
            .unwrap_or_default()
    };
    result["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "calls")
        .map(|e| (name(&e["source"]), name(&e["target"])))
        .collect()
}

const TEMPLATE: &str = "package sharechat.ui.model\n\nsealed class Template {\n    data class Column(val n: Int) : Template()\n}\n";

#[test]
fn a_kotlin_library_import_does_not_bind_a_same_named_repo_class() {
    let c = calls(&[
        ("ui/model/Template.kt", TEMPLATE),
        (
            "app/Screen.kt",
            "package sharechat.app\n\nimport androidx.compose.foundation.layout.Column\n\nfun Screen() {\n    Column { }\n}\n",
        ),
    ]);
    assert!(
        !c.iter()
            .any(|(s, t)| s == "app/Screen.kt:Screen()" && t.starts_with("ui/model/Template.kt:")),
        "{c:?}"
    );
}

#[test]
fn a_kotlin_import_of_a_scanned_nested_type_still_binds() {
    let c = calls(&[
        ("ui/model/Template.kt", TEMPLATE),
        (
            "app/Use.kt",
            "package sharechat.app\n\nimport sharechat.ui.model.Template.Column\n\nfun use() = Column(1)\n",
        ),
    ]);
    assert!(
        c.contains(&(
            "app/Use.kt:use()".into(),
            "ui/model/Template.kt:Column".into()
        )),
        "{c:?}"
    );
}

#[test]
fn an_incremental_rebuild_still_sees_an_unchanged_files_package() {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    // The package is not mirrored by the directory, so only the file says it.
    let template = root.join("models/Template.kt");
    let user = root.join("app/Use.kt");
    for (path, text) in [
        (&template, TEMPLATE),
        (
            &user,
            "package sharechat.app\n\nimport sharechat.ui.model.Template.Column\n\nfun use() = Column(1)\n",
        ),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let options = |context: Vec<Value>| ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.join("cache")),
        parallel: false,
        resolution_context_nodes: context,
        ..Default::default()
    };
    let full = extract(&[template.clone(), user.clone()], &options(vec![])).unwrap();
    let context = full["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["source_file"] == "models/Template.kt")
        .cloned()
        .collect::<Vec<_>>();
    let column = context
        .iter()
        .find(|n| n["label"] == "Column")
        .map(|n| n["id"].clone())
        .unwrap();
    let partial = extract(&[user], &options(context)).unwrap();
    let edges = partial["edges"].as_array().unwrap();
    // The scanned package is not mistaken for a library's.
    assert!(
        edges
            .iter()
            .any(|e| e["relation"] == "calls" && e["target"] == column),
        "{edges:?}"
    );
    assert!(
        !edges.iter().any(|e| e["relation"] == "imports"
            && e["target"].as_str().is_some_and(|t| t.starts_with("ref_"))),
        "{edges:?}"
    );
}

#[test]
fn a_backticked_keyword_package_is_still_scanned() {
    let c = calls(&[
        (
            "lib/Util.kt",
            "package `in`.mohalla.util\n\nfun launchJob() = 1\n",
        ),
        (
            "app/Main.kt",
            "package `in`.mohalla.app\n\nimport `in`.mohalla.util.launchJob\n\nfun main() {\n    launchJob()\n}\n",
        ),
    ]);
    assert!(
        c.contains(&(
            "app/Main.kt:main()".into(),
            "lib/Util.kt:launchJob()".into()
        )),
        "{c:?}"
    );
}
