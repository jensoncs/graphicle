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

#[test]
fn repeated_initializers_and_local_types_keep_distinct_stable_identities() {
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        (
            "app/main.go",
            "package app\nfunc init() {}\nfunc init() {}\nfunc F() { type T struct{}; _ = T{}; func() {}() }\nfunc G() { type T struct{}; _ = T{} }\n",
        ),
    ]);
    let first = run(&root, &["app"], &[], false, "");
    let second = run(&root, &["app"], &[], false, "");
    let symbols = first["worlds"][0]["symbols"].as_array().unwrap();
    let keys: std::collections::BTreeSet<_> =
        symbols.iter().map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(symbols.len(), 7);
    assert_eq!(keys.len(), symbols.len());
    assert_eq!(first, second);
    let path = root.path().join("app/main.go");
    let source = std::fs::read_to_string(&path).unwrap().replacen(
        "package app",
        "package app\nfunc Unrelated() {}",
        1,
    );
    std::fs::write(path, source).unwrap();
    let third = run(&root, &["app"], &[], false, "");
    let later: std::collections::BTreeSet<_> = third["worlds"][0]["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert!(keys.is_subset(&later));
}

#[test]
fn package_values_initializers_and_proven_local_calls_have_targets() {
    let root = tree(&[
        ("app/go.mod", "module example/app\ngo 1.23\n"),
        (
            "app/main.go",
            r#"package app
const (First = iota; Second)
var Value = Seed()
var Callback = Seed
func Seed() int { return Second }
type I interface { Target() int }
type A struct{}
func (A) Target() int { return Value }
func Known() int {
 f := Seed
 alias := f
 var receiver I = A{}
 closure := func() int { return Value }
 return alias() + receiver.Target() + closure() + (func() int { return First })()
}
func Unknown(f func() int, receiver I) int { return f()+receiver.Target()+Callback() }
func Rebound() int { f := Seed; f = func() int {return 9}; return f() }
func Escaped() int { f := Seed; p := &f; _ = p; return f() }
func Captured() int { f := Seed; mutate := func() { f = func() int {return 9} }; mutate(); return f() }
"#,
        ),
    ]);
    let report = run(&root, &["app"], &[], false, "");
    let world = &report["worlds"][0];
    assert!(world["issues"].as_array().unwrap().is_empty(), "{report}");
    let symbols = world["symbols"].as_array().unwrap();
    let calls = world["calls"].as_array().unwrap();
    let owner = |name: &str| {
        symbols
            .iter()
            .find(|s| s["name"].as_str().unwrap().ends_with(name))
            .unwrap()["key"]
            .clone()
    };
    for name in [".Known", ".Value", ".Seed"] {
        let selected: Vec<_> = calls
            .iter()
            .filter(|c| c["caller"] == owner(name))
            .collect();
        assert!(!selected.is_empty(), "{name}");
        assert!(
            selected
                .iter()
                .all(|c| c.get("reason").is_none() && c.get("target").is_some()),
            "{selected:?}"
        );
    }
    assert!(calls.iter().any(|c| c["caller"] == owner(".Callback")
        && c["relation"] == "references_callable"
        && c["target"] == owner(".Seed")));
    for name in [".Unknown", ".Rebound", ".Escaped", ".Captured"] {
        assert!(
            calls.iter().any(|c| c["caller"] == owner(name)
                && c["relation"] == "calls"
                && c["reason"] == "unresolved_dynamic_target"),
            "{name}: {calls:?}"
        );
    }
    assert!(
        calls
            .iter()
            .any(|c| c["relation"] == "uses_value" && c["target"] == owner(".First"))
    );
}

#[test]
fn function_values_arguments_assignments_and_storage_keep_potential_targets() {
    let root=tree(&[("app/go.mod","module example/app\ngo 1.23\n"),("app/main.go",r#"package app
func A() {}
func B() {}
func invoke(cb func()) { cb() }
func relay(cb func()) { invoke(cb) }
func Use(flag bool) {
    f := A
    if flag { f = B }
    f()
    relay(f)
    table := map[string]func(){"a": A, "b": B}
    table["a"]()
    storage := struct { Run func() }{Run: B}
    storage.Run()
}
func Other() { invoke(B) }
"#)]);
    let report=run(&root,&["app"],&[],false,"");let calls=report["worlds"][0]["calls"].as_array().unwrap();
    for (spelling,targets) in [("f()",vec!["A","B"]),("cb()",vec!["A","B"]),("table[\"a\"]()",vec!["A"]),("storage.Run()",vec!["B"])] {
        let name=spelling.trim_end_matches("()");
        let possible:std::collections::BTreeSet<_>=calls.iter().filter(|c|c["spelling"]==name && c["relation"]=="may_call").map(|c|c["target"].as_str().unwrap().rsplit('.').next().unwrap()).collect();
        for target in targets {assert!(possible.iter().any(|v|v.starts_with(target)),"{name}: {possible:?}\n{report}");}
        assert!(calls.iter().any(|c|c["spelling"]==name && c["relation"]=="calls" && c.get("reason").is_some()),"unresolved invocation lost: {name}");
    }
    assert!(!calls.iter().any(|c|c["spelling"]=="table[\"a\"]" && c["relation"]=="may_call" && c["target"].as_str().unwrap().contains(".B")));
}

#[test]
fn unknown_indexes_interfaces_and_incomplete_packages_do_not_invent_callbacks() {
    let root=tree(&[("app/go.mod","module example/app\ngo 1.23\n"),("app/main.go",r#"package app
func A() {}
func Use(key string, cb func(), service interface{ Run() }) {
    table := map[string]func(){"a": A}
    table[key]()
    cb()
    service.Run()
}

"#)]);
    let report=run(&root,&["app"],&[],false,"");let calls=report["worlds"][0]["calls"].as_array().unwrap();
    assert!(!calls.iter().any(|c|c["relation"]=="may_call"));
    assert_eq!(calls.iter().filter(|c|c["relation"]=="calls" && c.get("reason").is_some()).count(),3);
}

#[test]
fn explicit_generic_callbacks_and_sparse_storage_do_not_conflate_slots() {
    let root=tree(&[("app/go.mod","module example/app\ngo 1.23\n"),("app/main.go",r#"package app
func A() {}
func B() {}
func invoke[T any](value T, callback func(T)) { callback(value) }
func Receive(value int) {}
func Use() {
    invoke[int](1, Receive)
    slots := [4]func(){2:A, B}
    slots[1]()
    slots[3]()
    object := &struct { Run func() }{Run:A}
    object.Run()
}
"#)]);
    let report=run(&root,&["app"],&[],false,"");let calls=report["worlds"][0]["calls"].as_array().unwrap();
    assert!(calls.iter().any(|c|c["spelling"]=="callback" && c["relation"]=="may_call" && c["target"].as_str().unwrap().contains(".Receive|")),"{report}");
    assert!(!calls.iter().any(|c|c["spelling"]=="slots[1]" && c["relation"]=="may_call"));
    assert!(calls.iter().any(|c|c["spelling"]=="slots[3]" && c["relation"]=="may_call" && c["target"].as_str().unwrap().contains(".B|")));
    assert!(calls.iter().any(|c|c["spelling"]=="object.Run" && c["relation"]=="may_call" && c["target"].as_str().unwrap().contains(".A|")));
}

#[test]
fn storage_identity_is_local_and_mutations_never_keep_a_stale_literal_target() {
    let root=tree(&[("app/go.mod","module example/app\ngo 1.23\n"),("app/main.go",r#"package app
func A() {}
func B() {}
func Use() {
    one := struct { Run func() }{Run:A}
    two := struct { Run func() }{Run:B}
    one.Run()
    two.Run()
    mutable := &struct { Run func() }{Run:A}
    alias := mutable
    alias.Run = B
    mutable.Run()
}
"#)]);
    let report=run(&root,&["app"],&[],false,"");let calls=report["worlds"][0]["calls"].as_array().unwrap();
    for (name,target) in [("one.Run",".A|"),("two.Run",".B|")] {
        let candidates:Vec<_>=calls.iter().filter(|c|c["spelling"]==name && c["relation"]=="may_call").collect();assert_eq!(candidates.len(),1,"{report}");assert!(candidates[0]["target"].as_str().unwrap().contains(target));
    }
    assert!(!calls.iter().any(|c|c["spelling"]=="mutable.Run" && c["relation"]=="may_call"));
    assert!(calls.iter().any(|c|c["spelling"]=="mutable.Run" && c.get("reason").is_some()));
}

#[test]
fn callback_constraint_exhaustion_is_explicit_and_suppresses_storage_guesses() {
    let source = format!("package app\nfunc A() {{}}\nfunc Use() {{ f := A\n{}\nf()\nbox := struct {{ Run func() }}{{Run:A}}\nbox.Run()\n}}\n", "f = A\n".repeat(130));
    let root = tree(&[("app/go.mod", "module example/app\ngo 1.23\n"), ("app/main.go", &source)]);
    let report = run(&root, &["app"], &[], false, "");
    let calls = report["worlds"][0]["calls"].as_array().unwrap();
    for spelling in ["f", "box.Run"] {
        assert!(calls.iter().any(|c|c["spelling"] == spelling && c["reason"] == "function_value_budget"), "{report}");
    }
    assert!(!calls.iter().any(|c|c["spelling"] == "box.Run" && c["relation"] == "may_call"));
}
