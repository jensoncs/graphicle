// Adapted from Graphify 0.9.101-0.9.102 (f3e50225, 0a9491d3, 084c3fd1),
// Apache-2.0; see NOTICE.md.
//! A bare Python call in a method body never names a method, and a bare
//! builtin name (`NotImplementedError`, `list`) binds to a project class only
//! when the caller imports that exact symbol. Ported from upstream's
//! `test_python_bare_call_not_enclosing_method.py` and
//! `test_python_builtin_call_shadowing_4989.py`.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;
use std::collections::BTreeSet;

/// `(source file:label, target file:label, relation, confidence)` of every
/// `calls` and `indirect_call` edge.
type Edge = (String, String, String, String);

fn edges(files: &[(&str, &str)]) -> BTreeSet<Edge> {
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
                format!("{file}:{}", n["label"].as_str().unwrap_or(""))
            })
            .unwrap_or_default()
    };
    result["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| matches!(e["relation"].as_str(), Some("calls" | "indirect_call")))
        .map(|e| {
            (
                name(&e["source"]),
                name(&e["target"]),
                e["relation"].as_str().unwrap_or("").to_string(),
                e["confidence"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// Just the `calls` pairs.
fn calls(edges: &BTreeSet<Edge>) -> BTreeSet<(String, String)> {
    edges
        .iter()
        .filter(|(_, _, r, _)| r == "calls")
        .map(|(s, t, _, _)| (s.clone(), t.clone()))
        .collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.into(), b.into())
}

/// Edges from a function in `from` to one in `to`.
fn between<'a>(edges: &'a BTreeSet<Edge>, from: &str, to: &str) -> Vec<&'a Edge> {
    edges
        .iter()
        .filter(|(s, t, _, _)| {
            s.starts_with(&format!("{from}:")) && t.starts_with(&format!("{to}:"))
        })
        .collect()
}

// -- a bare call in a method body (0.9.101, f3e50225) ---------------------

const BUILD_API: &str = "def get_build_status(c, b):\n    return 1\n";

#[test]
fn classmethod_calling_an_imported_function_of_the_same_name() {
    let e = edges(&[
        ("tpl/__init__.py", ""),
        ("tpl/build_api.py", BUILD_API),
        (
            "tpl/main.py",
            "from tpl.build_api import get_build_status\n\nclass AsyncTemplate:\n    @classmethod\n    def get_build_status(cls, b):\n        return get_build_status(None, b)\n",
        ),
    ]);
    assert_eq!(
        calls(&e),
        BTreeSet::from([pair(
            "tpl/main.py:.get_build_status()",
            "tpl/build_api.py:get_build_status()"
        )]),
        "{e:?}"
    );
}

#[test]
fn plain_method_calling_an_imported_function_of_the_same_name() {
    let e = edges(&[
        ("tpl/__init__.py", ""),
        ("tpl/build_api.py", BUILD_API),
        (
            "tpl/main.py",
            "from tpl.build_api import get_build_status\n\nclass Template:\n    def get_build_status(self, b):\n        return get_build_status(None, b)\n",
        ),
    ]);
    assert_eq!(
        calls(&e),
        BTreeSet::from([pair(
            "tpl/main.py:.get_build_status()",
            "tpl/build_api.py:get_build_status()"
        )]),
        "{e:?}"
    );
}

#[test]
fn method_calling_a_same_file_module_function_of_the_same_name() {
    let e = edges(&[(
        "m.py",
        "def helper():\n    return 1\n\nclass C:\n    def helper(self):\n        return helper()\n",
    )]);
    assert_eq!(
        calls(&e),
        BTreeSet::from([pair("m.py:.helper()", "m.py:helper()")]),
        "{e:?}"
    );
}

#[test]
fn nested_function_in_a_method_skips_the_method() {
    let e = edges(&[(
        "m.py",
        "def helper():\n    return 1\n\nclass C:\n    def helper(self):\n        def inner():\n            return helper()\n        return inner()\n",
    )]);
    let c = calls(&e);
    assert!(c.contains(&pair("m.py:inner()", "m.py:helper()")), "{e:?}");
    assert!(
        !c.contains(&pair("m.py:inner()", "m.py:.helper()")),
        "{e:?}"
    );
}

#[test]
fn module_function_recursion_still_links_to_itself() {
    let e = edges(&[(
        "m.py",
        "def fact(n):\n    return 1 if n < 2 else n * fact(n - 1)\n",
    )]);
    assert_eq!(
        calls(&e),
        BTreeSet::from([pair("m.py:fact()", "m.py:fact()")]),
        "{e:?}"
    );
}

#[test]
fn self_method_call_still_resolves() {
    let e = edges(&[(
        "m.py",
        "class C:\n    def a(self):\n        return self.b()\n    def b(self):\n        return 1\n",
    )]);
    assert_eq!(
        calls(&e),
        BTreeSet::from([pair("m.py:.a()", "m.py:.b()")]),
        "{e:?}"
    );
}

// -- builtin names (0.9.102, #4989) ---------------------------------------

const ERRORS: &str = "class NotImplementedError(Exception):\n    pass\n";

#[test]
fn an_unimported_builtin_exception_has_no_cross_file_edge() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", ERRORS),
        (
            "pkg/a.py",
            "def todo():\n    raise NotImplementedError(\"x\")\n",
        ),
    ]);
    assert!(between(&e, "pkg/a.py", "pkg/errors.py").is_empty(), "{e:?}");
}

#[test]
fn an_imported_builtin_exception_still_resolves_as_extracted() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", ERRORS),
        (
            "pkg/b.py",
            "from pkg.errors import NotImplementedError\n\n\ndef todo2():\n    raise NotImplementedError(\"x\")\n",
        ),
    ]);
    let found = between(&e, "pkg/b.py", "pkg/errors.py");
    assert_eq!(found.len(), 1, "{e:?}");
    assert_eq!(found[0].3, "EXTRACTED", "{e:?}");
}

#[test]
fn another_symbol_from_the_same_module_does_not_certify_a_builtin() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/errors.py",
            "class NotImplementedError(Exception):\n    pass\n\n\nclass ValueError(Exception):\n    pass\n",
        ),
        (
            "pkg/base.py",
            "from pkg.errors import ValueError\n\n\ndef todo3():\n    raise NotImplementedError(\"x\")\n",
        ),
    ]);
    assert!(
        between(&e, "pkg/base.py", "pkg/errors.py").is_empty(),
        "{e:?}"
    );
}

#[test]
fn a_plain_module_import_does_not_bind_a_builtin_name() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", ERRORS),
        (
            "pkg/c.py",
            "import pkg.errors\n\n\ndef todo4():\n    raise NotImplementedError(\"x\")\n",
        ),
    ]);
    assert!(between(&e, "pkg/c.py", "pkg/errors.py").is_empty(), "{e:?}");
}

#[test]
fn a_local_class_with_a_builtin_name_still_binds_in_file() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", ERRORS),
        (
            "pkg/d.py",
            "class NotImplementedError(Exception):\n    pass\n\n\ndef todo5():\n    raise NotImplementedError(\"x\")\n",
        ),
    ]);
    let in_file: Vec<_> = e
        .iter()
        .filter(|(s, t, r, _)| {
            r == "calls" && s.starts_with("pkg/d.py:") && t == "pkg/d.py:NotImplementedError"
        })
        .collect();
    assert_eq!(in_file.len(), 1, "{e:?}");
    assert_eq!(in_file[0].3, "EXTRACTED", "{e:?}");
    assert!(between(&e, "pkg/d.py", "pkg/errors.py").is_empty(), "{e:?}");
}

#[test]
fn a_builtin_exception_passed_by_name_has_no_cross_file_edge() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", ERRORS),
        (
            "pkg/e.py",
            "def check(fn):\n    return fn\n\n\ndef todo6():\n    return check(NotImplementedError)\n",
        ),
    ]);
    assert!(between(&e, "pkg/e.py", "pkg/errors.py").is_empty(), "{e:?}");
}

#[test]
fn a_non_builtin_unimported_name_keeps_its_inferred_edge() {
    let e = edges(&[
        ("pkg/__init__.py", ""),
        ("pkg/errors.py", "class MyError(Exception):\n    pass\n"),
        ("pkg/f.py", "def todo7():\n    raise MyError(\"x\")\n"),
    ]);
    let found = between(&e, "pkg/f.py", "pkg/errors.py");
    assert_eq!(found.len(), 1, "{e:?}");
    assert_eq!(found[0].3, "INFERRED", "{e:?}");
}
