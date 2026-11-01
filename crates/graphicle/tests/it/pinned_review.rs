use graphicle_memory::text;
use graphicle_runtime::{
    review,
    store::{Store, s},
};
use serde_json::{Value, json};

#[path = "service_boundaries.rs"]
mod service_boundaries;
#[path = "basic_impact.rs"]
mod basic_impact;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    workspace: String,
    token: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("Pinned review").unwrap();
        Self {
            _dir: dir,
            store,
            workspace: s(&ws, "id").into(),
            token: s(&ws, "capability").into(),
        }
    }
    fn repo(&self, name: &str) -> String {
        s(
            &self
                .store
                .register(
                    &self.workspace,
                    &self.token,
                    &json!({"project_key":name,"name":name}),
                )
                .unwrap(),
            "id",
        )
        .into()
    }
    fn pin(&self, repo: &str, dir: &str, commit: char, files: &[(&str, &str)]) -> Value {
        self.pin_commit(repo, dir, &commit.to_string().repeat(40), files)
    }
    fn pin_commit(&self, repo: &str, dir: &str, commit: &str, files: &[(&str, &str)]) -> Value {
        self.pin_bytes(
            repo,
            dir,
            commit,
            &files
                .iter()
                .map(|(p, c)| (*p, c.as_bytes()))
                .collect::<Vec<_>>(),
        )
    }
    fn pin_bytes(&self, repo: &str, dir: &str, commit: &str, files: &[(&str, &[u8])]) -> Value {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let state = self
            .store
            .project_status(&self.workspace, &self.token, &json!({"repository_id":repo}))
            .unwrap();
        let policy = text::sha256(b"fixture source admission v1");
        let begin=self.store.sync_prepare(&self.workspace,&self.token,&json!({"repository_id":repo,"expected_snapshot":state["repository"]["snapshot"],"metadata":{"commit":commit,"policy_sha256":policy,"scope_complete":true},"manifest":files.iter().map(|(p,c)|json!({"path":p,"sha256":text::sha256(c),"bytes":c.len()})).collect::<Vec<_>>()})).unwrap();
        if !begin["sync_id"].is_null() {
            self.store.sync_upload(&self.workspace,&self.token,&json!({"repository_id":repo,"sync_id":begin["sync_id"],"files":files.iter().map(|(p,c)|json!({"path":p,"content_base64":STANDARD.encode(c)})).collect::<Vec<_>>()})).unwrap();
            self.store
                .sync_commit(
                    &self.workspace,
                    &self.token,
                    &json!({"repository_id":repo,"sync_id":begin["sync_id"]}),
                )
                .unwrap();
        }
        json!({"repository_id":repo,"directory":dir,"snapshot":begin["snapshot"],"commit":commit,"policy_sha256":policy})
    }
    fn run(&self, args: &Value) -> Value {
        review::run(&self.store, &self.workspace, &self.token, args).unwrap()
    }
}
const MOD: &str = "module example/tax\ngo 1.23\n";
fn request(repo: &str, base: Vec<Value>, head: Vec<Value>) -> Value {
    json!({"repository_id":repo,"base":base,"head":head,"profile":{"goos":"linux","goarch":"amd64","tests":false},"max_tokens":16000})
}
fn has(report: &Value, side: &str, name: &str) -> bool {
    report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["side"] == side && s(e, "name").ends_with(name))
}

fn business_history(pin: &Value, path: &str, source: &str, ranges: Value) -> Value {
    json!({"format":"graphicle.business-history.v1","pin":pin,"shallow":false,"files":[{"path":path,"file_sha256":text::sha256(source.as_bytes()),"line_count":source.lines().count(),"omitted_ranges":0,"attributions":ranges}]})
}

#[test]
fn scope_uses_compiler_profile_and_does_not_expand_documentation_edits() {
    let f = Fixture::new();
    let repo = f.repo("scope");
    let code = format!(
        "package tax\n{}",
        (0..20)
            .map(|i| format!("func F{i}() int {{ return {i} }}\n"))
            .collect::<String>()
    );
    let test = "package tax\nfunc TestHelper() int { return 1 }\n";
    let tagged = "//go:build special\n\npackage tax\nfunc Tagged() int { return 1 }\n";
    let native = "package tax\nimport \"C\"\nfunc Native() int { return 1 }\n";
    let old = [
        ("go.mod", MOD),
        ("code.go", code.as_str()),
        ("code_test.go", test),
        ("tag.go", tagged),
        ("native.go", native),
        ("README.md", "readme one\n"),
    ];
    let base = f.pin(&repo, "app", 'a', &old);
    for (case, path, content, expected) in [
        ('b', "code.go", code.replace("return 0", "return 100"), 2),
        ('c', "code_test.go", test.replace("return 1", "return 2"), 0),
        ('d', "README.md", "readme two\n".into(), 0),
        ('e', "tag.go", tagged.replace("return 1", "return 2"), 0),
        ('f', "native.go", native.replace("return 1", "return 2"), 0),
    ] {
        let changed: Vec<_> = old
            .iter()
            .map(|(p, c)| (*p, if *p == path { content.as_str() } else { *c }))
            .collect();
        let head = f.pin(&repo, "app", case, &changed);
        let mut args = request(&repo, vec![base.clone()], vec![head]);
        let packet = f.run(&args);
        assert_eq!(
            packet["entries"].as_array().unwrap().len(),
            expected,
            "{path}: {packet}"
        );
        assert_eq!(packet["coverage"]["complete"], true, "{path}: {packet}");
        if path == "code_test.go" {
            args["profile"]["tests"] = json!(true);
            assert_eq!(f.run(&args)["entries"].as_array().unwrap().len(), 2);
        }
        if path == "tag.go" {
            args["profile"]["tags"] = json!(["special"]);
            assert_eq!(f.run(&args)["entries"].as_array().unwrap().len(), 2);
        }
    }
}

#[test]
fn embedded_and_module_changes_keep_explicit_conservative_scope() {
    let f = Fixture::new();
    let repo = f.repo("embed-scope");
    let code = "package tax\nimport _ \"embed\"\n//go:embed README.md\nvar Contents string\nfunc F() int { return 1 }\n";
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[("go.mod", MOD), ("code.go", code), ("README.md", "one")],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("go.mod", MOD), ("code.go", code), ("README.md", "two")],
    );
    let packet = f.run(&request(&repo, vec![base.clone()], vec![head]));
    assert_eq!(packet["impact"], "incomplete");
    assert!(
        packet["file_changes"]
            .to_string()
            .contains("embedded_input")
    );
    assert!(packet["entries"].to_string().contains("conservative_scope"));
    let head = f.pin(
        &repo,
        "app",
        'c',
        &[
            ("go.mod", "module example/tax\ngo 1.24\n"),
            ("code.go", code),
            ("README.md", "one"),
        ],
    );
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert_eq!(packet["impact"], "incomplete");
    assert!(packet["entries"].to_string().contains("conservative_scope"));
}

#[test]
fn hunk_packing_keeps_current_head_and_delta_matches_canonical_selection() {
    let f = Fixture::new();
    let repo = f.repo("packing");
    let code = format!(
        "package tax\n{}",
        (0..20)
            .map(|i| format!(
                "func F{i}() int {{ return {i} }}\n{}",
                "// unchanged intervening explanation\n".repeat(20)
            ))
            .collect::<String>()
    );
    let changed = code
        .replace("return 0", "return 100")
        .replace("return 19", "return 119");
    let a = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("code.go", &code),
            ("code_test.go", "package tax\nfunc TestHelper() {}\n"),
        ],
    );
    let b = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("code.go", &changed),
            (
                "code_test.go",
                "package tax\nfunc TestHelper() {println(1)}\n",
            ),
        ],
    );
    let mut args = request(&repo, vec![a], vec![b]);
    args["max_tokens"] = json!(6000);
    let first = f.run(&args);
    assert_eq!(first["coverage"]["current_head"]["usable"], true, "{first}");
    assert!(has(&first, "head", ".F0") && has(&first, "base", ".F0"));
    assert!(first["diffs"].as_array().unwrap().len() >= 2, "{first}");
    assert!(first["diffs"].to_string().len() < code.len());
    args["previous_packet_id"] = first["packet_id"].clone();
    let delta = f.run(&args);
    assert_eq!(delta["context_order"], first["context_order"]);
    assert!(delta["entries"].as_array().unwrap().is_empty());
    assert!(delta["diffs"].as_array().unwrap().is_empty());
    assert!(text::count(&delta.to_string()) <= 6000);
}

#[test]
fn identical_tree_refreshes_binding_without_retransmitting_opted_in_text() {
    let f = Fixture::new();
    let repo = f.repo("body-reuse");
    let old = "package tax\nfunc F() int { return 1 }\n";
    let new = "package tax\nfunc F() int { return 2 }\n";
    let a = f.pin(&repo, "app", 'a', &[("go.mod", MOD), ("code.go", old)]);
    let b = f.pin(&repo, "app", 'b', &[("go.mod", MOD), ("code.go", new)]);
    let mut args = request(&repo, vec![a], vec![b.clone()]);
    args["reuse_text"] = json!(true);
    let first = f.run(&args);
    let c = f.pin(&repo, "app", 'c', &[("go.mod", MOD), ("code.go", new)]);
    assert_eq!(c["snapshot"], b["snapshot"]);
    args["head"] = json!([c]);
    args["previous_packet_id"] = first["packet_id"].clone();
    let delta = f.run(&args);
    let updated = delta["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head")
        .unwrap();
    let before = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == updated["id"])
        .unwrap();
    assert!(updated.get("text").is_none());
    assert_eq!(
        updated["text_sha256"],
        text::sha256(s(before, "text").as_bytes())
    );
    assert_ne!(before["package_binding"], updated["package_binding"]);
    assert_ne!(before["revision"], updated["revision"]);
}

#[test]
fn compiler_cache_reuses_exact_inputs_and_forgets_removed_snapshots() {
    let f = Fixture::new();
    let repo = f.repo("facts-cache");
    let a = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("code.go", "package tax\nfunc F() int { return 1 }\n"),
        ],
    );
    let b = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("code.go", "package tax\nfunc F() int { return 2 }\n"),
        ],
    );
    let mut args = request(&repo, vec![a.clone()], vec![b.clone()]);
    let cold = f.run(&args);
    assert_eq!(cold["analysis_cache"]["base_hit"], false);
    assert_eq!(cold["analysis_cache"]["head_hit"], false);
    let warm = f.run(&args);
    assert_eq!(warm["analysis_cache"]["base_hit"], true);
    assert_eq!(warm["analysis_cache"]["head_hit"], true);
    assert_eq!(cold["packet_id"], warm["packet_id"]);
    args["profile"]["tests"] = json!(true);
    let profile = f.run(&args);
    assert_eq!(profile["analysis_cache"]["base_hit"], false);
    f.store
        .transaction(|db| {
            db.execute(
                "DELETE FROM repo_snapshots WHERE repository_id=? AND sha256=?",
                rusqlite::params![repo, s(&a, "snapshot")],
            )?;
            let left: usize = db.query_row(
                "SELECT count(*) FROM repo_review_analysis_inputs WHERE snapshot=?",
                [s(&a, "snapshot")],
                |r| r.get(0),
            )?;
            assert_eq!(left, 0);
            let caches: usize = db.query_row(
                "SELECT count(*) FROM repo_review_analysis WHERE repository_id=?",
                [&repo],
                |r| r.get(0),
            )?;
            assert_eq!(caches, 2);
            Ok(())
        })
        .unwrap();
    let missing = f.run(&args);
    assert_eq!(missing["impact"], "incomplete");
    assert_eq!(missing["analysis_cache"]["base_hit"], false);
    assert!(review::run(&f.store, &f.workspace, "revoked", &args).is_err());
}

#[test]
fn cached_type_failures_stay_incomplete_and_source_repairs_invalidate_them() {
    let f = Fixture::new();
    let repo = f.repo("qualified-facts");
    let a = f.pin(&repo,"app",'a', &[("go.mod",MOD),("code.go","package tax\nfunc F() int { return omitted() }\n")]);
    let b = f.pin(&repo,"app",'b', &[("go.mod",MOD),("code.go","package tax\nfunc F() int { return omitted()+1 }\n")]);
    let args = request(&repo,vec![a.clone()],vec![b.clone()]);
    let cold = f.run(&args);
    let warm = f.run(&args);
    assert_eq!(warm["analysis_cache"]["base_hit"],true);
    assert_eq!(warm["analysis_cache"]["head_hit"],true);
    assert_eq!(cold["packet_id"],warm["packet_id"]);
    assert_eq!(warm["impact"],"incomplete");
    assert_eq!(warm["coverage"]["complete"],false);
    assert!(warm["coverage"]["issues"].to_string().contains("incomplete_package"));
    let fixed = f.pin(&repo,"app",'c', &[("go.mod",MOD),("code.go","package tax\nfunc F() int { return omitted()+1 }\n"),("restored.go","package tax\nfunc omitted() int { return 3 }\n")]);
    let repaired = f.run(&request(&repo,vec![a],vec![fixed]));
    assert_eq!(repaired["analysis_cache"]["base_hit"],true);
    assert_eq!(repaired["analysis_cache"]["head_hit"],false);
    let malformed = f.pin(&repo,"app",'d', &[("go.mod",MOD),("code.go","package tax\nfunc broken(\n")]);
    let bad_args = request(&repo,vec![b],vec![malformed]);
    f.run(&bad_args);
    let again = f.run(&bad_args);
    assert_eq!(again["analysis_cache"]["head_hit"],false);
    assert_eq!(again["coverage"]["complete"],false);
}

#[test]
fn review_reuse_requires_unchanged_file_and_transitive_package_inputs() {
    let f = Fixture::new();
    let repo = f.repo("review-reuse");
    let root =
        "package tax\nimport \"example/tax/dep\"\nfunc Caller() int { return dep.Value() }\n";
    let dep = "package dep\nfunc Value() int { return 1 }\n";
    let separate = "package separate\nfunc Independent() int { return 1 }\n";
    let dep2 = dep.replace("return 1", "return 2");
    let dep3 = dep.replace("return 1", "return 3");
    let independent2 = separate.replace("return 1", "return 2");
    let a = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("root.go", root),
            ("dep/dep.go", dep),
            ("separate/independent.go", separate),
        ],
    );
    let b = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("root.go", root),
            ("dep/dep.go", &dep2),
            ("separate/independent.go", &independent2),
        ],
    );
    let c = f.pin(
        &repo,
        "app",
        'c',
        &[
            ("go.mod", MOD),
            ("root.go", root),
            ("dep/dep.go", &dep3),
            ("separate/independent.go", &independent2),
        ],
    );
    let mut args = request(&repo, vec![a], vec![b]);
    let first = f.run(&args);
    assert_eq!(first["coverage"]["complete"], true, "{first}");
    let head: Vec<_> = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .collect();
    let independent = head
        .iter()
        .find(|e| s(e, "name").ends_with("Independent"))
        .unwrap()["id"]
        .clone();
    args["previous_packet_id"] = first["packet_id"].clone();
    args["head"] = json!([c]);
    args["reviewed_entries"] = json!(head.iter().map(|e| e["id"].clone()).collect::<Vec<_>>());
    let second = f.run(&args);
    assert_eq!(second["review_reuse"]["eligible"], json!([independent]));
    assert_eq!(second["analysis_cache"]["base_hit"], true);
    assert_eq!(second["analysis_cache"]["head_hit"], false);
    args["business_context"] = json!({});
    assert!(
        f.run(&args)["review_reuse"]["eligible"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    args["base"][0]["commit"] = json!("d".repeat(40));
    assert_eq!(
        review::run(&f.store, &f.workspace, &f.token, &args)
            .unwrap_err()
            .status,
        axum::http::StatusCode::CONFLICT
    );
}

#[test]
fn business_context_adds_historical_constraints_and_revision_safe_deltas() {
    let f = Fixture::new();
    let repo = f.repo("permissions");
    let before = "package tax\nfunc Check(tenant, owner string) bool {\n return tenant == owner\n}\nfunc Authorize(tenant, owner string) bool { return Check(tenant, owner) }\nfunc Author() string { return \"unrelated\" }\n";
    let after = before.replace("tenant == owner", "tenant != owner");
    let checkout = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout.path())
            .args([
                "-c",
                "user.name=Business Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    std::fs::write(checkout.path().join("go.mod"), MOD).unwrap();
    std::fs::write(
        checkout.path().join("access.go"),
        before.replace("tenant == owner", "true"),
    )
    .unwrap();
    git(&["add", "go.mod", "access.go"]);
    git(&["commit", "-qm", "Introduce access flow"]);
    std::fs::write(checkout.path().join("access.go"), before).unwrap();
    git(&[
        "commit",
        "-qam",
        "Fix tenant isolation: reject cross-account access",
    ]);
    let base_commit = git(&["rev-parse", "HEAD"]);
    let base = f.pin_commit(
        &repo,
        "app",
        &base_commit,
        &[("go.mod", MOD), ("access.go", before)],
    );
    std::fs::write(checkout.path().join("access.go"), &after).unwrap();
    git(&["commit", "-qam", "Adjust comparison"]);
    let head = f.pin_commit(
        &repo,
        "app",
        &git(&["rev-parse", "HEAD"]),
        &[("go.mod", MOD), ("access.go", &after)],
    );
    let mut args = request(&repo, vec![base.clone()], vec![head.clone()]);
    args["max_tokens"] = json!(8000);
    let baseline = f.run(&args);
    // The new edit overwrites the original guard. Base-side history is essential.
    let collected = graphicle_runtime::business_context::collect(
        checkout.path(),
        serde_json::from_value(base.clone()).unwrap(),
        &["access.go".into()],
    )
    .unwrap();
    let history = business_history(
        &head,
        "access.go",
        &after,
        json!([
            {"commit":"d".repeat(40),"line_start":5,"line_end":5,"subject":"Revert payment experiment"},
            {"commit":"e".repeat(40),"line_start":6,"line_end":6,"subject":"Fix duplicate charges in unrelated example"}
        ]),
    );
    args["business_context"] = json!({"history":[collected,history]});
    let enhanced = f.run(&args);
    let find = |packet: &Value, suffix: &str| {
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                e["side"] == if suffix == ".Check" { "base" } else { "head" }
                    && s(e, "name").ends_with(suffix)
            })
            .unwrap()
            .clone()
    };
    let check = find(&enhanced, ".Check");
    let caller = find(&enhanced, ".Authorize");
    assert_eq!(
        check["business_context"]["historical_assertions"][0]["subject"],
        "Fix tenant isolation: reject cross-account access"
    );
    assert_eq!(
        check["business_context"]["historical_assertions"][0]["validity"],
        "unverified"
    );
    assert_eq!(
        check["business_context"]["candidates"][0]["domain"],
        "authorization"
    );
    assert_eq!(
        check["business_context"]["candidates"][0]["basis"],
        "historical_subject"
    );
    assert_eq!(
        caller["business_context"]["historical_assertions"][0]["kind"],
        "revert_candidate"
    );
    assert!(
        !caller["business_context"]["candidates"]
            .to_string()
            .contains("payments")
    );
    assert!(caller["reasons"].to_string().contains("potential_consumer"));
    assert!(caller["reasons"].to_string().contains("go/types"));
    assert!(!has(&enhanced, "head", ".Author"));
    for packet in [&baseline, &enhanced] {
        assert!(has(packet, "head", ".Check") && has(packet, "head", ".Authorize"));
        assert_eq!(packet["coverage"]["complete"], true, "{packet}");
        assert!(text::count(&packet.to_string()) <= 8000);
    }
    assert!(find(&baseline, ".Check").get("business_context").is_none());
    let metrics = json!({"fixture":"tenant-isolation-guard-change","budget":8000,"baseline_tokens":baseline["tokens"],"enhanced_tokens":enhanced["tokens"],"required_code_recall_baseline":1.0,"required_code_recall_enhanced":1.0,"historical_constraint_recall_baseline":0.0,"historical_constraint_recall_enhanced":1.0,"unsupported_positive_assertions":0,"model_calls":0,"scope":"deterministic evidence retrieval, not model review quality"});
    println!("business_context_fixture_metrics={metrics}");
    if let Some(path) = std::env::var_os("GRAPHICLE_BUSINESS_EVAL_DIR") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&path).unwrap();
        for (name, value) in [
            ("baseline.json", &baseline),
            ("enhanced.json", &enhanced),
            ("summary.json", &metrics),
        ] {
            std::fs::write(path.join(name), serde_json::to_vec_pretty(value).unwrap()).unwrap();
        }
    }
    let mut tight_args = args.clone();
    tight_args["max_tokens"] = json!(3500);
    let tight = f.run(&tight_args);
    assert!(text::count(&tight.to_string()) <= 3500);
    assert!(tight["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
    assert_eq!(tight["impact"], "incomplete");

    args["previous_packet_id"] = enhanced["packet_id"].clone();
    let repeated = f.run(&args);
    assert!(repeated["entries"].as_array().unwrap().is_empty());
    assert!(
        repeated["unchanged"]
            .as_array()
            .unwrap()
            .contains(&check["id"])
    );
    let next_source = after.replace("tenant != owner", "tenant != owner && owner != \"\"");
    let next_pin = f.pin(
        &repo,
        "app",
        'c',
        &[("go.mod", MOD), ("access.go", &next_source)],
    );
    let mut pushed = args.clone();
    pushed["head"] = json!([next_pin]);
    assert!(
        review::run(&f.store, &f.workspace, &f.token, &pushed).is_err(),
        "Old head history must not survive a new pin"
    );
    pushed["business_context"]["history"][1]["pin"] = pushed["head"][0].clone();
    pushed["business_context"]["history"][1]["files"][0]["file_sha256"] =
        json!(text::sha256(next_source.as_bytes()));
    let next_packet = f.run(&pushed);
    assert!(
        next_packet["unchanged"]
            .as_array()
            .unwrap()
            .contains(&check["id"])
    );
    assert_eq!(find(&next_packet, ".Authorize")["update"], "updated");
    args["business_context"]["history"][0]["files"][0]["attributions"][1]["subject"] =
        json!("Fix permission isolation: revised historical assertion");
    let updated = f.run(&args);
    let new_check = find(&updated, ".Check");
    assert_eq!(new_check["update"], "updated");
    assert_ne!(new_check["revision"], check["revision"]);
    args["previous_packet_id"] = updated["packet_id"].clone();
    args["business_context"] = json!({});
    let removed = f.run(&args);
    let no_history = find(&removed, ".Check");
    assert_eq!(no_history["update"], "updated");
    assert!(
        no_history["business_context"]["historical_assertions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    args["previous_packet_id"] = removed["packet_id"].clone();
    args.as_object_mut().unwrap().remove("business_context");
    let disabled = f.run(&args);
    assert!(find(&disabled, ".Check").get("business_context").is_none());
}

#[test]
fn business_history_refuses_stale_foreign_and_unavailable_evidence() {
    let f = Fixture::new();
    let repo = f.repo("history");
    let source = "package tax\nfunc Check() bool { return true }\n";
    let pin = f.pin(&repo, "app", 'a', &[("go.mod", MOD), ("access.go", source)]);
    let history = business_history(
        &pin,
        "access.go",
        source,
        json!([{ "commit":"c".repeat(40),"line_start":2,"line_end":2,"subject":"Fix permission check" }]),
    );
    let mut args = request(&repo, vec![pin.clone()], vec![pin.clone()]);
    for mutation in ["hash", "range", "commit", "duplicate", "missing", "policy"] {
        let mut invalid = history.clone();
        match mutation {
            "hash" => invalid["files"][0]["file_sha256"] = json!("0".repeat(64)),
            "range" => invalid["files"][0]["attributions"][0]["line_end"] = json!(3),
            "commit" => invalid["pin"]["commit"] = json!("d".repeat(40)),
            "policy" => invalid["pin"]["policy_sha256"] = json!("d".repeat(64)),
            "duplicate" => {
                let range = invalid["files"][0]["attributions"][0].clone();
                invalid["files"][0]["attributions"]
                    .as_array_mut()
                    .unwrap()
                    .push(range);
            }
            _ => invalid["files"][0]["path"] = json!("missing.go"),
        }
        args["business_context"] = json!({"history":[invalid]});
        assert!(
            review::run(&f.store, &f.workspace, &f.token, &args).is_err(),
            "{mutation}"
        );
    }
    let other = f.store.create_workspace("unavailable-history").unwrap();
    let hidden = f
        .store
        .register(
            s(&other, "id"),
            s(&other, "capability"),
            &json!({"project_key":"hidden","name":"hidden"}),
        )
        .unwrap();
    let mut foreign = history;
    foreign["pin"]["repository_id"] = hidden["id"].clone();
    foreign["pin"]["directory"] = json!("hidden");
    args["base"]
        .as_array_mut()
        .unwrap()
        .push(foreign["pin"].clone());
    args["head"]
        .as_array_mut()
        .unwrap()
        .push(foreign["pin"].clone());
    args["business_context"] = json!({"history":[foreign]});
    let error = review::run(&f.store, &f.workspace, &f.token, &args).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert!(!error.message.contains("permission check"));
}

#[test]
fn pinned_chain_delta_and_identical_tree_commit_binding() {
    let f = Fixture::new();
    let tax = f.repo("tax");
    let pricing = f.repo("pricing");
    let checkout = f.repo("checkout");
    let tax_a = f.pin(
        &tax,
        "tax",
        'a',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int { return 1 }\n"),
        ],
    );
    let pmod = "module example/pricing\ngo 1.23\nrequire example/tax v0.0.0\nreplace example/tax => ../tax\n";
    let pc =
        "package pricing\nimport \"example/tax\"\nfunc Quote() int { return tax.Calculate() }\n";
    let p = f.pin(
        &pricing,
        "pricing",
        'a',
        &[("go.mod", pmod), ("pricing.go", pc)],
    );
    let cmod = "module example/checkout\ngo 1.23\nrequire (\nexample/pricing v0.0.0\nexample/tax v0.0.0\n)\nreplace example/pricing => ../pricing\nreplace example/tax => ../tax\n";
    let c=f.pin(&checkout,"checkout",'a',&[("go.mod",cmod),("checkout.go","package checkout\nimport \"example/pricing\"\nfunc Submit() int { return pricing.Quote() }\n")]);
    let tax_b = f.pin(
        &tax,
        "tax",
        'b',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int { return 2 }\n"),
        ],
    );
    let mut args = request(
        &checkout,
        vec![c.clone(), p.clone(), tax_a],
        vec![c.clone(), p.clone(), tax_b.clone()],
    );
    let first = f.run(&args);
    assert_eq!(first["coverage"]["complete"], true, "{first}");
    for name in ["Calculate", "Quote", "Submit"] {
        assert!(has(&first, "head", name), "{name}: {first}");
    }
    let names: std::collections::BTreeMap<_, _> = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .map(|e| (s(e, "symbol_id"), s(e, "name")))
        .collect();
    let mut edges = std::collections::BTreeSet::new();
    for e in first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
    {
        for reason in e["reasons"].as_array().unwrap() {
            for step in reason["steps"].as_array().into_iter().flatten() {
                edges.insert((names[s(step, "caller")], names[s(step, "callee")]));
            }
        }
    }
    let expected = std::collections::BTreeSet::from([
        ("example/checkout.Submit", "example/pricing.Quote"),
        ("example/pricing.Quote", "example/tax.Calculate"),
    ]);
    assert_eq!(edges, expected);
    let required = ["Calculate", "Quote", "Submit"];
    let context_hits = required
        .iter()
        .filter(|name| has(&first, "head", name))
        .count();
    println!(
        "review_fixture_metrics={}",
        json!({"call_edge_precision":edges.intersection(&expected).count() as f64/edges.len() as f64,"call_edge_recall":edges.intersection(&expected).count() as f64/expected.len() as f64,"affected_symbol_recall":context_hits as f64/required.len() as f64,"required_context_recall":context_hits as f64/required.len() as f64,"budget":16000,"tokens":first["tokens"]})
    );
    let submit = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && s(e, "name").ends_with("Submit"))
        .unwrap();
    assert!(
        submit["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"].as_array().unwrap().len() == 3)
    );
    args["previous_packet_id"] = first["packet_id"].clone();
    let repeated = f.run(&args);
    assert!(repeated["entries"].as_array().unwrap().is_empty());
    assert!(repeated["diffs"].as_array().unwrap().is_empty());
    assert!(!repeated["unchanged"].as_array().unwrap().is_empty());
    // Same source tree at a new commit retains a separate valid binding.
    let same = f.pin(
        &tax,
        "tax",
        'c',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int { return 2 }\n"),
        ],
    );
    let unchanged = f.run(&request(&tax, vec![tax_b], vec![same]));
    assert_eq!(unchanged["impact"], "no_changes_in_scope", "{unchanged}");
    let tax_d = f.pin(
        &tax,
        "tax",
        'd',
        &[
            ("go.mod", MOD),
            (
                "moved.go",
                "package tax\nfunc Calculate() int { return 3 }\n",
            ),
        ],
    );
    args["head"] = json!([c, p, tax_d]);
    let delta = f.run(&args);
    assert!(
        !delta["invalidated"].as_array().unwrap().is_empty(),
        "{delta}"
    );
    assert!(has(&delta, "head", "Calculate"));
    let stale_ids: std::collections::BTreeSet<_> = delta["invalidated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| s(x, "id"))
        .collect();
    assert!(
        first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["side"] == "head" && s(e, "name").ends_with("Calculate"))
            .all(|e| stale_ids.contains(s(e, "id")))
    );
    let mut full_args = args.clone();
    full_args
        .as_object_mut()
        .unwrap()
        .remove("previous_packet_id");
    let full = f.run(&full_args);
    let mut state: std::collections::BTreeMap<String, String> = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(first["diffs"].as_array().unwrap())
        .map(|e| (s(e, "id").into(), s(e, "revision").into()))
        .collect();
    for e in delta["invalidated"].as_array().unwrap() {
        state.remove(s(e, "id"));
    }
    for e in delta["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(delta["diffs"].as_array().unwrap())
    {
        state.insert(s(e, "id").into(), s(e, "revision").into());
    }
    let rebuilt: std::collections::BTreeMap<String, String> = full["entries"]
        .as_array()
        .unwrap()
        .iter()
        .chain(full["diffs"].as_array().unwrap())
        .map(|e| (s(e, "id").into(), s(e, "revision").into()))
        .collect();
    assert_eq!(
        state, rebuilt,
        "delta application must equal a fresh packet"
    );
    args.as_object_mut().unwrap().remove("previous_packet_id");
    args["max_depth"] = json!(0);
    let shallow = f.run(&args);
    assert_eq!(shallow["coverage"]["complete"], false);
    assert!(
        shallow["coverage"]["issues"]
            .to_string()
            .contains("depth_limit")
    );
}

#[test]
fn deletion_nesting_move_and_proven_closure_dispatch_are_explicit() {
    let f = Fixture::new();
    let repo = f.repo("tax");
    let before = "package tax\nfunc Calculate() int { return 1 }\nfunc Removed() int { return Calculate() }\nfunc Outer() int { f := func() int { return Calculate() }; return f() + 10 }\n";
    let after = "package tax\nfunc Calculate() int { return 2 }\nfunc Outer() int { f := func() int { return Calculate() }; return f() + 20 }\n";
    let a = f.pin(&repo, "tax", 'a', &[("go.mod", MOD), ("tax.go", before)]);
    let b = f.pin(&repo, "tax", 'b', &[("go.mod", MOD), ("tax.go", after)]);
    let report = f.run(&request(&repo, vec![a], vec![b.clone()]));
    assert!(has(&report, "base", "Removed"));
    assert!(!has(&report, "head", "Removed"));
    assert_eq!(report["coverage"]["complete"], true);
    assert!(
        !report["coverage"]["issues"]
            .to_string()
            .contains("unresolved_dynamic_target")
    );
    let outer = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && s(e, "name").ends_with("Outer"))
        .unwrap();
    assert!(s(outer, "text").ends_with("+ 20 }"));
    let closure = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && e["kind"] == "closure")
        .unwrap();
    assert!(!s(closure, "parent").is_empty());
    assert!(!s(closure, "text").contains("+ 20"));
    let c = f.pin(
        &repo,
        "tax",
        'c',
        &[
            ("go.mod", MOD),
            (
                "package/moved.go",
                "package tax\nfunc Calculate() int { return 2 }\n",
            ),
        ],
    );
    let moved = f.run(&request(&repo, vec![b], vec![c]));
    assert!(
        moved["changes"].to_string().contains("moved_or_renamed"),
        "{moved}"
    );
}

#[test]
fn unavailable_snapshots_and_bad_attestations_never_claim_no_impact() {
    let f = Fixture::new();
    let repo = f.repo("tax");
    let a = f.pin(
        &repo,
        "tax",
        'a',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int { return 1 }\n"),
        ],
    );
    let mut b = a.clone();
    b["commit"] = json!("b".repeat(40));
    let args = request(&repo, vec![a], vec![b]);
    let report = f.run(&args);
    assert_eq!(report["impact"], "incomplete");
    assert!(
        report["coverage"]["issues"]
            .to_string()
            .contains("snapshot_binding_mismatch")
    );
    assert_eq!(
        review::run(&f.store, &f.workspace, "invalid", &args)
            .unwrap_err()
            .status,
        axum::http::StatusCode::FORBIDDEN
    );
    let mut args = args;
    args["previous_packet_id"] = json!("0".repeat(64));
    assert!(review::run(&f.store, &f.workspace, &f.token, &args).is_err());
}

#[test]
fn empty_build_selections_and_missing_dependencies_produce_qualified_packets() {
    let f = Fixture::new();
    let repo = f.repo("empty-selections");
    let files = [
        ("go.mod", "module example/selection\ngo 1.23\n"),
        (
            "main.go",
            "package selection\nfunc Value() int { return 1 }\n",
        ),
        ("checks/only_test.go", "package checks\nfunc Helper() {}\n"),
        (
            "tagged/only.go",
            "//go:build excluded\n\npackage tagged\nfunc Hidden() {}\n",
        ),
        (
            "platform/only_windows.go",
            "package platform\nfunc Windows() {}\n",
        ),
        ("arch/only_arm64.go", "package arch\nfunc Arm() {}\n"),
        (
            "native/only.go",
            "package native\nimport \"C\"\nfunc Native() {}\n",
        ),
    ];
    let base = f.pin(&repo, "app", 'a', &files);
    let mut changed = files;
    changed[1].1 = "package selection\nfunc Value() int { return 2 }\n";
    let head = f.pin(&repo, "app", 'b', &changed);
    for tests in [false, true] {
        let mut args = request(&repo, vec![base.clone()], vec![head.clone()]);
        args["profile"]["tests"] = json!(tests);
        let first = f.run(&args);
        assert!(has(&first, "head", ".Value"), "{first}");
        args["previous_packet_id"] = first["packet_id"].clone();
        let delta = f.run(&args);
        assert_eq!(delta["impact"], first["impact"]);
        assert!(!delta["unchanged"].as_array().unwrap().is_empty());
    }
    let mut missing = changed.to_vec();
    missing.push((
        "bad/call.go",
        "package bad\nimport \"example.test/missing\"\nfunc Call() { missing.Run() }\n",
    ));
    let unavailable = f.pin(&repo, "app", 'c', &missing);
    let report = f.run(&request(&repo, vec![base], vec![unavailable]));
    assert_eq!(report["impact"], "incomplete");
    assert_eq!(report["coverage"]["complete"], false);
    assert!(
        report["coverage"]["issues"]
            .to_string()
            .contains("incomplete_discovery")
    );
}

#[test]
fn budget_and_inaccessible_repository_omissions_are_visible() {
    let f = Fixture::new();
    let repo = f.repo("tax");
    let content = format!(
        "package tax\nfunc Calculate() int {{\n{}return 1\n}}\n",
        "// evidence needed to understand a deliberately large function\n".repeat(2500)
    );
    let changed = content.replace("return 1", "return 2");
    let a = f.pin(&repo, "tax", 'a', &[("go.mod", MOD), ("tax.go", &content)]);
    let b = f.pin(&repo, "tax", 'b', &[("go.mod", MOD), ("tax.go", &changed)]);
    let other = f.store.create_workspace("inaccessible").unwrap();
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
    args["max_tokens"] = json!(4000);
    let report = f.run(&args);
    assert_eq!(report["coverage"]["complete"], false);
    assert_eq!(report["impact"], "incomplete");
    assert!(
        report["coverage"]["budget_omitted"].as_u64().unwrap() > 0,
        "{report}"
    );
    assert!(
        report["coverage"]["issues"]
            .to_string()
            .contains("unavailable_snapshot")
    );
    assert!(text::count(&report.to_string()) <= 4000);
    args["previous_packet_id"] = report["packet_id"].clone();
    let error = review::run(&f.store, &f.workspace, &f.token, &args).unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(
        error.message,
        "Previous packet references an unavailable repository; request a full packet"
    );
    // Dropping the receipt recovers without exposing the inaccessible pin.
    args.as_object_mut().unwrap().remove("previous_packet_id");
    let full = f.run(&args);
    assert_eq!(full["impact"], "incomplete");
    assert!(
        full["coverage"]["issues"]
            .to_string()
            .contains("unavailable_snapshot")
    );
}

#[test]
fn review_api_and_mcp_preserve_project_confinement() {
    use graphicle_runtime::{api, model};
    let f = Fixture::new();
    let repo = f.repo("tax");
    let pin = f.pin(
        &repo,
        "tax",
        'a',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int {return 1}\n"),
        ],
    );
    let mut args = request(&repo, vec![pin.clone()], vec![pin.clone()]);
    args["business_context"] = json!({});
    args["compact_provenance"] = json!(true);
    let app = api::App::new(f.store.clone(), model::Config::default(), vec![]).unwrap();
    let capabilities = api::command(&app, &f.workspace, &f.token, "build.info", &json!({})).unwrap();
    assert_eq!(capabilities["review_compact_provenance"], "graphicle.service-provenance.v1");
    let packet = api::command(&app, &f.workspace, &f.token, "graph.impact", &args).unwrap();
    assert_eq!(packet["impact"], "no_changes_in_scope");
    let mut mcp_args = args;
    mcp_args.as_object_mut().unwrap().remove("repository_id");
    let rpc = |args: Value| {
        api::mcp(&app,&f.workspace,&f.token,&repo,&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"review_changes","arguments":args}})).unwrap()
    };
    let result = rpc(mcp_args.clone());
    assert_ne!(result["result"]["isError"], true, "{result}");
    assert_eq!(
        result["result"]["structuredContent"]["impact"], "no_changes_in_scope",
        "{result}"
    );
    let mut foreign = pin;
    foreign["repository_id"] = json!(f.repo("foreign"));
    foreign["directory"] = json!("foreign");
    mcp_args["head"].as_array_mut().unwrap().push(foreign);
    let refused = rpc(mcp_args);
    assert!(
        refused.to_string().contains("cross-repository review"),
        "{refused}"
    );
}

#[test]
fn changed_package_constant_links_its_unchanged_consumer() {
    let f = Fixture::new();
    let tax = f.repo("tax");
    let consumer = f.repo("consumer");
    let a = f.pin(
        &tax,
        "tax",
        'a',
        &[("go.mod", MOD), ("rate.go", "package tax\nconst Rate=1\n")],
    );
    let b = f.pin(
        &tax,
        "tax",
        'b',
        &[("go.mod", MOD), ("rate.go", "package tax\nconst Rate=2\n")],
    );
    let c=f.pin(&consumer,"consumer",'a',&[("go.mod","module example/consumer\ngo 1.23\nrequire example/tax v0.0.0\nreplace example/tax => ../tax\n"),("use.go","package consumer\nimport \"example/tax\"\nfunc Use() int {return tax.Rate}\n")]);
    let report = f.run(&request(&consumer, vec![c.clone(), a], vec![c, b]));
    assert!(has(&report, "head", "Use"), "{report}");
    assert_eq!(report["impact"], "potential_impact");
    assert_eq!(report["coverage"]["complete"], true, "{report}");
    assert!(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"].as_str().unwrap().ends_with("Use")
                && e["reasons"].to_string().contains("uses_value")),
        "{report}"
    );
}

#[tokio::test]
async fn core_http_routes_coverage_bindings_and_second_push_are_revision_safe() {
    use graphicle_runtime::{api, model};
    let f = Fixture::new();
    let repo = f.repo("tax");
    let a = f.pin(
        &repo,
        "tax",
        'a',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int {return 1}\n"),
        ],
    );
    let b = f.pin(
        &repo,
        "tax",
        'b',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int {return 2}\n"),
        ],
    );
    let args = request(&repo, vec![a.clone()], vec![b.clone()]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = api::App::new(
        f.store.clone(),
        model::Config::default(),
        vec![address.to_string()],
    )
    .unwrap();
    let server =
        tokio::spawn(async move { axum::serve(listener, api::router(app)).await.unwrap() });
    let client = reqwest::Client::new();
    let url = format!(
        "http://{address}/v1/workspaces/{}/repos/{repo}",
        f.workspace
    );
    let post = |path: &str, body: Value, token: &str| {
        client
            .post(format!("{url}/{path}"))
            .bearer_auth(token)
            .json(&body)
            .send()
    };
    let mut upload = json!({"repository_id":repo,"format":"graphicle.coverage.v1","commit":b["commit"],"snapshot":b["snapshot"],"policy_sha256":b["policy_sha256"],"profile":args["profile"],"environment":"fixture-ci","ranges":[{"path":"tax.go","line_start":2,"line_end":2,"test_id":"TestCalculate"}]});
    let first = client
        .post(format!(
            "http://{address}/api/workspaces/{}/repositories/{repo}/impact",
            f.workspace
        ))
        .bearer_auth(&f.token)
        .json(&args)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first: Value = first.json().await.unwrap();
    let stored = post("coverage", upload.clone(), &f.token).await.unwrap();
    assert_eq!(stored.status(), 200);
    let stored: Value = stored.json().await.unwrap();
    assert_eq!(stored["replayed"], false);
    let replay: Value = post("coverage", upload.clone(), &f.token)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replay["id"], stored["id"]);
    assert_eq!(replay["replayed"], true);
    let mut delta_args = args.clone();
    delta_args["previous_packet_id"] = first["packet_id"].clone();
    let response = post("impact/report", delta_args, &f.token).await.unwrap();
    assert_eq!(response.status(), 200);
    let enriched: Value = response.json().await.unwrap();
    let observed = enriched["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head")
        .unwrap();
    assert_eq!(observed["update"], "updated");
    assert_eq!(
        observed["execution_observations"][0]["ranges"][0]["test_id"],
        "TestCalculate"
    );
    assert_eq!(enriched["execution_coverage"]["matching_runs"], 1);
    assert_eq!(
        enriched["execution_coverage"]["complete_test_inventory"],
        false
    );
    // Build-profile mismatches do not attach observations from another build.
    let mut tagged = args.clone();
    tagged["profile"]["tags"] = json!(["pilot"]);
    assert_eq!(f.run(&tagged)["execution_coverage"]["matching_runs"], 0);
    // Same tree at a new commit still requires its own execution observation.
    let c = f.pin(
        &repo,
        "tax",
        'c',
        &[
            ("go.mod", MOD),
            ("tax.go", "package tax\nfunc Calculate() int {return 2}\n"),
        ],
    );
    let mut next = request(&repo, vec![a], vec![c]);
    next["previous_packet_id"] = enriched["packet_id"].clone();
    let next = f.run(&next);
    assert_eq!(next["execution_coverage"]["matching_runs"], 0);
    let entry = next["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head")
        .unwrap();
    assert_eq!(entry["update"], "updated");
    assert!(entry.get("execution_observations").is_none());
    // Bad source ranges, unknown bindings and unauthorized uploads are refused.
    upload["ranges"][0]["line_end"] = json!(99);
    assert_eq!(
        post("coverage", upload.clone(), &f.token)
            .await
            .unwrap()
            .status(),
        400
    );
    upload["ranges"][0]["line_end"] = json!(2);
    upload["commit"] = json!("f".repeat(40));
    assert_eq!(
        post("coverage", upload.clone(), &f.token)
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(
        post("coverage", upload.clone(), "invalid")
            .await
            .unwrap()
            .status(),
        403
    );
    let reader = f
        .store
        .grant(
            &f.workspace,
            &f.token,
            &json!({"label":"reader","role":"reader"}),
        )
        .unwrap();
    assert_eq!(
        post("coverage", upload, s(&reader, "capability"))
            .await
            .unwrap()
            .status(),
        403
    );
    server.abort();
}

#[test]
fn changed_function_selects_internal_and_external_unchanged_tests() {
    let f = Fixture::new();
    let repo = f.repo("tax");
    let files = |body| {
        vec![
            ("go.mod", MOD),
            ("tax.go", body),
            (
                "tax_test.go",
                "package tax\nfunc InternalTestHelper() int {return Calculate()}\n",
            ),
            (
                "external_test.go",
                "package tax_test\nimport t \"example/tax\"\nfunc ExternalTestHelper() int {return t.Calculate()}\n",
            ),
        ]
    };
    let a = f.pin(
        &repo,
        "tax",
        'a',
        &files("package tax\nfunc Calculate() int {return 1}\n"),
    );
    let b = f.pin(
        &repo,
        "tax",
        'b',
        &files("package tax\nfunc Calculate() int {return 2}\n"),
    );
    let mut args = request(&repo, vec![a], vec![b]);
    args["profile"]["tests"] = json!(true);
    args["max_tokens"] = json!(32000);
    let report = f.run(&args);
    for name in ["InternalTestHelper", "ExternalTestHelper"] {
        let entry = report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head" && s(e, "name").ends_with(name))
            .unwrap_or_else(|| panic!("missing {name}: {report}"));
        assert_eq!(entry["test"], true);
        assert!(
            entry["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == "potential_consumer")
        );
    }
    assert_eq!(report["coverage"]["complete"], true, "{report}");
}

#[test]
fn identical_source_from_a_different_repository_changes_the_dependency_binding() {
    let f = Fixture::new();
    let anchor = f.repo("consumer");
    let original = f.repo("tax-original");
    let replacement = f.repo("tax-replacement");
    let files = [
        ("go.mod", MOD),
        ("tax.go", "package tax\nfunc Calculate() int {return 1}\n"),
    ];
    let a = f.pin(&original, "tax", 'a', &files);
    let b = f.pin(&replacement, "tax", 'a', &files);
    let consumer = f.pin(&anchor, "consumer", 'a', &[("go.mod", "module example/consumer\ngo 1.23\nrequire example/tax v0.0.0\nreplace example/tax => ../tax\n"), ("main.go", "package consumer\nimport \"example/tax\"\nfunc Use() int {return tax.Calculate()}\n")]);
    let report = f.run(&request(
        &anchor,
        vec![consumer.clone(), a],
        vec![consumer, b],
    ));
    assert_eq!(report["dependency_binding_changed"], true, "{report}");
    assert_eq!(report["impact"], "potential_impact");
    for (side, repo) in [("base", original), ("head", replacement)] {
        assert!(
            report["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == side && e["repository_id"] == repo)
        );
    }
}

#[test]
fn binary_embeds_are_exact_inputs_and_never_text_excerpts() {
    let f = Fixture::new();
    let repo = f.repo("binary-embed");
    let code = b"package tax\nimport _ \"embed\"\n//go:embed defaults.binpb\nvar data []byte\nfunc F() int { return 1 }\n";
    let bytes = [0, 0xff, 13, 10, 42];
    let before = f.pin_bytes(
        &repo,
        "app",
        &"a".repeat(40),
        &[
            ("go.mod", MOD.as_bytes()),
            ("credentials/code.go", code),
            ("credentials/defaults.binpb", &bytes),
        ],
    );
    let repeated = f.pin_bytes(
        &repo,
        "app",
        &"b".repeat(40),
        &[
            ("go.mod", MOD.as_bytes()),
            ("credentials/code.go", code),
            ("credentials/defaults.binpb", &bytes),
        ],
    );
    assert_eq!(before["snapshot"], repeated["snapshot"]);
    f.store
        .transaction(|db| {
            let stored: Vec<u8> = db.query_row(
                "SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?",
                rusqlite::params![repo, text::sha256(&bytes)],
                |r| r.get(0),
            )?;
            assert_eq!(stored, bytes);
            let project = serde_json::json!({"id":repo});
            let docs = graphicle_runtime::memory::repository_documents(db, &project, true)?;
            assert!(
                docs.iter()
                    .all(|d| d["path"] != "credentials/defaults.binpb")
            );
            Ok(())
        })
        .unwrap();
    let unchanged = f.run(&request(&repo, vec![before.clone()], vec![repeated]));
    assert!(unchanged["coverage"]["complete"] == true, "{unchanged}");
    let changed = f.pin_bytes(
        &repo,
        "app",
        &"c".repeat(40),
        &[
            ("go.mod", MOD.as_bytes()),
            ("credentials/code.go", code),
            ("credentials/defaults.binpb", &[0, 0xfe, 13, 10, 42]),
        ],
    );
    let result = f.run(&request(&repo, vec![before], vec![changed]));
    assert_eq!(result["impact"], "incomplete");
    assert!(
        result["file_changes"]["examples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["binary"] == true && e["classification"] == "embedded_input")
    );
    assert!(
        result["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["file"] != "credentials/defaults.binpb")
    );
    assert!(result["diffs"].as_array().unwrap().is_empty());
}

#[test]
fn prepared_dependencies_are_bound_to_their_native_build_profile() {
    let f = Fixture::new();
    let repo = f.repo("prepared-profile");
    let pin = f.pin(
        &repo,
        "tax",
        'a',
        &[("go.mod", MOD), ("code.go", "package tax\nfunc F() {}\n")],
    );
    f.store.transaction(|db| {
        db.execute("UPDATE repo_snapshots SET metadata=json_set(metadata,'$.dependency_preparation',json(?)) WHERE repository_id=? AND sha256=?", rusqlite::params![json!({"profile":{"goos":"linux","goarch":"amd64","tests":false}}).to_string(), repo, s(&pin,"snapshot")])?;
        Ok(())
    }).unwrap();
    let mut args = request(&repo, vec![pin.clone()], vec![pin]);
    let matching = f.run(&args);
    assert_eq!(matching["coverage"]["complete"], true);
    args["profile"]["tests"] = json!(true);
    let other = f.run(&args);
    assert_eq!(other["coverage"]["complete"], false);
    assert!(
        other["coverage"]
            .to_string()
            .contains("dependency_preparation_profile_mismatch")
    );
    assert_eq!(other["analysis_cache"]["base_hit"], false);
}

#[test]
fn package_unknowns_and_import_changes_do_not_seed_unrelated_packages() {
    let f = Fixture::new();
    let repo = f.repo("package-scope");
    let before = [
        ("go.mod", MOD),
        (
            "dep/code.go",
            "package dep\nconst Limit = 1\nfunc F() int { return Limit }\n",
        ),
        (
            "code.go",
            "package tax\nimport \"example/tax/dep\"\nfunc Use() int { return dep.F() }\nfunc init() {}\nfunc init() {}\n",
        ),
        (
            "unused/code.go",
            "package unused\nfunc Unrelated() int { return 9 }\n",
        ),
    ];
    let a = f.pin(&repo, "tax", 'a', &before);
    let mut after = before;
    after[1].1 = "package dep\nconst Limit = 2\nfunc F() int { return Limit }\n";
    let b = f.pin(&repo, "tax", 'b', &after);
    let packet = f.run(&request(&repo, vec![a.clone()], vec![b.clone()]));
    assert!(has(&packet, "head", "Use"));
    assert!(!has(&packet, "head", "Unrelated"));
    assert!(
        !packet["coverage"]["issues"]
            .to_string()
            .contains("duplicate_compiler_declaration")
    );
    assert_eq!(packet["changes"]["count"], 1);
    after[2].1 = "package tax\nfunc Use() int { return 3 }\nfunc init() {}\nfunc init() {}\n";
    let c = f.pin(&repo, "tax", 'c', &after);
    let changed = f.run(&request(&repo, vec![b], vec![c]));
    assert_eq!(changed["dependency_binding_changed"], true);
    assert!(!has(&changed, "head", "Unrelated"));
}

#[test]
fn initializer_effects_reach_consumers_even_when_only_the_callee_changes() {
    let f = Fixture::new();
    let repo = f.repo("initializers");
    let source = "package tax\nvar _ = Changed()\nvar callback = func() int { return Changed() }\nfunc Changed() int { return 1 }\nfunc Unchanged() int { return 7 }\n";
    let a = f.pin(&repo, "tax", 'a', &[("go.mod", MOD), ("code.go", source)]);
    let next = source.replace("return 1", "return 2");
    let b = f.pin(&repo, "tax", 'b', &[("go.mod", MOD), ("code.go", &next)]);
    let packet = f.run(&request(&repo, vec![a], vec![b]));
    assert!(has(&packet, "head", "Unchanged"));
    assert!(
        packet["coverage"]["issues"]
            .to_string()
            .contains("package_initialization_scope")
    );
    assert!(
        packet["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "example/tax.callback"
                && e["reasons"].to_string().contains("references_callable"))
    );
}

#[test]
fn reuse_is_scoped_and_omissions_never_preserve_a_proof() {
    let f = Fixture::new();
    let repo = f.repo("scoped-reuse");
    let leaf = "package leaf\nfunc Value() int { return 1 }\n";
    let leaf2 = leaf.replace("return 1", "return 2");
    let noisy = "package noisy\nfunc Dynamic(f func()) { f() }\n";
    let huge = format!(
        "package huge\nfunc Large() int {{\n{}return 1\n}}\n",
        "// lots of context here\n".repeat(4000)
    );
    let huge2 = huge.replace("return 1", "return 2");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[
            ("go.mod", MOD),
            ("leaf/value.go", leaf),
            ("noisy/call.go", noisy),
            ("huge/code.go", &huge),
        ],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("leaf/value.go", &leaf2),
            ("noisy/call.go", noisy),
            ("huge/code.go", &huge2),
        ],
    );
    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(6000);
    let first = f.run(&args);
    assert_eq!(first["coverage"]["complete"], false);
    assert!(first["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
    let entry = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && e["file"] == "leaf/value.go")
        .unwrap();
    assert_eq!(entry["review_basis"]["complete"], true, "{first}");
    args["previous_packet_id"] = first["packet_id"].clone();
    args["reviewed_entries"] = json!([entry["id"]]);
    let second = f.run(&args);
    assert_eq!(second["review_reuse"]["eligible"], args["reviewed_entries"]);
    // A new incoming consumer joins the component and invalidates the proof.
    let caller =
        "package caller\nimport \"example/tax/leaf\"\nfunc Run() int { return leaf.Value() }\n";
    let next = f.pin(
        &repo,
        "app",
        'c',
        &[
            ("go.mod", MOD),
            ("leaf/value.go", &leaf2),
            ("noisy/call.go", noisy),
            ("huge/code.go", &huge2),
            ("caller/run.go", caller),
        ],
    );
    args["head"] = json!([next]);
    assert_eq!(f.run(&args)["review_reuse"]["eligible"], json!([]));
}

#[test]
fn unresolved_calls_and_missing_file_declarations_force_full_review() {
    let f = Fixture::new();
    let repo = f.repo("closed-reuse");
    let base_code = "package tax\nfunc Value() int { return 1 }\nfunc Unchanged() {}\n";
    let next_code = base_code.replace("return 1", "return 2");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[("go.mod", MOD), ("main.go", base_code)],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[("go.mod", MOD), ("main.go", &next_code)],
    );
    let mut args = request(&repo, vec![base], vec![head]);
    let first = f.run(&args);
    let entry = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head")
        .unwrap();
    // The unchanged second declaration was not supplied. File reuse must fail
    // even though static impact coverage is otherwise complete.
    assert_eq!(first["coverage"]["complete"], true);
    assert_eq!(entry["review_basis"]["complete"], false);
    args["previous_packet_id"] = first["packet_id"].clone();
    args["reviewed_entries"] = json!([entry["id"]]);
    assert_eq!(f.run(&args)["review_reuse"]["eligible"], json!([]));
    let dynamic = "package tax\nfunc Run(f func()) { f() }\n";
    let dynamic2 = dynamic.replace("f() }", "f(); f() }");
    let a = f.pin(&repo, "app", 'c', &[("go.mod", MOD), ("main.go", dynamic)]);
    let b = f.pin(
        &repo,
        "app",
        'd',
        &[("go.mod", MOD), ("main.go", &dynamic2)],
    );
    let mut args = request(&repo, vec![a], vec![b]);
    let first = f.run(&args);
    let entry = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head")
        .unwrap();
    assert_eq!(entry["review_basis"]["complete"], false);
    args["previous_packet_id"] = first["packet_id"].clone();
    args["reviewed_entries"] = json!([entry["id"]]);
    assert_eq!(f.run(&args)["review_reuse"]["eligible"], json!([]));
}

#[test]
fn smaller_budget_revokes_file_proof_and_updates_delta_metadata() {
    let f = Fixture::new();
    let repo = f.repo("reuse-budget");
    let small = "package tax\nfunc Value() int { return 1 }\n";
    let changed = small.replace("return 1", "return 2");
    let large = format!(
        "package tax\nfunc Large() int {{\n{}return 1\n}}\n",
        "// required evidence\n".repeat(1600)
    );
    let large2 = large.replace("return 1", "return 2");
    let base = f.pin(
        &repo,
        "app",
        'a',
        &[("go.mod", MOD), ("small.go", small), ("large.go", &large)],
    );
    let head = f.pin(
        &repo,
        "app",
        'b',
        &[
            ("go.mod", MOD),
            ("small.go", &changed),
            ("large.go", &large2),
        ],
    );
    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(64000);
    let first = f.run(&args);
    let entry = first["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && e["file"] == "small.go")
        .unwrap();
    assert_eq!(entry["review_basis"]["complete"], true, "{first}");
    args["previous_packet_id"] = first["packet_id"].clone();
    args["reviewed_entries"] = json!([entry["id"]]);
    args["max_tokens"] = json!(4000);
    let delta = f.run(&args);
    assert_eq!(delta["review_reuse"]["eligible"], json!([]));
    let next = delta["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == entry["id"])
        .unwrap();
    assert_eq!(next["review_basis"]["complete"], false);
    assert_ne!(next["revision"], entry["revision"]);
    assert_eq!(next["update"], "updated");
}

#[test]
fn historical_coupling_adds_exact_context_and_explains_review_priority() {
    let f = Fixture::new();
    let repo = f.repo("history-risk");
    let checkout = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(checkout.path())
            .args([
                "-c",
                "user.name=History Fixture",
                "-c",
                "user.email=history@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "--initial-branch=history-fixture"]);
    let check = |v| {
        format!(
            "package tax\nfunc Check(x int) bool {{\n if x < 0 {{ return false }}\n if x == 0 {{ return false }}\n if x > 100 {{ return false }}\n if x == 99 {{ return false }}\n return x >= {v}\n}}\n"
        )
    };
    let verify = |v| format!("package tax\nfunc Verify(x int) bool {{ return x >= {v} }}\n");
    let other = |v| format!("package tax\nfunc Other() int {{ return {v} }}\n");
    let caller = "package tax\nfunc Caller(x int) bool { return Check(x) }\n";
    for (path, source) in [
        ("go.mod", MOD.to_owned()),
        ("access.go", check(0)),
        ("audit.go", verify(0)),
        ("other.go", other(0)),
        ("caller.go", caller.into()),
    ] {
        std::fs::write(checkout.path().join(path), source).unwrap();
    }
    git(&["add", "."]);
    git(&["commit", "-qm", "Introduce flow"]);
    for i in 1..=7 {
        std::fs::write(checkout.path().join("access.go"), check(i)).unwrap();
        std::fs::write(checkout.path().join("audit.go"), verify(i)).unwrap();
        git(&["commit", "-qam", "Fix tenant isolation and companion check"]);
    }
    for i in 1..=4 {
        std::fs::write(checkout.path().join("other.go"), other(i)).unwrap();
        git(&["commit", "-qam", "Update unrelated behavior"]);
    }
    let before = check(7);
    let after = check(8);
    let audit = verify(7);
    let other = other(4);
    let base = f.pin_commit(
        &repo,
        "app",
        &git(&["rev-parse", "HEAD"]),
        &[
            ("go.mod", MOD),
            ("access.go", &before),
            ("audit.go", &audit),
            ("other.go", &other),
            ("caller.go", caller),
        ],
    );
    std::fs::write(checkout.path().join("access.go"), &after).unwrap();
    git(&["commit", "-qam", "Adjust threshold"]);
    let head = f.pin_commit(
        &repo,
        "app",
        &git(&["rev-parse", "HEAD"]),
        &[
            ("go.mod", MOD),
            ("access.go", &after),
            ("audit.go", &audit),
            ("other.go", &other),
            ("caller.go", caller),
        ],
    );
    let base_history = graphicle_runtime::business_context::collect_analysis(
        checkout.path(),
        serde_json::from_value(base.clone()).unwrap(),
        &["access.go".into()],
    )
    .unwrap();
    let head_history = graphicle_runtime::business_context::collect_analysis(
        checkout.path(),
        serde_json::from_value(head.clone()).unwrap(),
        &["access.go".into()],
    )
    .unwrap();
    let mut args = request(&repo, vec![base], vec![head]);
    args["max_tokens"] = json!(32000);
    let baseline = f.run(&args);
    assert!(!has(&baseline, "head", "Verify"));
    args["business_context"] = json!({"history":[base_history,head_history]});
    let history_input = args["business_context"].clone();
    let enhanced = f.run(&args);
    assert!(has(&enhanced, "head", "Check"));
    assert!(has(&enhanced, "head", "Caller"));
    assert!(has(&enhanced, "head", "Verify"));
    let check = enhanced["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && s(e, "name").ends_with(".Check"))
        .unwrap();
    let evidence = &check["business_context"]["evolution"];
    assert!(evidence.is_object(), "{enhanced}");
    assert!(
        evidence["cochange"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["file"] == "audit.go" && c["changed_in_review"] == false)
    );
    assert_eq!(
        check["business_context"]["review_priority"]["level"],
        "elevated"
    );
    assert_eq!(check["business_context"]["criticality"], "unknown");
    assert!(
        evidence["function_history"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| !h["cause_candidates"].as_array().unwrap().is_empty())
    );
    assert_eq!(
        evidence["ownership"]["contributors"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let peer = enhanced["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["side"] == "head" && s(e, "name").ends_with(".Verify"))
        .unwrap();
    assert_eq!(peer["text"], "func Verify(x int) bool { return x >= 7 }");
    assert_eq!(peer["reasons"][0]["reason"], "historical_cochange");
    assert_eq!(peer["reasons"][0]["compiler_dependency"], false);
    assert!(text::count(&enhanced.to_string()) <= 32000);
    let mut bounded_args = args.clone();
    bounded_args["max_tokens"] = json!(8000);
    let bounded = f.run(&bounded_args);
    assert!(text::count(&bounded.to_string()) <= 8000);
    for name in ["Check", "Caller", "Verify"] {
        assert!(has(&bounded, "head", name), "{name}: {bounded}");
    }
    assert!(
        bounded["entries"].as_array().unwrap().iter().any(|e| {
            e["side"] == "head"
                && s(e, "name").ends_with(".Check")
                && e["business_context"]["evolution"].is_object()
        }),
        "{bounded}"
    );
    let mut unsafe_history = args.clone();
    unsafe_history["business_context"]["history"][0]["evolution"]["units"][0]["subject"] =
        json!(format!("Fix using ghp_{}", "q".repeat(36)));
    assert!(review::run(&f.store, &f.workspace, &f.token, &unsafe_history).is_err());
    let mut partial = args.clone();
    partial["previous_packet_id"] = enhanced["packet_id"].clone();
    partial["business_context"]["history"][1]["evolution"]["units"][0]["omitted_changes"] =
        json!(1);
    let changed_membership = f.run(&partial);
    assert!(
        changed_membership["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["side"] == "head"
                    && s(entry, "name").ends_with(".Check")
                    && entry["update"] == "updated"
                    && entry["business_context"]["evolution"]["sample"]["coupling_available"]
                        == false
            }),
        "{changed_membership}"
    );
    let mut stale = args.clone();
    stale["business_context"]["history"][0]["evolution"]["functions"][0]["file_sha256"] =
        json!("0".repeat(64));
    assert!(review::run(&f.store, &f.workspace, &f.token, &stale).is_err());
    args["previous_packet_id"] = enhanced["packet_id"].clone();
    args["business_context"] = json!({});
    let removed = f.run(&args);
    assert!(
        removed["invalidated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == peer["id"])
    );
    if let Some(directory) = std::env::var_os("GRAPHICLE_HISTORY_RISK_EVAL_DIR") {
        let dir = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, value) in [
            ("baseline", baseline.clone()),
            ("enhanced", enhanced.clone()),
            ("bounded", bounded.clone()),
            ("history", history_input),
            (
                "summary",
                json!({"model_calls":0,"baseline_tokens":baseline["tokens"],"enhanced_tokens":enhanced["tokens"],"budget":32000,"bounded_tokens":bounded["tokens"],"bounded_budget":8000,"changed_helper_retained":true,"static_caller_retained":true,"historical_peer_added":true,"historical_peer_is_not_call_edge":true,"cause_links_are_candidates":true,"removal_invalidates_peer":true,"scope":"deterministic context and risk-evidence acceptance, not reviewer quality"}),
            ),
        ] {
            std::fs::write(
                dir.join(format!("{name}.json")),
                serde_json::to_vec_pretty(&value).unwrap(),
            )
            .unwrap();
        }
    }
}

#[test]
fn callback_candidates_expand_impact_but_never_claim_complete_dispatch() {
    let f=Fixture::new();let repo=f.repo("callback-impact");
    let code="package tax\nfunc A() int {return 1}\nfunc invoke(cb func() int) int {return cb()}\nfunc Handle() int {return invoke(A)}\n";
    let base=f.pin(&repo,"app",'a',&[("go.mod",MOD),("main.go",code)]);
    let changed=code.replace("return 1","return 2");let head=f.pin(&repo,"app",'b',&[("go.mod",MOD),("main.go",&changed)]);
    let mut args=request(&repo,vec![base],vec![head]);
    let full=f.run(&args);assert!(has(&full,"head",".invoke"));assert!(has(&full,"head",".Handle"));assert_eq!(full["coverage"]["complete"],false);
    assert!(full["coverage"]["issues"].to_string().contains("unresolved_dynamic_target"));
    let entry=full["entries"].as_array().unwrap().iter().find(|e|e["side"]=="head" && s(e,"name").ends_with(".invoke")).unwrap();
    let reason=entry["reasons"].as_array().unwrap().iter().find(|r|r.get("execution_flow").is_some()).unwrap();
    assert_eq!(reason["steps"][0]["relation"],"may_call");assert_eq!(reason["steps"][0]["citation"]["commit"],"b".repeat(40));assert_eq!(reason["execution_flow"]["observed"],false);
    args["previous_packet_id"]=full["packet_id"].clone();let delta=f.run(&args);
    assert_eq!(full["execution_flows"],delta["execution_flows"]);assert_eq!(full["context_order"],delta["context_order"]);
}
