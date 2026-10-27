use graphicle_memory::text;
use graphicle_runtime::{
    review,
    store::{Store, s},
};
use serde_json::{Value, json};

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
        let state = self
            .store
            .project_status(&self.workspace, &self.token, &json!({"repository_id":repo}))
            .unwrap();
        let policy = text::sha256(b"fixture source admission v1");
        let begin=self.store.sync_prepare(&self.workspace,&self.token,&json!({"repository_id":repo,"expected_snapshot":state["repository"]["snapshot"],"metadata":{"commit":commit.to_string().repeat(40),"policy_sha256":policy,"scope_complete":true},"manifest":files.iter().map(|(p,c)|json!({"path":p,"sha256":text::sha256(c.as_bytes()),"bytes":c.len()})).collect::<Vec<_>>()})).unwrap();
        if !begin["sync_id"].is_null() {
            self.store.sync_upload(&self.workspace,&self.token,&json!({"repository_id":repo,"sync_id":begin["sync_id"],"files":files.iter().map(|(p,c)|json!({"path":p,"content":c})).collect::<Vec<_>>()})).unwrap();
            self.store
                .sync_commit(
                    &self.workspace,
                    &self.token,
                    &json!({"repository_id":repo,"sync_id":begin["sync_id"]}),
                )
                .unwrap();
        }
        json!({"repository_id":repo,"directory":dir,"snapshot":begin["snapshot"],"commit":commit.to_string().repeat(40),"policy_sha256":policy})
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
fn deletion_nesting_move_and_unresolved_dispatch_are_explicit() {
    let f = Fixture::new();
    let repo = f.repo("tax");
    let before = "package tax\nfunc Calculate() int { return 1 }\nfunc Removed() int { return Calculate() }\nfunc Outer() int { f := func() int { return Calculate() }; return f() + 10 }\n";
    let after = "package tax\nfunc Calculate() int { return 2 }\nfunc Outer() int { f := func() int { return Calculate() }; return f() + 20 }\n";
    let a = f.pin(&repo, "tax", 'a', &[("go.mod", MOD), ("tax.go", before)]);
    let b = f.pin(&repo, "tax", 'b', &[("go.mod", MOD), ("tax.go", after)]);
    let report = f.run(&request(&repo, vec![a], vec![b.clone()]));
    assert!(has(&report, "base", "Removed"));
    assert!(!has(&report, "head", "Removed"));
    assert_eq!(report["coverage"]["complete"], false);
    assert!(
        report["coverage"]["issues"]
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
    let args = request(&repo, vec![pin.clone()], vec![pin.clone()]);
    let app = api::App::new(f.store.clone(), model::Config::default(), vec![]).unwrap();
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
fn changed_package_constant_seeds_consumer_and_reports_unmodeled_data_flow() {
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
    assert_eq!(report["impact"], "incomplete");
    assert!(
        report["coverage"]["issues"]
            .to_string()
            .contains("unsupported_package_value_dependency"),
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
