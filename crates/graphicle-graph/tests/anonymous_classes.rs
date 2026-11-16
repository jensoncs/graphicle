//! A Java anonymous class (`new ArrayList() {{ add(x); }}`) or Kotlin object
//! literal (`object : Runnable { ... }`) is labelled by its base type, but it
//! is not that type: a `new ArrayList<>()` or `Runnable { }` elsewhere never
//! constructs it. Its own members still get nodes and calls.
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

#[test]
fn a_java_anonymous_class_is_not_its_base_type() {
    let c = calls(&[
        (
            "src/Cohorts.java",
            "import java.util.*;\nclass Cohorts {\n  List<String> names() {\n    return new ArrayList() {{\n      add(\"a\");\n    }};\n  }\n}\n",
        ),
        (
            "src/Other.java",
            "import java.util.*;\nclass Other {\n  List<String> make() {\n    return new ArrayList<>();\n  }\n}\n",
        ),
    ]);
    assert!(
        !c.iter()
            .any(|(s, t)| s == "src/Other.java:.make()" && t.starts_with("src/Cohorts.java:")),
        "{c:?}"
    );
}

#[test]
fn a_kotlin_object_literal_is_not_its_base_type() {
    let c = calls(&[
        (
            "src/Jobs.kt",
            "fun schedule(): Runnable {\n    return object : Runnable {\n        override fun run() {}\n    }\n}\n",
        ),
        (
            "src/Other.kt",
            "fun make(): Runnable {\n    return Runnable { }\n}\n",
        ),
    ]);
    assert!(
        !c.iter()
            .any(|(s, t)| s == "src/Other.kt:make()" && t.starts_with("src/Jobs.kt:")),
        "{c:?}"
    );
}

#[test]
fn a_java_anonymous_class_member_still_binds_inside_it() {
    let c = calls(&[(
        "src/Task.java",
        "class Task {\n  void start() {\n    Runnable r = new Runnable() {\n      public void run() { helper(); }\n      void helper() {}\n    };\n  }\n}\n",
    )]);
    assert!(
        c.iter()
            .any(|(s, t)| s == "src/Task.java:.run()" && t == "src/Task.java:.helper()"),
        "{c:?}"
    );
}
