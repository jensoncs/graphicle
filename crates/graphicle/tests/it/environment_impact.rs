use super::*;

const DISABLED: &str = r#"{"format":"graphicle.service-boundaries.v2","inventory_complete":true,"bindings":[],"environment_scopes":[]}"#;

const CODE: &str = r#"package tax
import env "os"
func Read() string { return env.Getenv("TIMEOUT") }
func Caller() string { return Read() }
func Other() string { value, _ := env.LookupEnv("OTHER"); return value }
type fake struct{}
func (fake) Getenv(key string) string { return key }
func Shadowed() string { os := fake{}; return os.Getenv("TIMEOUT") }
"#;
const DEPLOY: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app\n  namespace: company\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n          image: app:v1\n          env:\n            - name: TIMEOUT\n              value: \"10\"\n            - name: OTHER\n              value: \"unchanged\"\n";
fn manifest(directory: Option<&str>) -> String {
    let mut deployment = json!({"file":"deployment.yaml","container":"app"});
    if let Some(directory) = directory {
        deployment["directory"] = json!(directory);
    }
    json!({"format":"graphicle.service-boundaries.v2","inventory_complete":true,"bindings":[],
        "environment_scopes":[{"id":"app-env","service":{"namespace":"company","name":"app","environment":"prod","configuration":"deployment-v1"},"module":".","deployment":deployment}]}).to_string()
}
fn pin(
    f: &Fixture,
    repo: &str,
    commit: char,
    code: &str,
    deployment: &str,
    manifest: Option<&str>,
) -> Value {
    let mut files = vec![
        ("go.mod", MOD),
        ("service.go", code),
        ("deployment.yaml", deployment),
    ];
    if let Some(m) = manifest {
        files.push(("graphicle-services.json", m));
    }
    f.pin(repo, "app", commit, &files)
}
fn run(f: &Fixture, repo: &str, base: Value, head: Value) -> Value {
    let mut args = request(repo, vec![base], vec![head]);
    args["max_tokens"] = json!(64000);
    f.run(&args)
}
fn issue(packet: &Value, reason: &str) -> bool {
    packet["coverage"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == reason)
}

#[test]
fn literal_environment_change_selects_consumers_and_measures_reduction() {
    let f = Fixture::new();
    let repo = f.repo("environment");
    let mut code = CODE.to_owned();
    for i in 0..100 {
        code.push_str(&format!("func Unrelated{i}() int {{ return {i} }}\n"));
    }
    let changed = DEPLOY.replace("\"10\"", "\"20\"");
    let baseline = run(
        &f,
        &repo,
        pin(&f, &repo, 'a', &code, DEPLOY, Some(DISABLED)),
        pin(&f, &repo, 'b', &code, &changed, Some(DISABLED)),
    );
    let m = manifest(None);
    let base = pin(&f, &repo, 'c', &code, DEPLOY, Some(&m));
    let head = pin(&f, &repo, 'd', &code, &changed, Some(&m));
    let scoped = run(&f, &repo, base.clone(), head.clone());
    assert!(has(&scoped, "head", ".Read"));
    assert!(has(&scoped, "head", ".Caller"));
    for name in [".Other", ".Shadowed", ".Getenv", ".Unrelated0"] {
        assert!(!has(&scoped, "head", name), "unexpected {name}: {scoped}");
    }
    for i in 0..100 {
        assert!(!has(&scoped, "head", &format!(".Unrelated{i}")));
    }
    assert!(!issue(&scoped, "uncertain_file_scope"), "{scoped}");
    assert!(issue(&baseline, "uncertain_file_scope"));
    assert!(issue(&scoped, "environment_analysis_limitations"));
    let stats = &scoped["service_graph"]["head"]["environment"]["scopes"][0];
    assert_eq!(stats["literal_reads"], 2);
    assert_eq!(stats["resolved_reads"], 2);
    assert_eq!(stats["narrowing_eligible"], true);
    // The changed deployment's own nodes are not candidates being narrowed.
    let candidates = |p: &Value| {
        p["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["file"] != "deployment.yaml")
            .count()
            + p["diffs"].as_array().unwrap().len()
            + p["coverage"]["budget_omitted"].as_u64().unwrap() as usize
    };
    let before = candidates(&baseline);
    let after = candidates(&scoped);
    assert!(
        before >= 200 && after < 10,
        "before={before} after={after}: {:?}",
        scoped["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (&e["side"], &e["file"], &e["name"]))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "environment probe: entry/diff candidates {before} -> {after}; direct consumers found 2/2; unrelated functions retained 0/100; literal bindings resolved 2/2"
    );
    let reader = scoped["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && s(e, "name").ends_with(".Read"))
        .unwrap();
    assert!(reader["reasons"].to_string().contains("reads_environment"));
    assert!(reader["reasons"].to_string().contains("go/types:os.Getenv"));
    let input = scoped["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && e["environment_input"]["key"] == "TIMEOUT")
        .unwrap();
    assert!(s(input, "text").contains("name: TIMEOUT"));
    assert!(!s(input, "text").contains("name: OTHER"));
    assert_eq!(scoped["coverage"]["budget_omitted"], 0);
    eprintln!(
        "environment probe: budget omissions {} -> {}",
        baseline["coverage"]["budget_omitted"], scoped["coverage"]["budget_omitted"]
    );

    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(64000);
    args["previous_packet_id"] = scoped["packet_id"].clone();
    let repeated = f.run(&args);
    assert!(repeated["entries"].as_array().unwrap().is_empty());
    let next = pin(
        &f,
        &repo,
        'e',
        &code,
        &DEPLOY.replace("\"10\"", "\"30\""),
        Some(&m),
    );
    args["head"] = json!([next]);
    let updated = f.run(&args);
    assert!(
        updated["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["environment_input"]["key"] == "TIMEOUT" && e["side"] == "head")
    );
}

#[test]
fn uncertainty_preserves_conservative_scope() {
    let f = Fixture::new();
    let repo = f.repo("environment-uncertain");
    let m = manifest(None);
    for (extra, changed, reason) in [
        (
            "func Callback(read func(string) string) string { return read(\"TIMEOUT\") }\n",
            DEPLOY.replace("\"10\"", "\"20\""),
            "environment_indirect_consumer_unresolved",
        ),
        (
            "func Unbound() string { return env.Getenv(\"MISSING\") }\n",
            DEPLOY.replace("\"10\"", "\"20\""),
            "environment_read_unbound",
        ),
        (
            "func Dynamic(k string) string { return env.Getenv(k) }\n",
            DEPLOY.replace("\"10\"", "\"20\""),
            "environment_dynamic_read",
        ),
        (
            "func Override() { _ = env.Setenv(\"TIMEOUT\", \"99\") }\n",
            DEPLOY.replace("\"10\"", "\"20\""),
            "environment_runtime_override",
        ),
        (
            "func All() []string { return env.Environ() }\n",
            DEPLOY.replace("\"10\"", "\"20\""),
            "environment_dynamic_read",
        ),
        (
            "",
            DEPLOY.replace(
                "value: \"10\"",
                "valueFrom: {configMapKeyRef: {name: settings, key: timeout}}",
            ),
            "environment_value_from_unresolved",
        ),
        (
            "",
            DEPLOY.replace("value: \"10\"", "value: \"$(OTHER)\""),
            "environment_value_unresolved",
        ),
        (
            "",
            DEPLOY.replace("namespace: company", "namespace: elsewhere"),
            "environment_namespace_mismatch",
        ),
    ] {
        let code = format!("{CODE}{extra}");
        let packet = run(
            &f,
            &repo,
            pin(&f, &repo, 'a', &code, DEPLOY, Some(&m)),
            pin(&f, &repo, 'b', &code, &changed, Some(&m)),
        );
        assert!(issue(&packet, reason), "{reason}: {packet}");
        if reason == "environment_value_from_unresolved" {
            assert!(
                packet["service_graph"]["head"]["environment"]["scopes"][0]["resolved_reads"]
                    .is_null()
            );
        }

        assert!(issue(&packet, "uncertain_file_scope"), "{reason}: {packet}");
        assert!(has(&packet, "head", ".Other"));
        assert_eq!(packet["impact"], "incomplete");
    }
    for changed in [
        DEPLOY.replace("app:v1", "app:v2"),
        DEPLOY.replace("name: TIMEOUT", "name: NEW_KEY"),
    ] {
        let packet = run(
            &f,
            &repo,
            pin(&f, &repo, 'a', CODE, DEPLOY, Some(&m)),
            pin(&f, &repo, 'b', CODE, &changed, Some(&m)),
        );
        assert!(issue(&packet, "uncertain_file_scope"));
    }
}

#[test]
fn deployment_in_another_pin_is_authorized_and_scope_is_not_inferred_from_key_names() {
    let f = Fixture::new();
    let app = f.repo("environment-app");
    let deploy = f.repo("environment-deploy");
    let other = f.repo("environment-other");
    let m = manifest(Some("deploy"));
    let code = f.pin(
        &app,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("service.go", CODE),
            ("graphicle-services.json", &m),
        ],
    );
    let unrelated = f.pin(
        &other,
        "other",
        'a',
        &[
            ("go.mod", MOD),
            ("service.go", CODE),
            ("deployment.yaml", DEPLOY),
            ("graphicle-services.json", &manifest(None)),
        ],
    );
    let base = f.pin(&deploy, "deploy", 'a', &[("deployment.yaml", DEPLOY)]);
    let head = f.pin(
        &deploy,
        "deploy",
        'b',
        &[("deployment.yaml", &DEPLOY.replace("\"10\"", "\"20\""))],
    );
    let mut args = request(
        &app,
        vec![code.clone(), unrelated.clone(), base],
        vec![code.clone(), unrelated.clone(), head],
    );
    args["max_tokens"] = json!(64000);
    let packet = f.run(&args);
    assert!(has(&packet, "head", ".Read"));
    assert!(
        !packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["repository_id"] == other)
    );
    assert!(!issue(&packet, "uncertain_file_scope"));
    let mut missing = request(&app, vec![code.clone()], vec![code.clone()]);
    missing["max_tokens"] = json!(64000);
    assert!(issue(&f.run(&missing), "environment_source_unavailable"));
    let mut ambiguous: Value = serde_json::from_str(&m).unwrap();
    let mut duplicate = ambiguous["environment_scopes"][0].clone();
    duplicate["id"] = json!("staging");
    duplicate["service"]["environment"] = json!("staging");
    ambiguous["environment_scopes"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let pin = f.pin(
        &app,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("service.go", CODE),
            ("graphicle-services.json", &ambiguous.to_string()),
        ],
    );
    let packet = run(&f, &app, code, pin);
    assert!(issue(&packet, "environment_scope_ambiguous"));
}

#[test]
fn embedded_deployment_remains_a_full_file_dependency() {
    let f = Fixture::new();
    let repo = f.repo("environment-embed");
    let m = manifest(None);
    let code = CODE.replace("import env \"os\"", "import env \"os\"\nimport _ \"embed\"")
        + "//go:embed deployment.yaml\nvar configuration string\nfunc Embedded() string { return configuration }\n";
    let packet = run(
        &f,
        &repo,
        pin(&f, &repo, 'a', &code, DEPLOY, Some(&m)),
        pin(
            &f,
            &repo,
            'b',
            &code,
            &DEPLOY.replace("\"10\"", "\"20\""),
            Some(&m),
        ),
    );
    assert!(issue(&packet, "uncertain_file_scope"));
    assert!(has(&packet, "head", ".Embedded"));
}

#[test]
fn automatic_environment_analysis_needs_no_manifest_and_keeps_json_config_conservative() {
    let f = Fixture::new();
    let repo = f.repo("automatic-environment");
    let base = pin(&f, &repo, 'a', CODE, DEPLOY, None);
    let head = pin(
        &f,
        &repo,
        'b',
        CODE,
        &DEPLOY.replace("\"10\"", "\"20\""),
        None,
    );
    let packet = run(&f, &repo, base, head);
    let scope = &packet["service_graph"]["head"]["environment"]["scopes"][0];
    assert_eq!(scope["narrowing_eligible"], true, "{packet}");
    assert_eq!(scope["authority"], "same_repository_candidate");
    assert!(scope["service"].is_null());
    assert!(has(&packet, "head", ".Read"));
    assert!(has(&packet, "head", ".Caller"));
    assert!(!has(&packet, "head", ".Other"));
    assert!(!issue(&packet, "uncertain_file_scope"));
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "environment_input")
            .all(|e| e["environment_input"]["manifest"].is_null())
    );
    // Application JSON remains source context; it is not a deployment binding.
    let pins = ["10", "20"].map(|value| {
        f.pin(
            &repo,
            "app",
            'c',
            &[
                ("go.mod", MOD),
                ("service.go", CODE),
                ("deployment.yaml", DEPLOY),
                ("config.json", &format!("{{\"timeout\":{value}}}")),
            ],
        )
    });
    let packet = run(&f, &repo, pins[0].clone(), pins[1].clone());
    assert!(issue(&packet, "uncertain_file_scope"));
    assert!(has(&packet, "head", ".Other"));
}

#[test]
fn automatic_discovery_does_not_choose_between_workloads_or_override_manifests() {
    let f = Fixture::new();
    let repo = f.repo("automatic-ambiguous");
    for manifest in [None, Some("invalid manifest"), Some(DISABLED)] {
        let sides = [DEPLOY.to_owned(), DEPLOY.replace("\"10\"", "\"20\"")].map(|deployment| {
            let mut files = vec![
                ("go.mod", MOD),
                ("service.go", CODE),
                ("deployment.yaml", deployment.as_str()),
            ];
            if let Some(manifest) = manifest {
                files.push(("graphicle-services.json", manifest));
            } else {
                files.push(("staging.yaml", DEPLOY));
            }
            f.pin(&repo, "app", 'a', &files)
        });
        let packet = run(&f, &repo, sides[0].clone(), sides[1].clone());
        assert!(
            packet["service_graph"]["head"]["environment"]["scopes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(issue(&packet, "uncertain_file_scope"));
        assert!(has(&packet, "head", ".Other"));
        if manifest.is_none() {
            assert!(issue(&packet, "environment_auto_scope_ambiguous"));
        }
    }
}
