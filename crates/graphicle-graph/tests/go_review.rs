use graphicle::extractors::go_types;
use serde_json::{Value, json};
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    root
}
fn run(
    root: &tempfile::TempDir,
    roots: &[&str],
    tags: &[&str],
    tests: bool,
    workspace: &str,
) -> Value {
    go_types::review(root.path(),&json!({"roots":roots.iter().map(|r|json!({"directory":r,"module":"."})).collect::<Vec<_>>(),"goos":"linux","goarch":"amd64","tags":tags,"tests":tests,"workspace":workspace})).unwrap()
}
#[test]
fn compiler_keeps_receiver_identity_and_build_tags() {
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        (
            "app/main.go",
            "package app\ntype A struct{}\ntype B struct{}\nfunc (A) Target() int {return 1}\nfunc (B) Target() int {return 2}\nfunc Use() int {return A{}.Target()}\n",
        ),
        (
            "app/active.go",
            "//go:build pilot\npackage app\nfunc Tagged() int {return A{}.Target()}\n",
        ),
        (
            "app/inactive.go",
            "//go:build !pilot\npackage app\nfunc Tagged() int {return B{}.Target()}\n",
        ),
    ]);
    let report = run(&root, &["app"], &["pilot"], false, "");
    let world = &report["worlds"][0];
    assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
    let calls: Vec<_> = world["calls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["relation"] == "calls")
        .collect();
    assert_eq!(calls.len(), 2);
    for c in calls {
        assert!(
            c["target"]
                .as_str()
                .unwrap()
                .contains("(example/app.A).Target"),
            "{c}"
        );
        assert!(c.get("reason").is_none());
    }
    assert!(
        !world["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["file"] == "app/inactive.go")
    );
}
#[test]
fn each_consumer_uses_its_own_package_instance_and_local_override() {
    let root = tree(&[
        (
            "one/go.mod",
            "module example/one\ngo 1.23\nrequire example/dep v1.0.0\nreplace example/dep => ../v1\n",
        ),
        (
            "one/main.go",
            "package one\nimport alias \"example/dep\"\nfunc Use() int {return alias.Target()}\n",
        ),
        (
            "two/go.mod",
            "module example/two\ngo 1.23\nrequire example/dep v1.1.0\nreplace example/dep => ../v2\n",
        ),
        (
            "two/main.go",
            "package two\nimport \"example/dep\"\nfunc Use() int {return dep.Target()}\n",
        ),
        ("v1/go.mod", "module example/dep\ngo 1.23\n"),
        ("v1/main.go", "package dep\nfunc Target() int {return 1}\n"),
        ("v2/go.mod", "module example/dep\ngo 1.23\n"),
        ("v2/main.go", "package dep\nfunc Target() int {return 2}\n"),
    ]);
    let report = run(&root, &["one", "two", "v1", "v2"], &[], false, "");
    for (index, file, version) in [(0, "v1/main.go", "v1.0.0"), (1, "v2/main.go", "v1.1.0")] {
        let world = &report["worlds"][index];
        assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
        assert_eq!(world["calls"][0]["target_file"], file);
        assert!(
            world["packages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["path"] == "example/dep" && p["version"] == version)
        );
    }
}
#[test]
fn workspace_tests_and_unsafe_replacements_have_explicit_results() {
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        ("app/go.work", "go 1.23\nuse .\n"),
        ("app/main.go", "package app\nfunc Target() int {return 1}\n"),
        (
            "app/main_test.go",
            "package app\nimport \"testing\"\nfunc TestTarget(t *testing.T) { if Target()!=1 {t.Fatal(\"bad\")} }\n",
        ),
    ]);
    let report = run(&root, &["app"], &[], true, "app/go.work");
    let world = &report["worlds"][0];
    assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
    assert!(
        world["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["file"] == "app/main_test.go")
    );
    assert!(
        world["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["spelling"] == "Target" && c.get("target").is_some())
    );
    std::fs::write(
        root.path().join("app/go.mod"),
        "module example/app\ngo 1.23\nreplace example/escape => ../../outside\n",
    )
    .unwrap();
    let refused = run(&root, &["app"], &[], false, "");
    assert!(
        refused["worlds"][0]["issues"]
            .to_string()
            .contains("unsafe_or_invalid_module_configuration")
    );
    assert!(
        refused["worlds"][0]["symbols"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn review_test_variants_link_exact_declarations_and_can_be_disabled() {
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        ("app/main.go", "package app\nfunc Target() int {return 1}\n"),
        (
            "app/main_test.go",
            "package app\nfunc Internal() int {return Target()}\n",
        ),
        (
            "app/external_test.go",
            "package app_test\nimport a \"example/app\"\nfunc External() int {return a.Target()}\n",
        ),
    ]);
    let report = run(&root, &["app"], &[], true, "");
    let world = &report["worlds"][0];
    assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
    for file in ["app/main_test.go", "app/external_test.go"] {
        let call = world["calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["file"] == file && c["relation"] == "calls")
            .unwrap();
        assert_eq!(call["target_file"], "app/main.go");
        assert!(call.get("reason").is_none(), "{call}");
        assert!(call["target"].as_str().unwrap().contains("Target"));
    }
    let disabled = run(&root, &["app"], &[], false, "");
    assert!(
        disabled["worlds"][0]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| !s["file"].as_str().unwrap().ends_with("_test.go"))
    );
}

#[test]
fn vendored_dependencies_are_resolved_from_the_pinned_tree() {
    let root = tree(&[
        (
            "app/go.mod",
            "module example/app\ngo 1.23\nrequire example/dep v1.0.0\n",
        ),
        (
            "app/main.go",
            "package app\nimport \"example/dep\"\nfunc Use() int { return dep.Target() }\n",
        ),
        (
            "app/vendor/modules.txt",
            "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\n",
        ),
        (
            "app/vendor/example/dep/dep.go",
            "package dep\nfunc Target() int { return 1 }\n",
        ),
    ]);
    let report = run(&root, &["app"], &[], false, "");
    let world = &report["worlds"][0];
    assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
    assert!(
        world["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["target_file"] == "app/vendor/example/dep/dep.go"
                && c.get("reason").is_none()),
        "{report}"
    );
}

#[test]
fn closures_keep_lexical_identity_after_unrelated_line_insertions() {
    let code = "package app\nfunc Use() { a := func() int { return 1 }; b := func() int { return 1 }; _, _ = a, b }\n";
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        ("app/main.go", code),
    ]);
    let first = run(&root, &["app"], &[], false, "");
    std::fs::write(
        root.path().join("app/main.go"),
        format!("// unrelated comment\n{code}"),
    )
    .unwrap();
    let second = run(&root, &["app"], &[], false, "");
    let closures = |r: &Value| {
        r["worlds"][0]["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["kind"] == "closure")
            .map(|s| (s["key"].clone(), s["revision"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(closures(&first).len(), 2);
    assert_eq!(closures(&first), closures(&second));
}
