use super::*;

fn attest(f: &Fixture, pin: &Value, files: &[(&str, &str)], paths: Value) {
    let repo = s(pin, "repository_id");
    let state = f
        .store
        .project_status(
            &f.workspace,
            &f.token,
            &json!({"repository_id":repo,"compact":true}),
        )
        .unwrap();
    let prepared = f.store.sync_prepare(&f.workspace, &f.token, &json!({
        "repository_id":repo,"expected_snapshot":state["repository"]["snapshot"],
        "manifest":files.iter().map(|(p,c)|json!({"path":p,"sha256":text::sha256(c.as_bytes()),"bytes":c.len()})).collect::<Vec<_>>(),
        "metadata":{"commit":pin["commit"],"policy_sha256":pin["policy_sha256"],"scope_complete":false,"withheld_paths":paths}
    })).unwrap();
    if !prepared["sync_id"].is_null() {
        f.store
            .sync_commit(
                &f.workspace,
                &f.token,
                &json!({"repository_id":repo,"sync_id":prepared["sync_id"]}),
            )
            .unwrap();
    }
}

fn admission_issue(packet: &Value) -> bool {
    packet["coverage"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == "source_admission_incomplete")
}

#[test]
fn exhaustive_credential_omissions_clear_only_the_admission_gap() {
    let f = Fixture::new();
    let repo = f.repo("withheld");
    let old = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 1 }\n"),
    ];
    let new = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 2 }\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &old);
    let head = f.pin(&repo, "app", 'b', &new);
    attest(&f, &base, &old, Value::Null);
    attest(&f, &head, &new, Value::Null);
    let args = request(&repo, vec![base.clone()], vec![head.clone()]);
    let legacy = f.run(&args);
    assert!(admission_issue(&legacy));
    assert_eq!(legacy["impact"], "incomplete");
    for (pin, files) in [(&base, &old), (&head, &new)] {
        attest(
            &f,
            pin,
            files,
            json!([
                "private/credentials.json",
                ".env.production",
                "tls/client.key"
            ]),
        );
    }
    let improved = f.run(&args);
    assert!(!admission_issue(&improved), "{improved}");
    assert_eq!(improved["impact"], "potential_impact");
    assert_eq!(
        improved["coverage"]["source_admission"]["head"][0]["outside_build_count"],
        3
    );
    assert_eq!(
        improved["analysis_cache"]["head_hit"], true,
        "attestation must be assessed afresh with cached compiler facts"
    );

    for path in [
        "unrelated/new_consumer.go",
        "vendor/modules.txt",
        "nested/go.mod",
        "data/unknown.json",
        "private/.env.go",
    ] {
        attest(&f, &head, &new, json!([path]));
        let packet = f.run(&args);
        assert!(admission_issue(&packet), "{path}: {packet}");
        assert_eq!(packet["impact"], "incomplete");
    }
    attest(&f, &head, &new, json!([]));
    assert_eq!(f.run(&args)["impact"], "potential_impact");
}

#[test]
fn withheld_embedded_credentials_remain_incomplete() {
    let f = Fixture::new();
    let repo = f.repo("withheld-embed");
    let files = [
        ("go.mod", MOD),
        (
            "code.go",
            "package tax\nimport \"embed\"\n//go:embed assets/*\nvar Assets embed.FS\nfunc F() int { return 1 }\n",
        ),
        ("assets/public.txt", "public"),
    ];
    let pin = f.pin(&repo, "app", 'a', &files);
    attest(&f, &pin, &files, json!(["assets/token.key"]));
    let packet = f.run(&request(&repo, vec![pin.clone()], vec![pin]));
    assert!(admission_issue(&packet), "{packet}");
    assert_eq!(
        packet["coverage"]["source_admission"]["head"][0]["unresolved_count"],
        1
    );
    assert_eq!(packet["impact"], "incomplete");
}

#[test]
fn omission_attestations_are_bound_to_commit_and_policy() {
    let f = Fixture::new();
    let repo = f.repo("withheld-bindings");
    let files = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 1 }\n"),
    ];
    let a = f.pin(&repo, "app", 'a', &files);
    let b = f.pin(&repo, "app", 'b', &files);
    let mut policy_b = b.clone();
    policy_b["policy_sha256"] = json!(text::sha256(b"different policy"));
    attest(&f, &a, &files, json!(["credentials.json"]));
    attest(&f, &b, &files, json!(["missing.go"]));
    attest(&f, &policy_b, &files, json!(["credentials.json"]));
    for (pin, expected) in [
        (a, "no_changes_in_scope"),
        (b, "incomplete"),
        (policy_b, "no_changes_in_scope"),
    ] {
        let packet = f.run(&request(&repo, vec![pin.clone()], vec![pin]));
        assert_eq!(packet["impact"], expected);
    }
}

#[test]
fn invalid_omission_attestations_are_rejected_before_admission() {
    let f = Fixture::new();
    let repo = f.repo("withheld-validation");
    let manifest = json!([{"path":"code.go","sha256":text::sha256(b"package tax"),"bytes":11}]);
    for paths in [
        json!("credentials.json"),
        json!([4]),
        json!(["../escape"]),
        json!(["/absolute"]),
        json!(["x", "x"]),
        json!(["code.go"]),
    ] {
        assert!(f.store.sync_prepare(&f.workspace,&f.token,&json!({"repository_id":repo,"expected_snapshot":"","manifest":manifest,"metadata":{"scope_complete":false,"withheld_paths":paths}})).is_err(), "{paths}");
    }
    assert!(f.store.sync_prepare(&f.workspace,&f.token,&json!({"repository_id":repo,"expected_snapshot":"","manifest":manifest,"metadata":{"scope_complete":true,"withheld_paths":["credentials.json"]}})).is_err());
    // An overlap arriving in a later part must check the initial attestation.
    let first = f.store.sync_prepare(&f.workspace,&f.token,&json!({"repository_id":repo,"expected_snapshot":"","manifest":[],"manifest_complete":false,"metadata":{"scope_complete":false,"withheld_paths":["code.go"]}})).unwrap();
    assert!(
        f.store
            .sync_prepare(
                &f.workspace,
                &f.token,
                &json!({"repository_id":repo,"sync_id":first["sync_id"],"manifest":manifest})
            )
            .is_err()
    );
}

#[test]
fn old_binding_schema_migrates_without_claiming_exhaustive_omissions() {
    let f = Fixture::new();
    let repo = f.repo("withheld-migration");
    let files = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 1 }\n"),
    ];
    let pin = f.pin(&repo, "app", 'a', &files);
    attest(&f, &pin, &files, Value::Null);
    let db = rusqlite::Connection::open(f._dir.path().join("reviews.sqlite3")).unwrap();
    db.execute_batch("DROP TRIGGER repo_scope_resets_withheld; ALTER TABLE repo_snapshot_bindings DROP COLUMN withheld_paths")
        .unwrap();
    drop(db);
    let reopened = Store::open(f._dir.path()).unwrap();
    let packet = review::run(
        &reopened,
        &f.workspace,
        &f.token,
        &request(&repo, vec![pin.clone()], vec![pin]),
    )
    .unwrap();
    assert!(admission_issue(&packet));
    assert_eq!(packet["coverage"]["source_admission"]["head"], json!([]));
    assert_eq!(packet["impact"], "incomplete");
}

#[test]
fn external_package_failures_are_distinct_from_local_discovery_and_type_errors() {
    let f = Fixture::new();
    let repo = f.repo("outside-modules");
    let files = [
        ("go.mod", MOD),
        (
            "code.go",
            "package tax\nimport \"unavailable.invalid/dependency\"\nfunc F() int { return dependency.Value }\n",
        ),
        (
            "healthy/code.go",
            "package healthy\nfunc F() int { return 1 }\n",
        ),
    ];
    let pin = f.pin(&repo, "app", 'a', &files);
    let packet = f.run(&request(&repo, vec![pin.clone()], vec![pin]));
    let reasons: Vec<_> = packet["coverage"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| s(i, "reason"))
        .collect();
    assert!(
        reasons.contains(&"unavailable_external_package:unavailable.invalid/dependency"),
        "{reasons:?}"
    );
    assert!(
        !reasons.contains(&"incomplete_discovery:example/tax"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"type_check_failed:example/tax"),
        "{reasons:?}"
    );
    assert!(
        !reasons.iter().any(|r| r.ends_with(":example/tax/healthy")),
        "{reasons:?}"
    );
    assert_eq!(packet["impact"], "incomplete");
    let summary = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(summary["incomplete_package_instances"], 1);
    assert_eq!(summary["unavailable_external_package_instances"], 1);
    assert!(
        summary["examples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["cause"] == "outside_snapshot")
    );
}

#[test]
fn diagnostic_summary_separates_one_broken_package_from_many_references() {
    let f = Fixture::new();
    let repo = f.repo("diagnostic-amplification");
    let code = format!(
        "package tax\n{}",
        (0..1000)
            .map(|i| format!("func F{i}() {{ MissingImplementation() }}\n"))
            .collect::<String>()
    );
    let pin = f.pin(&repo, "app", 'a', &[("go.mod", MOD), ("code.go", &code)]);
    let args = request(&repo, vec![pin.clone()], vec![pin]);
    let packet = f.run(&args);
    let summary = &packet["coverage"]["analysis_diagnostics"]["head"]["go"];
    assert_eq!(summary["incomplete_package_instances"], 1);
    assert_eq!(summary["type_check_failed_package_instances"], 1);
    assert_eq!(summary["incomplete_package_references"], 1000);
    assert!(
        summary["examples"]
            .to_string()
            .contains("undefined: MissingImplementation"),
        "{summary}"
    );
    assert!(summary["examples"].as_array().unwrap().len() <= 8);
    assert_eq!(packet["impact"], "incomplete");
    let cached = f.run(&args);
    assert_eq!(cached["analysis_cache"]["head_hit"], true);
    assert_eq!(
        cached["coverage"]["analysis_diagnostics"],
        packet["coverage"]["analysis_diagnostics"]
    );
}

#[test]
fn old_writer_cannot_preserve_a_stale_exhaustive_list() {
    let f = Fixture::new();
    let repo = f.repo("withheld-old-writer");
    let files = [
        ("go.mod", MOD),
        ("code.go", "package tax\nfunc F() int { return 1 }\n"),
    ];
    let pin = f.pin(&repo, "app", 'a', &files);
    attest(&f, &pin, &files, json!(["credentials.json"]));
    let args = request(&repo, vec![pin.clone()], vec![pin.clone()]);
    assert_eq!(f.run(&args)["impact"], "no_changes_in_scope");
    let db = rusqlite::Connection::open(f._dir.path().join("reviews.sqlite3")).unwrap();
    // The pre-feature binary only updates this column, even when its value
    // stays false. This must invalidate the previous writer's exact list.
    db.execute(
        "UPDATE repo_snapshot_bindings SET scope_complete=0 WHERE repository_id=?",
        [&repo],
    )
    .unwrap();
    assert!(admission_issue(&f.run(&args)));
    attest(&f, &pin, &files, json!(["credentials.json"]));
    assert_eq!(f.run(&args)["impact"], "no_changes_in_scope");
}
