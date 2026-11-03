use super::*;

#[test]
fn pinned_go_packages_under_build_output_names_keep_types_and_embeds() {
    let f = Fixture::new();
    let repo = f.repo("pinned-build-names");
    let files = [
        (
            "go.mod",
            "module example/app\ngo 1.23\nrequire example/dep v1.0.0\n",
        ),
        (
            "main.go",
            "package app\nimport (\"example/app/internal/build\";\"example/dep/pkg/target\";\"example/dep/krusty\")\nfunc Value() int {return build.Value()+target.Value()+krusty.Value()}\n",
        ),
        (
            "internal/build/code.go",
            "package build\nimport _ \"embed\"\n//go:embed assets/dist/message.txt\nvar Message string\nfunc Value() int {return len(Message)}\n",
        ),
        (
            "internal/build/assets/dist/message.txt",
            "admitted embedded input",
        ),
        (
            "vendor/modules.txt",
            "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep/pkg/target\nexample/dep/internal/target\nexample/dep/krusty\n",
        ),
        (
            "vendor/example/dep/pkg/target/code.go",
            "package target\nfunc Value() int {return 1}\n",
        ),
        (
            "vendor/example/dep/internal/target/code.go",
            "package target\nfunc Value() int {return 2}\n",
        ),
        (
            "vendor/example/dep/krusty/code.go",
            "package krusty\nimport \"example/dep/internal/target\"\nfunc Value() int {return target.Value()}\n",
        ),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let changed = files[2].1.replace("len(Message)", "len(Message)+1");
    let mut next = files;
    next[2].1 = &changed;
    let head = f.pin(&repo, "app", 'b', &next);
    let args = request(&repo, vec![base], vec![head]);
    let packet = f.run(&args);
    for side in ["base", "head"] {
        let counts = &packet["coverage"]["analysis_diagnostics"][side]["go"];
        for name in [
            "incomplete_package_instances",
            "type_check_failed_package_instances",
            "incomplete_package_references",
            "unavailable_external_package_instances",
        ] {
            assert_eq!(counts[name], 0, "{side} {name}: {counts}");
        }
        assert_eq!(counts["owned_package_instances"], 2);
        assert_eq!(counts["dependency_package_instances"], 3);
    }
    assert!(
        !packet["coverage"]["issues"]
            .to_string()
            .contains("excluded_source")
    );
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["side"] == "head"
                && entry["file"] == "internal/build/code.go"
                && s(entry, "name").ends_with(".Value"))
    );
    // Corrected input inventory must also remain valid on cache reuse.
    let cached = f.run(&args);
    assert_eq!(cached["analysis_cache"]["base_hit"], true);
    assert_eq!(cached["analysis_cache"]["head_hit"], true);
    assert_eq!(cached["packet_id"], packet["packet_id"]);
}

#[test]
fn collector_can_exclude_fixture_module_roots_without_hiding_scope() {
    let f = Fixture::new();
    let repo = f.repo("excluded-modules");
    let pin = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("live/go.mod", "module example/live\ngo 1.23\n"),
            ("live/code.go", "package live\nfunc F() {}\n"),
            ("evals/copy/go.mod", "module example/copy\ngo 1.23\n"),
            (
                "evals/copy/code.go",
                "package copy\nfunc F() { Missing() }\n",
            ),
        ],
    );
    let all = f.run(&request(&repo, vec![pin.clone()], vec![pin.clone()]));
    assert!(
        all["coverage"]["analysis_diagnostics"]["head"]["go"]["type_check_failed_package_instances"]
            .as_u64()
            .unwrap() > 0
    );
    let mut excluded = pin;
    excluded["excluded_modules"] = json!(["evals/copy"]);
    let packet = f.run(&request(
        &repo,
        vec![excluded.clone()],
        vec![excluded.clone()],
    ));
    assert_no_false_go_failure(&packet);
    assert_eq!(packet["package_instances"]["head_count"], 1);
    assert!(
        packet["coverage"]["issues"]
            .to_string()
            .contains("go_module_excluded")
    );
    assert_eq!(packet["impact"], "incomplete");
    excluded["excluded_modules"] = json!(["../escape"]);
    assert!(
        review::run(
            &f.store,
            &f.workspace,
            &f.token,
            &request(&repo, vec![excluded.clone()], vec![excluded])
        )
        .is_err()
    );
}

#[test]
fn excluded_module_is_still_analyzed_when_imported_by_a_selected_root() {
    let f = Fixture::new();
    let repo = f.repo("excluded-dependency");
    let mut pin=f.pin(&repo,"app",'a',&[
        ("go.mod","module example/live\ngo 1.23\nrequire example/copy v0.0.0\nreplace example/copy => ./evals/copy\n"),
        ("code.go","package live\nimport \"example/copy\"\nfunc F() {copy.F()}\n"),
        ("evals/copy/go.mod","module example/copy\ngo 1.23\n"),
        ("evals/copy/code.go","package copy\nfunc F() { Missing() }\n"),
    ]);
    pin["excluded_modules"] = json!(["evals/copy"]);
    let packet = f.run(&request(&repo, vec![pin.clone()], vec![pin]));
    assert_eq!(packet["impact"], "incomplete");
    assert!(packet["coverage"]["analysis_diagnostics"]["head"]["go"]["type_check_failed_package_instances"].as_u64().unwrap()>0);
}

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
    assert_eq!(packet["impact"], "incomplete", "{packet}");
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
    assert_eq!(subtree["impact"], "incomplete", "{subtree}");
}

#[test]
fn dependency_changes_invalidate_owned_consumers_without_expanding_dependency_bodies() {
    let f = Fixture::new();
    let repo = f.repo("dependency-boundary");
    let files = [
        (
            "go.mod",
            "module example/app\ngo 1.23\nrequire example/dep v1.0.0\n",
        ),
        (
            "code.go",
            "package app\nimport \"example/dep\"\nfunc Use() int {return dep.Target()}\nfunc Caller() int {return Use()}\n",
        ),
        (
            "vendor/modules.txt",
            "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\n",
        ),
        (
            "vendor/example/dep/dep.go",
            "package dep\nfunc Target() int {return 1}\n",
        ),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let mut changed = files;
    changed[1].1 = "package app\nimport \"example/dep\"\nfunc Use() int {return dep.Target()+1}\nfunc Caller() int {return Use()}\n";
    let head = f.pin(&repo, "app", 'b', &changed);
    let packet = f.run(&request(&repo, vec![base.clone()], vec![head.clone()]));
    assert!(has(&packet, "head", ".Use"));
    assert!(has(&packet, "head", ".Caller"));
    assert!(!has(&packet, "head", ".Target"));
    let go = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(go["resolved_dependency_references"], 1);
    assert_eq!(go["dependency_package_instances"], 1);
    assert_eq!(go["incomplete_package_references"], 0);
    assert_eq!(
        go["dependency_boundary_examples"][0]["target"]["symbol"],
        "Target"
    );
    assert!(
        packet["coverage"]["issues"]
            .to_string()
            .contains("dependency_effects_unmodeled")
    );
    assert_eq!(packet["coverage"]["complete"], false);
    let warm = f.run(&request(&repo, vec![base.clone()], vec![head]));
    assert_eq!(warm["analysis_cache"]["head_hit"], true);
    assert_eq!(warm["packet_id"], packet["packet_id"]);
    changed = files;
    changed[3].1 = "package dep\nfunc Target() int {return 99}\n";
    let upgrade = f.pin(&repo, "app", 'c', &changed);
    let packet = f.run(&request(&repo, vec![base], vec![upgrade]));
    assert_eq!(packet["analysis_cache"]["head_hit"], false);
    assert!(has(&packet, "head", ".Use"));
    assert!(has(&packet, "head", ".Caller"));
    assert!(!has(&packet, "head", ".Target"));
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["side"] == "head")
            .any(|e| e["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == "conservative_scope"))
    );
}

#[test]
fn opaque_standard_library_callbacks_keep_coverage_incomplete() {
    let f = Fixture::new();
    let repo = f.repo("standard-callback");
    let files = [
        ("go.mod", "module example/app\ngo 1.23\n"),
        (
            "code.go",
            "package app\nimport \"encoding/json\"\ntype V struct{}\nfunc (V) MarshalJSON() ([]byte,error) {return []byte(\"1\"),nil}\nfunc Encode() ([]byte,error) {return json.Marshal(V{})}\n",
        ),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let modified = files[1].1.replace("[]byte(\"1\")", "[]byte(\"2\")");
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("go.mod", files[0].1), ("code.go", &modified)],
    );
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["side"] == "head" && s(e, "name").contains("MarshalJSON"))
    );
    assert_eq!(packet["impact"], "incomplete");
    let go = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(go["resolved_dependency_references"], 1);
    assert_eq!(
        go["dependency_boundary_examples"][0]["target"]["kind"],
        "standard_library"
    );
    assert!(
        !packet["coverage"]["issues"]
            .to_string()
            .contains("outside_pinned_scope")
    );
    assert!(
        packet["coverage"]["issues"]
            .to_string()
            .contains("dependency_effects_unmodeled")
    );
}

#[test]
fn large_vendor_assets_are_admitted_without_losing_owned_go_impact() {
    let f = Fixture::new();
    let repo = f.repo("large-vendor");
    let module = b"module example/app\ngo 1.23\nrequire example/dep v1.0.0\n";
    let before = b"package app\nimport \"example/dep\"\nfunc Use() int {return dep.Value()}\nfunc Caller() int {return Use()}\n";
    let after = b"package app\nimport \"example/dep\"\nfunc Use() int {return dep.Value()+1}\nfunc Caller() int {return Use()}\n";
    let vendor = b"# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\n";
    let dependency = b"package dep\nimport \"embed\"\n//go:embed data/*.bin\nvar Files embed.FS\nfunc Value() int {return 1}\n";
    // 72 MiB materialized, despite object-store deduplication of equal blobs.
    let blob = vec![0u8; 8 * 1024 * 1024];
    let names: Vec<_> = (0..9).map(|n| format!("vendor/example/dep/data/{n}.bin")).collect();
    let mut files: Vec<(&str, &[u8])> = vec![("go.mod",module),("code.go",before),
        ("vendor/modules.txt",vendor),("vendor/example/dep/code.go",dependency)];
    files.extend(names.iter().map(|p| (p.as_str(),blob.as_slice())));
    let base = f.pin_bytes(&repo,"app",&"a".repeat(40),&files);
    files[1].1 = after;
    let head = f.pin_bytes(&repo,"app",&"b".repeat(40),&files);
    let packet = f.run(&request(&repo,vec![base.clone()],vec![head.clone()]));
    assert_no_false_go_failure(&packet);
    assert!(has(&packet,"head",".Use"));
    assert!(has(&packet,"head",".Caller"));
    assert!(!has(&packet,"head",".Value"));
    assert_eq!(packet["coverage"]["analysis_diagnostics"]["head"]["go"]["dependency_package_instances"],1);
    let warm = f.run(&request(&repo,vec![base],vec![head]));
    assert_eq!(warm["analysis_cache"]["base_hit"],true);
    assert_eq!(warm["analysis_cache"]["head_hit"],true);
    assert_eq!(warm["packet_id"],packet["packet_id"]);
    // An arbitrary folder named vendor cannot use the larger allowance.
    files.retain(|(path,_)| *path != "vendor/modules.txt");
    let unmarked = f.pin_bytes(&repo,"app",&"c".repeat(40),&files);
    let error = review::run(&f.store,&f.workspace,&f.token,
        &request(&repo,vec![unmarked.clone()],vec![unmarked])).unwrap_err();
    assert!(error.to_string().contains("64 MiB of repository source"), "{error}");
}
