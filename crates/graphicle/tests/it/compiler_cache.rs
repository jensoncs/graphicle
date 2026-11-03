use super::*;

const DEP_MOD: &str = "module example/tax\ngo 1.23\nrequire example/dep v1.0.0\n";
const DEP_LIST: &str = "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\n";

fn dependency_pin(f: &Fixture, repo: &str, commit: char, code: &str) -> Value {
    f.pin(
        repo,
        "app",
        commit,
        &[
            ("go.mod", DEP_MOD),
            ("vendor/modules.txt", DEP_LIST),
            (
                "vendor/example/dep/dep.go",
                "package dep\ntype T struct{}\nfunc (T) F() int{return 1}\n",
            ),
            ("main.go", code),
        ],
    )
}

fn comparable(mut packet: Value) -> Value {
    packet.as_object_mut().unwrap().remove("analysis_cache");
    packet.as_object_mut().unwrap().remove("tokens");
    packet
}

#[test]
fn dependency_exports_survive_owned_edits_and_match_fresh_analysis() {
    let f = Fixture::new();
    let repo = f.repo("dependency-exports");
    let old = "package tax\nimport \"example/dep\"\nfunc F() int {return dep.T{}.F()}\n";
    let new = old.replace("return dep", "return 2+dep");
    let base = dependency_pin(&f, &repo, 'a', old);
    let head = dependency_pin(&f, &repo, 'b', &new);
    let args = request(&repo, vec![base.clone()], vec![head]);
    let first = f.run(&args);
    assert_eq!(first["analysis_cache"]["packages"]["base"]["misses"], 1);
    assert_eq!(first["analysis_cache"]["packages"]["head"]["hits"], 1);
    assert_eq!(first["analysis_cache"]["head_hit"], false);
    clear_facts(&f);
    let warm = f.run(&args);
    assert_eq!(warm["analysis_cache"]["packages"]["base"]["hits"], 1);
    assert_eq!(warm["analysis_cache"]["packages"]["head"]["hits"], 1);
    assert_eq!(comparable(first), comparable(warm));
    let third = dependency_pin(&f, &repo, 'c', &new.replace("2+dep", "3+dep"));
    let next = f.run(&request(&repo, vec![base.clone()], vec![third]));
    assert_eq!(next["analysis_cache"]["head_hit"], false);
    assert_eq!(next["analysis_cache"]["packages"]["head"]["hits"], 1);
    assert!(review::run(&f.store, &f.workspace, "revoked", &args).is_err());
    f.store
        .transaction(|db| {
            db.execute(
                "DELETE FROM repo_snapshots WHERE repository_id=? AND sha256=?",
                rusqlite::params![repo, s(&base, "snapshot")],
            )?;
            let count: usize =
                db.query_row("SELECT count(*) FROM repo_review_package_facts", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn incomplete_owned_package_reuses_valid_dependencies_and_repairs_corruption() {
    let f = Fixture::new();
    let repo = f.repo("partial-dependency-exports");
    let code = "package tax\nimport \"example/dep\"\nfunc F() int {return dep.T{}.F()+missing}\n";
    let pin = dependency_pin(&f, &repo, 'a', code);
    let args = request(&repo, vec![pin.clone()], vec![pin]);
    let first = f.run(&args);
    assert_eq!(first["impact"], "incomplete");
    clear_facts(&f);
    let warm = f.run(&args);
    assert_eq!(warm["analysis_cache"]["base_hit"], false);
    assert_eq!(warm["analysis_cache"]["packages"]["base"]["hits"], 1);
    assert_eq!(comparable(first.clone()), comparable(warm));
    // Matching checksum exercises native export rejection, not only DB integrity.
    f.store
        .transaction(|db| {
            let data = "bm90IGV4cG9ydCBkYXRh";
            db.execute(
                "UPDATE repo_review_package_facts SET data=?,sha256=?",
                rusqlite::params![data, text::sha256(data.as_bytes())],
            )?;
            Ok(())
        })
        .unwrap();
    clear_facts(&f);
    let repaired = f.run(&args);
    assert_eq!(
        repaired["analysis_cache"]["packages"]["base"]["rejected"],
        1
    );
    assert_eq!(comparable(first.clone()), comparable(repaired));
    clear_facts(&f);
    assert_eq!(
        f.run(&args)["analysis_cache"]["packages"]["base"]["hits"],
        1
    );
    f.store
        .transaction(|db| {
            db.execute("UPDATE repo_review_package_facts SET sha256='corrupt'", [])?;
            Ok(())
        })
        .unwrap();
    clear_facts(&f);
    let checksum = f.run(&args);
    assert_eq!(checksum["analysis_cache"]["packages"]["base"]["hits"], 0);
    assert_eq!(comparable(first), comparable(checksum));
}

fn clear_facts(f: &Fixture) {
    f.store
        .transaction(|db| {
            db.execute("DELETE FROM repo_review_analysis", [])?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn later_configuration_edits_reuse_go_facts_with_fresh_report_bindings() {
    let f = Fixture::new();
    let repo = f.repo("compiler-input-reuse");
    let old = "package tax\nfunc F() int { return 1 }\n";
    let new = "package tax\nfunc F() int { return 2 }\n";
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("code.go", old),
            ("deploy.yaml", "replicas: 1\n"),
        ],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("code.go", new),
            ("deploy.yaml", "replicas: 1\n"),
        ],
    );
    let first = f.run(&request(&repo, vec![base.clone()], vec![head.clone()]));
    assert_eq!(first["analysis_cache"]["head_hit"], false);
    let next = f.pin(
        &repo,
        "app",
        'c',
        &[
            ("go.mod", MOD),
            ("code.go", new),
            ("deploy.yaml", "replicas: 2\n"),
        ],
    );
    assert_ne!(next["snapshot"], head["snapshot"]);
    let args = request(&repo, vec![base], vec![next.clone()]);
    let mut reused = f.run(&args);
    assert_eq!(reused["analysis_cache"]["base_hit"], true);
    assert_eq!(reused["analysis_cache"]["head_hit"], true);
    assert_eq!(reused["head"][0]["snapshot"], next["snapshot"]);
    assert_ne!(reused["packet_id"], first["packet_id"]);
    assert!(reused["file_changes"].to_string().contains("deploy.yaml"));
    assert!(has(&reused, "head", "F"));

    // Independent compiler execution must produce the same complete packet,
    // including the new configuration content, spans, pins and package instances.
    clear_facts(&f);
    let mut cold = f.run(&args);
    assert_eq!(cold["analysis_cache"]["base_hit"], false);
    assert_eq!(cold["analysis_cache"]["head_hit"], false);
    for packet in [&mut reused, &mut cold] {
        packet.as_object_mut().unwrap().remove("analysis_cache");
        packet.as_object_mut().unwrap().remove("tokens");
    }
    assert_eq!(reused, cold);
}

#[test]
fn cold_documentation_review_compiles_once_but_rechecks_current_inputs() {
    let f = Fixture::new();
    let repo = f.repo("cold-docs-reuse");
    let code = "package tax\nfunc F() int { return 1 }\n";
    let files = [("go.mod", MOD), ("code.go", code), ("README.md", "one")];
    let base = f.pin(&repo, "app", 'a', &files);
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("go.mod", MOD), ("code.go", code), ("README.md", "two")],
    );
    let args = request(&repo, vec![base.clone()], vec![head]);
    let packet = f.run(&args);
    assert_eq!(packet["analysis_cache"]["base_hit"], false);
    assert_eq!(packet["analysis_cache"]["head_hit"], true);
    assert_eq!(packet["coverage"]["complete"], true);
    assert!(packet["entries"].as_array().unwrap().is_empty());
    assert!(review::run(&f.store, &f.workspace, "revoked", &args).is_err());
    f.store
        .transaction(|db| {
            // Both snapshots bind the reused fact set. Forgetting either evicts it.
            let bindings: usize = db.query_row(
                "SELECT count(*) FROM repo_review_analysis_inputs",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(bindings, 2);
            db.execute(
                "DELETE FROM repo_snapshots WHERE repository_id=? AND sha256=?",
                rusqlite::params![repo, s(&base, "snapshot")],
            )?;
            let facts: usize =
                db.query_row("SELECT count(*) FROM repo_review_analysis", [], |r| {
                    r.get(0)
                })?;
            assert_eq!(facts, 0);
            Ok(())
        })
        .unwrap();
    let missing = f.run(&args);
    assert_eq!(missing["impact"], "incomplete");
    assert_eq!(missing["analysis_cache"]["base_hit"], false);
}

#[test]
fn embed_content_and_directory_membership_invalidate_compiler_facts() {
    let f = Fixture::new();
    let repo = f.repo("embed-cache-invalidation");
    let code = "package tax\nimport \"embed\"\n//go:embed assets\nvar Assets embed.FS\nfunc F() int { return 1 }\n";
    let original = [
        ("go.mod", MOD),
        ("code.go", code),
        ("assets/settings.json", "{}"),
    ];
    let base = f.pin(&repo, "app", 'a', &original);
    f.run(&request(&repo, vec![base.clone()], vec![base.clone()]));
    for (commit, files) in [
        (
            'b',
            vec![
                ("go.mod", MOD),
                ("code.go", code),
                ("assets/settings.json", "{\"changed\":true}"),
            ],
        ),
        (
            'c',
            vec![
                ("go.mod", MOD),
                ("code.go", code),
                ("assets/settings.json", "{}"),
                ("assets/extra.txt", "added"),
            ],
        ),
        (
            'd',
            vec![
                ("go.mod", MOD),
                ("code.go", code),
                ("assets/extra.txt", "replaced"),
            ],
        ),
    ] {
        let head = f.pin(&repo, "app", commit, &files);
        let args = request(&repo, vec![base.clone()], vec![head]);
        let packet = f.run(&args);
        assert_eq!(packet["analysis_cache"]["head_hit"], false, "{commit}");
        assert!(
            packet["file_changes"]
                .to_string()
                .contains("embedded_input")
        );
        assert_eq!(f.run(&args)["analysis_cache"]["head_hit"], true);
    }
}

#[test]
fn discovery_inventory_invalidates_even_for_documentation_paths() {
    let f = Fixture::new();
    let repo = f.repo("inventory-cache-invalidation");
    let code = "package tax\nfunc F() int { return 1 }\n";
    let base = f.pin(&repo, "app", 'a', &[("go.mod", MOD), ("code.go", code)]);
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("go.mod", MOD), ("code.go", code), ("README.md", "added")],
    );
    let args = request(&repo, vec![base], vec![head]);
    let packet = f.run(&args);
    assert_eq!(packet["analysis_cache"]["base_hit"], false);
    assert_eq!(packet["analysis_cache"]["head_hit"], false);
}

#[test]
fn custom_workspace_filename_is_a_compiler_input() {
    let f = Fixture::new();
    let repo = f.repo("custom-workspace-cache");
    let files = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 1 }\n"),
        ("sub/go.mod", "module example/sub\ngo 1.23\n"),
        ("sub/code.go", "package sub\nfunc F() int { return 2 }\n"),
        ("workspace.json", "go 1.23\nuse (\n .\n ./sub\n)\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let mut args = request(&repo, vec![base.clone()], vec![base]);
    args["profile"]["workspace"] = json!("app/workspace.json");
    let first = f.run(&args);
    // The JSON suffix has its own unsupported-source qualification. The Go
    // world nevertheless succeeds and is eligible for compiler-fact reuse.
    assert_eq!(
        first["coverage"]["analysis_diagnostics"]["head"]["go"]["owned_package_instances"],
        2
    );
    assert_eq!(first["analysis_cache"]["head_hit"], true);
    let changed: Vec<_> = files
        .iter()
        .map(|(path, text)| {
            (
                *path,
                if *path == "workspace.json" {
                    "go 1.23\nuse .\n"
                } else {
                    *text
                },
            )
        })
        .collect();
    args["head"] = json!([f.pin(&repo, "app", 'b', &changed)]);
    let packet = f.run(&args);
    assert_eq!(packet["analysis_cache"]["base_hit"], true);
    assert_eq!(packet["analysis_cache"]["head_hit"], false);
    assert_eq!(packet["coverage"]["complete"], false);
    let go = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(go["owned_package_instances"], 1);
    assert!(
        go["examples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|example| example["cause"] == "discovery_error"
                && example["consumer_build"] == "app/sub")
    );
}
