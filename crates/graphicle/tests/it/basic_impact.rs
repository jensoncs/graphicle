use super::*;

#[test]
fn basic_languages_use_authorized_pins_and_revision_safe_packets() {
    let f = Fixture::new();
    let repo = f.repo("basic");
    let initial = [
        ("infra/main.tf", "output \"answer\" { value = 1 }\n"),
        (
            "chart/Chart.yaml",
            "apiVersion: v2\nname: app\nversion: 0.1.0\n",
        ),
        ("chart/values.yaml", "replicas: 1\n"),
        (
            "chart/templates/deploy.yaml",
            "kind: Deployment\nreplicas: {{ .Values.replicas }}\n",
        ),
        ("scripts/lib.sh", "work() { echo 1; }\n"),
        ("scripts/run.sh", "source ./lib.sh\nwork\n"),
        ("tax.py", "def pay(): return 1\n"),
        (
            "caller.py",
            "from tax import pay\ndef run(): return pay()\n",
        ),
        ("src/lib.rs", "mod tax;\npub fn run() { tax::pay(); }\n"),
        ("src/tax.rs", "pub fn pay() -> i32 { 1 }\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &initial);
    let changed = initial.map(|(p, t)| (p, t.replace("1", "2")));
    let files: Vec<_> = changed.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let head = f.pin(&repo, "app", 'b', &files);
    let mut args = request(&repo, vec![base.clone()], vec![head.clone()]);
    args["max_tokens"] = json!(24000);
    let first = f.run(&args);
    assert_eq!(first["coverage"]["complete"], false);
    assert_eq!(first["impact"], "incomplete");
    for lang in ["terraform", "helm", "bash", "python", "rust"] {
        let entries: Vec<_> = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["side"] == "head" && e["language"] == lang)
            .collect();
        assert!(!entries.is_empty(), "missing {lang}: {first}");
        for entry in entries {
            assert_eq!(entry["repository_id"], repo);
            assert_eq!(entry["analysis_level"], "basic_file");
            assert_eq!(
                entry["file_sha256"],
                text::sha256(s(entry, "text").as_bytes())
            );
            assert_ne!(entry["review_basis"]["complete"], true);
        }
    }
    assert_eq!(first["head"][0]["snapshot"], head["snapshot"]);
    assert!(
        !first["coverage"]["issues"]
            .to_string()
            .contains("unsupported_source_language_or_contract")
    );
    assert!(
        !first["coverage"]["issues"]
            .to_string()
            .contains("compiler_analysis_unavailable")
    );
    args["previous_packet_id"] = first["packet_id"].clone();
    let repeated = f.run(&args);
    assert!(!repeated["unchanged"].as_array().unwrap().is_empty());
    assert!(repeated["entries"].as_array().unwrap().is_empty());
    let mut third = files.clone();
    third.retain(|(p, _)| *p != "tax.py");
    let next = f.pin(&repo, "app", 'c', &third);
    args["head"] = json!([next]);
    let delta = f.run(&args);
    let old = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && e["file"] == "tax.py")
        .unwrap();
    assert!(
        delta["invalidated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == old["id"])
    );
}

#[test]
fn basic_missing_repository_preserves_the_explicit_full_packet_fallback() {
    let f = Fixture::new();
    let repo = f.repo("visible-basic");
    let a = f.pin(&repo, "app", 'a', &[("app.py", "import hidden\n")]);
    let b = f.pin(
        &repo,
        "app",
        'b',
        &[("app.py", "import hidden\n# changed\n")],
    );
    let other = f.store.create_workspace("hidden basic").unwrap();
    let hidden = f
        .store
        .register(
            s(&other, "id"),
            s(&other, "capability"),
            &json!({"project_key":"hidden","name":"hidden"}),
        )
        .unwrap();
    let mut hidden_pin = a.clone();
    hidden_pin["repository_id"] = hidden["id"].clone();
    hidden_pin["directory"] = json!("hidden");
    let mut args = request(&repo, vec![a, hidden_pin.clone()], vec![b, hidden_pin]);
    let first = f.run(&args);
    assert!(
        first["coverage"]["issues"]
            .to_string()
            .contains("unavailable_snapshot")
    );
    assert!(
        first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["repository_id"] == repo)
    );
    args["previous_packet_id"] = first["packet_id"].clone();
    let error = review::run(&f.store, &f.workspace, &f.token, &args).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert!(error.message.contains("request a full packet"));
}

#[test]
fn go_compiler_facts_survive_mixed_basic_review() {
    let f = Fixture::new();
    let repo = f.repo("mixed-basic");
    let files = [
        ("go.mod", MOD),
        (
            "tax.go",
            "package tax\nfunc Calculate() int { return 1 }\nfunc Caller() int { return Calculate() }\n",
        ),
        ("infra/main.tf", "output \"answer\" { value = 1 }\n"),
        ("tool.py", "def tool(): return 1\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let changed = files.map(|(p, t)| (p, t.replace("return 1", "return 2")));
    let head = f.pin(
        &repo,
        "app",
        'b',
        &changed
            .iter()
            .map(|(p, t)| (*p, t.as_str()))
            .collect::<Vec<_>>(),
    );
    let r = f.run(&request(&repo, vec![base], vec![head]));
    assert!(has(&r, "head", ".Calculate"));
    assert!(has(&r, "head", ".Caller"));
    assert!(
        r["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["side"] == "head" && e["language"] == "python")
    );
}
