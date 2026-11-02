use super::*;

fn assert_no_false_go_failure(packet: &Value) {
    let issues = packet["coverage"]["issues"].to_string();
    for reason in [
        "type_check_failed",
        "unavailable_external_package",
        "package_discovery_failed",
    ] {
        assert!(!issues.contains(reason), "{packet}");
    }
}

#[test]
fn nonmodule_go_files_do_not_invoke_go_and_keep_explicit_source_gap() {
    let f = Fixture::new();
    let repo = f.repo("nonmodule-root");
    let pin = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("app.py", "def run(): pass\n"),
            ("examples/snippet.go", "package sample\nfunc F() {}\n"),
        ],
    );
    let packet = f.run(&request(&repo, vec![pin.clone()], vec![pin]));
    assert_eq!(packet["toolchain"]["head"], "none");
    assert!(s(&packet, "analyzer").starts_with("source-only"));
    assert_no_false_go_failure(&packet);
    assert!(
        packet["coverage"]["issues"]
            .to_string()
            .contains("go_source_without_module")
    );
    assert_eq!(packet["impact"], "incomplete");
}

#[test]
fn rootless_repository_discovers_nested_modules_and_skips_go_testdata() {
    let f = Fixture::new();
    let repo = f.repo("nested-modules");
    let base_files = [
        ("a/go.mod", "module example/a\ngo 1.23\n"),
        ("a/code.go", "package a\nfunc F() int {return 1}\n"),
        ("b/go.mod", "module example/b\ngo 1.23\n"),
        ("b/code.go", "package b\nfunc F() int {return 1}\n"),
        ("testdata/broken.go", "invalid Go fixture"),
        ("testdata/go.mod", "module example/fixture\ngo 1.23\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &base_files);
    let mut files = base_files;
    files[1].1 = "package a\nfunc F() int {return 2}\n";
    let head = f.pin(&repo, "app", 'b', &files);
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert_no_false_go_failure(&packet);
    assert_eq!(packet["impact"], "potential_impact", "{packet}");
    assert_eq!(packet["package_instances"]["head_count"], 2);
    assert!(has(&packet, "head", ".F"));
}

#[test]
fn admitted_vendor_sources_restore_the_import_chain_and_cover_subtrees() {
    let f = Fixture::new();
    let repo = f.repo("vendor-chain");
    let module = "module example/app\ngo 1.23\nrequire example.invalid/pq v1.0.0\n";
    let code =
        "package db\nimport \"example.invalid/pq\"\nfunc Value() int { return pq.Value() }\n";
    let entity =
        "package entity\nimport \"example/app/db\"\nfunc Value() int { return db.Value() }\n";
    let files = [
        ("go.mod", module),
        ("db/code.go", code),
        ("entity/code.go", entity),
    ];
    let missing = f.pin(&repo, "app", 'a', &files);
    let broken = f.run(&request(&repo, vec![missing.clone()], vec![missing]));
    let counts = &broken["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(counts["incomplete_package_instances"], 2);
    assert_eq!(counts["type_check_failed_package_instances"], 2);
    assert!(counts["incomplete_package_references"].as_u64().unwrap() > 0);

    let mut prepared = files.to_vec();
    prepared.extend([
        (
            "vendor/modules.txt",
            "# example.invalid/pq v1.0.0\n## explicit; go 1.23\nexample.invalid/pq\n",
        ),
        (
            "vendor/example.invalid/pq/code.go",
            "package pq\nfunc Value() int {return 1}\n",
        ),
    ]);
    let base = f.pin(&repo, "app", 'b', &prepared);
    prepared[2].1 =
        "package entity\nimport \"example/app/db\"\nfunc Value() int { return db.Value()+1 }\n";
    let head = f.pin(&repo, "app", 'c', &prepared);
    let packet = f.run(&request(&repo, vec![base.clone()], vec![head.clone()]));
    assert_eq!(packet["impact"], "potential_impact", "{packet}");
    let counts = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    for name in [
        "incomplete_package_instances",
        "type_check_failed_package_instances",
        "incomplete_package_references",
        "unavailable_external_package_instances",
    ] {
        assert_eq!(counts[name], 0, "{name}: {counts}");
    }
    assert!(has(&packet, "head", ".Value"));
    // A caller selecting a package subtree must still use the module vendor
    // tree. All authored Go files are covered by these two scopes.
    let mut a = base;
    let mut b = head;
    a["modules"] = json!(["db", "entity"]);
    b["modules"] = a["modules"].clone();
    let subtree = f.run(&request(&repo, vec![a], vec![b]));
    assert_no_false_go_failure(&subtree);
    assert_eq!(subtree["impact"], "potential_impact", "{subtree}");
}
