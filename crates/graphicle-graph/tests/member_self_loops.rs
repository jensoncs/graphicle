//! `x.load(s)` inside `func load(_ s:)` calls another object's `load`, not
//! this method: a member call on a receiver other than `self`/`this` never
//! binds to its own caller by name. Recursion (`load()`, `self.load()`)
//! stays a self-loop.
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
fn a_swift_member_call_on_another_object_is_not_recursion() {
    let c = calls(&[(
        "Web.swift",
        "class Web {\n    var view = WKWebView()\n    func load(_ s: String) {\n        view.load(s)\n    }\n    func count(_ n: Int) -> Int {\n        return n == 0 ? 0 : self.count(n - 1)\n    }\n    func walk(_ n: Int) {\n        if n > 0 { walk(n - 1) }\n    }\n}\n",
    )]);
    assert!(
        !c.contains(&("Web.swift:.load()".into(), "Web.swift:.load()".into())),
        "{c:?}"
    );
    assert!(
        c.contains(&("Web.swift:.count()".into(), "Web.swift:.count()".into())),
        "{c:?}"
    );
    assert!(
        c.contains(&("Web.swift:.walk()".into(), "Web.swift:.walk()".into())),
        "{c:?}"
    );
}

#[test]
fn a_kotlin_member_call_on_another_object_is_not_recursion() {
    let c = calls(&[(
        "Pipe.kt",
        "class Pipe(val stream: java.io.InputStream) {\n    fun close() {\n        stream.close()\n    }\n    fun drain(n: Int): Int = if (n == 0) 0 else this.drain(n - 1)\n    override fun toString(): String = super.toString()\n}\n",
    )]);
    assert!(
        !c.contains(&("Pipe.kt:.close()".into(), "Pipe.kt:.close()".into())),
        "{c:?}"
    );
    assert!(
        c.contains(&("Pipe.kt:.drain()".into(), "Pipe.kt:.drain()".into())),
        "{c:?}"
    );
    assert!(
        !c.contains(&("Pipe.kt:.toString()".into(), "Pipe.kt:.toString()".into())),
        "{c:?}"
    );
}
