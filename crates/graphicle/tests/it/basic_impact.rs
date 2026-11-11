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
        assert!(
            entries.iter().any(|e| e["analysis_level"] == "basic_file"),
            "{lang}"
        );
        for entry in entries {
            assert_eq!(entry["repository_id"], repo);
            // A file entry is the whole file; a declaration entry its span.
            if entry["analysis_level"] == "basic_file" {
                assert_eq!(
                    entry["file_sha256"],
                    text::sha256(s(entry, "text").as_bytes())
                );
            } else {
                assert_eq!(entry["analysis_level"], "basic_symbol", "{entry}");
                assert!(entry["byte_end"].as_u64() > entry["byte_start"].as_u64());
            }
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

/// Under a tight budget the callers of a changed function reach the packet
/// before files that only import it. Prints the counts for comparison.
#[test]
fn resolved_callers_reach_a_tight_packet_before_importers() {
    let f = Fixture::new();
    let repo = f.repo("callers");
    let filler: String = (0..120)
        .map(|i| format!("export const unrelated{i} = {i} * 2;\n"))
        .collect();
    let files = |body: &str| -> Vec<(String, String)> {
        let mut files = vec![(
            "src/price.ts".to_string(),
            format!("export function price(): number {{\n  return {body};\n}}\n"),
        )];
        for name in ["cart", "checkout"] {
            files.push((
                format!("src/{name}.ts"),
                format!("import {{ price }} from './price';\n{filler}export function {name}Total(): number {{\n  return price() + 1;\n}}\n"),
            ));
        }
        for i in 0..20 {
            files.push((
                format!("src/view{i}.ts"),
                format!(
                    "import {{ price }} from './price';\n{filler}export const handler{i} = price;\n"
                ),
            ));
        }
        files
    };
    let pin = |rev: char, body: &str| {
        let owned = files(body);
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        f.pin(&repo, "app", rev, &borrowed)
    };
    let (base, head) = (pin('a', "1"), pin('b', "2"));
    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(6000);
    let packet = f.run(&args);
    let entries = packet["entries"].as_array().unwrap();
    let delivered = |file: &str| {
        entries
            .iter()
            .any(|e| e["side"] == "head" && e["file"] == file)
    };
    let callers = ["src/cart.ts", "src/checkout.ts"]
        .iter()
        .filter(|f| delivered(f))
        .count();
    let importers = (0..20)
        .filter(|i| delivered(&format!("src/view{i}.ts")))
        .count();
    eprintln!(
        "CALLERS delivered={callers}/2 importers={importers}/20 coverage.callers={} tokens={}",
        packet["coverage"]["callers"],
        text::count(&packet.to_string())
    );
    assert_eq!(callers, 2, "{}", packet["coverage"]);
    assert_eq!(
        packet["coverage"]["callers"]["required"], 2,
        "{}",
        packet["coverage"]["callers"]
    );
    assert_eq!(packet["coverage"]["callers"]["selected"], 2);
    assert_eq!(packet["impact"], "incomplete");
}
