use super::*;

fn fixture(specs: &[(&str, Value)]) -> (Input, Analysis) {
    let mut input = Input {
        pins: vec![],
        sources: BTreeMap::new(),
        issues: vec![],
        bindings: vec![],
    };
    let mut a = Analysis {
        diagnostics:vec![],
        diagnostic_count:0,
        symbols: BTreeMap::new(),
        calls: vec![],
        packages: vec![],
        issues: vec![],
        analyzer: "test".into(),
        toolchain: "test".into(),
        inputs: vec![],
        raw: None,
        package_cache: crate::review_cache::PackageResult::default(),
        services: Links::default(),
    };
    for (dir, bindings) in specs {
        let code = "package main\nfunc Handle() { send() }\n";
        let config = "deployment: production\n";
        let contract = "contract version one\n";
        let manifest =
            json!({"format":FORMAT,"inventory_complete":true,"bindings":bindings}).to_string();
        let pin = Pin {
            repository_id: dir.to_string(),
            commit: "a".repeat(40),
            snapshot: text::sha256(manifest.as_bytes()),
            policy_sha256: "b".repeat(64),
            directory: dir.to_string(),
            modules: vec![".".into()],
            excluded_modules: vec![],
        };
        input.pins.push(pin);
        for (file, code) in [
            ("main.go", code),
            ("deployment.txt", config),
            ("contract.txt", contract),
            (MANIFEST, &manifest),
        ] {
            input.sources.insert(
                format!("{dir}/{file}"),
                Source {
                    repository: dir.to_string(),
                    path: file.into(),
                    sha: text::sha256(code.as_bytes()),
                    content: crate::source_object::Content::Text(code.into()),
                },
            );
        }
        let start = code.find("func").unwrap();
        let end = code.trim_end().len();
        let symbol = Symbol {
            key: "Handle".into(),
            file: format!("{dir}/main.go"),
            package: dir.to_string(),
            name: "Handle".into(),
            kind: "function".into(),
            parent: String::new(),
            start,
            end,
            body_start: start,
            line: 2,
            end_line: 2,
            revision: text::sha256(&code.as_bytes()[start..end]),
            signature: "sig".into(),
            implementation: "impl".into(),
            correspondence: "corr".into(),
        };
        a.symbols
            .insert(key(dir, "Handle"), (dir.to_string(), symbol));
    }
    (input, a)
}

fn citation(file: &str, code: &str, start: usize, end: usize) -> Value {
    json!({"file":file,"sha256":text::sha256(code.as_bytes()),"byte_start":start,"byte_end":end})
}
fn binding(id: &str, service: &str, inbound: bool, transport: Value) -> Value {
    let code = "package main\nfunc Handle() { send() }\n";
    let config = "deployment: production\n";
    let contract = "contract version one\n";
    json!({"id":id,"service":{"name":service,"namespace":"test","environment":"prod","configuration":"v1"},
        "direction":if inbound{"inbound"}else{"outbound"},"transport":transport,
        "owner":citation("main.go",code,code.find("func").unwrap(),code.trim_end().len()),
        "site":citation("main.go",code,code.find("send()").unwrap(),code.find("send()").unwrap()+6),
        "configuration":[citation("deployment.txt",config,0,config.len())],
        "contract":citation("contract.txt",contract,0,contract.len())})
}
fn http(path: &str) -> Value {
    json!({"protocol":"http","authority":"https://orders.example","method":"GET","path":path})
}
fn grpc(service: &str) -> Value {
    json!({"protocol":"grpc","authority":"grpcs://pricing.example","service":service,"method":"Quote"})
}
fn event(group: Option<&str>) -> Value {
    json!({"protocol":"event","broker":"project-a/pubsub","topic":"orders","schema":"Order/v1","group":group})
}
fn linked(a: &Analysis, relation: &str) -> usize {
    a.services
        .edges
        .iter()
        .filter(|e| e.relation == relation)
        .count()
}
fn reason(a: &Analysis, value: &str) -> bool {
    a.issues.iter().any(|i| i["reason"] == value)
}

#[test]
fn http_identity_ambiguity_and_dynamic_config_never_choose_a_first_match() {
    let caller = binding("get", "checkout", false, http("/orders/123"));
    let target = binding("handle", "orders", true, http("/orders/{id}"));
    let mut other = target.clone();
    other["id"] = json!("other");
    for (case, mut candidate, expected) in [
        ("other_environment", other.clone(), 1),
        ("other_host", other.clone(), 1),
        ("duplicate", other, 0),
    ] {
        if case == "other_environment" {
            candidate["service"]["environment"] = json!("staging");
        }
        if case == "other_host" {
            candidate["transport"]["authority"] = json!("https://unrelated.example");
        }
        let (input, mut a) = fixture(&[
            ("checkout", json!([caller])),
            ("orders", json!([target, candidate])),
        ]);
        attach(&input, &mut a).unwrap();
        assert_eq!(linked(&a, "http_calls"), expected, "{case}");
        if expected == 0 {
            assert!(reason(&a, "service_target_ambiguous"));
        }
    }
    for dynamic in [true, false] {
        let mut c = caller.clone();
        if dynamic {
            c["dynamic"] = json!(true);
        } else {
            c["transport"]["authority"] = Value::Null;
        }
        let (input, mut a) = fixture(&[("checkout", json!([c])), ("orders", json!([target]))]);
        attach(&input, &mut a).unwrap();
        assert_eq!(linked(&a, "http_calls"), 0);
        assert!(reason(&a, "service_dynamic_configuration"));
    }
}

// Adapted from CodeGraphContext's MIT-licensed per-row target regression:
// tests/unit/tools/test_query_correctness_batch.py::
// test_find_callers_keeps_per_row_target_when_definitions_differ.
// Copyright (c) 2025-2026. See licenses/CodeGraphContext-MIT.txt.
#[test]
fn http_common_operation_keeps_each_callers_own_service_target() {
    let mut a = binding("save", "a", false, http("/save"));
    let mut b = binding("save", "b", false, http("/save"));
    let mut user = binding("save", "users", true, http("/save"));
    let mut order = binding("save", "orders", true, http("/save"));
    for binding in [&mut a, &mut user] {
        binding["transport"]["authority"] = json!("https://users.example");
    }
    for binding in [&mut b, &mut order] {
        binding["transport"]["authority"] = json!("https://orders.example");
    }
    let (input, mut graph) = fixture(&[
        ("a", json!([a])),
        ("b", json!([b])),
        ("users", json!([user])),
        ("orders", json!([order])),
    ]);
    attach(&input, &mut graph).unwrap();
    let pairs: BTreeSet<_> = graph
        .services
        .edges
        .iter()
        .filter(|e| e.relation == "http_calls")
        .map(|e| {
            (
                s(&e.evidence["source"]["service"], "name"),
                s(&e.evidence["target"]["service"], "name"),
            )
        })
        .collect();
    assert_eq!(pairs, BTreeSet::from([("a", "users"), ("b", "orders")]));
}

#[test]
fn grpc_uses_qualified_contract_version_and_listener_identity() {
    let caller = binding("quote", "checkout", false, grpc("pricing.v1.Price"));
    let target = binding("handle", "pricing", true, grpc("pricing.v1.Price"));
    let (input, mut a) = fixture(&[("checkout", json!([caller])), ("pricing", json!([target]))]);
    attach(&input, &mut a).unwrap();
    assert_eq!(linked(&a, "grpc_calls"), 1);
    for field in ["service", "authority", "contract"] {
        let mut target = target.clone();
        if field == "service" {
            target["transport"][field] = json!("other.v1.Price");
        } else if field == "authority" {
            target["transport"][field] = json!("grpcs://other.example");
        } else {
            target["contract"]["sha256"] = json!("0".repeat(64));
        }
        let (input, mut a) =
            fixture(&[("checkout", json!([caller])), ("pricing", json!([target]))]);
        attach(&input, &mut a).unwrap();
        assert_eq!(linked(&a, "grpc_calls"), 0, "{field}");
        assert!(reason(&a, "service_target_unavailable"));
    }
}

#[test]
fn events_fan_out_only_across_distinct_groups_and_exact_broker_topic_schema() {
    let producer = binding("publish", "orders", false, event(None));
    let a = binding("consume", "billing", true, event(Some("billing")));
    let b = binding("consume", "email", true, event(Some("email")));
    let (input, mut graph) = fixture(&[
        ("orders", json!([producer])),
        ("billing", json!([a])),
        ("email", json!([b])),
    ]);
    attach(&input, &mut graph).unwrap();
    assert_eq!(linked(&graph, "event_delivers"), 2);
    let mut b = b;
    b["transport"]["group"] = json!("billing");
    let (input, mut graph) = fixture(&[
        ("orders", json!([producer])),
        ("billing", json!([a])),
        ("email", json!([b])),
    ]);
    attach(&input, &mut graph).unwrap();
    assert_eq!(linked(&graph, "event_delivers"), 0);
    assert!(reason(&graph, "service_target_ambiguous"));
    for field in ["broker", "topic", "schema"] {
        let mut a = a.clone();
        a["transport"][field] = json!("different");
        let (input, mut graph) = fixture(&[("orders", json!([producer])), ("billing", json!([a]))]);
        attach(&input, &mut graph).unwrap();
        assert_eq!(linked(&graph, "event_delivers"), 0, "{field}");
    }
}

#[test]
fn missing_source_stale_hash_ambiguous_owner_and_incomplete_inventory_are_visible() {
    for case in ["missing", "stale", "owner", "inventory", "traversal"] {
        let caller = binding("request", "checkout", false, http("/orders"));
        let mut target = binding("handle", "orders", true, http("/orders"));
        if case == "stale" {
            target["owner"]["sha256"] = json!("0".repeat(64));
        }
        if case == "traversal" {
            target["owner"]["file"] = json!("../checkout/main.go");
        }
        let (mut input, mut a) =
            fixture(&[("checkout", json!([caller])), ("orders", json!([target]))]);
        if case == "missing" {
            input.sources.retain(|p, _| !p.starts_with("orders/"));
            a.symbols.retain(|_, (w, _)| w != "orders");
        }
        if case == "owner" {
            a.symbols.retain(|_, (w, _)| w != "orders");
        }
        if case == "inventory" {
            let source = input
                .sources
                .get_mut(&format!("orders/{MANIFEST}"))
                .unwrap();
            let mut manifest: Value = serde_json::from_slice(source.content.bytes()).unwrap();
            manifest["inventory_complete"] = json!(false);
            source.content = crate::source_object::Content::Text(manifest.to_string());
            source.sha = text::sha256(source.content.bytes());
        }
        attach(&input, &mut a).unwrap();
        assert!(!a.issues.is_empty(), "{case}");
        if case != "inventory" {
            assert_eq!(linked(&a, "http_calls"), 0, "{case}");
        }
    }
}

#[test]
fn mixed_protocol_paths_share_traversal_depth_limits_and_pinned_provenance() {
    let (input, mut a) = fixture(&[
        (
            "checkout",
            json!([binding("http", "checkout", false, http("/orders"))]),
        ),
        (
            "orders",
            json!([
                binding("serve", "orders", true, http("/orders")),
                binding("grpc", "orders", false, grpc("pricing.v1.Price"))
            ]),
        ),
        (
            "pricing",
            json!([
                binding("quote", "pricing", true, grpc("pricing.v1.Price")),
                binding("publish", "pricing", false, event(None))
            ]),
        ),
        (
            "billing",
            json!([binding("consume", "billing", true, event(Some("billing")))]),
        ),
    ]);
    attach(&input, &mut a).unwrap();
    let start = key("checkout", "Handle");
    let end = key("billing", "Handle");
    let selected = select(&a, std::slice::from_ref(&start), 12).unwrap();
    assert!(selected.reasons.contains_key(&end));
    let path = selected.reasons[&end]
        .iter()
        .find(|r| r["reason"] == "dependency")
        .unwrap();
    let steps = path["steps"].as_array().unwrap();
    for relation in ["http_calls", "grpc_calls", "event_delivers"] {
        assert!(steps.iter().any(|s| s["relation"] == relation));
    }
    assert!(steps.iter().all(|s| s["evidence"] == "configured_static"));
    assert!(path.to_string().contains(&input.pins[0].snapshot));
    let limited = select(&a, &[start], 2).unwrap();
    assert!(!limited.reasons.contains_key(&end));
    assert!(limited.issues.iter().any(|i| i["reason"] == "depth_limit"));
    let reverse = select(&a, &[end], 12).unwrap();
    assert!(reverse.reasons.contains_key(&key("checkout", "Handle")));
}

#[test]
fn non_go_owners_reuse_structural_spans_without_claiming_complete_local_calls() {
    for (file, code) in [
        ("handler.js", "function handle() { return 1; }"),
        ("handler.py", "def handle():\n    return 1\n"),
    ] {
        let caller = binding("request", "checkout", false, http("/orders"));
        let mut target = binding("handler", "orders", true, http("/orders"));
        let facts = graphicle_memory::structure::analyze(file, code).unwrap();
        let symbol = &facts["symbols"][0];
        target["owner"] = citation(
            file,
            code,
            symbol["byte_start"].as_u64().unwrap() as usize,
            symbol["byte_end"].as_u64().unwrap() as usize,
        );
        target["site"] = target["owner"].clone();
        let (mut input, mut a) =
            fixture(&[("checkout", json!([caller])), ("orders", json!([target]))]);
        input.sources.insert(
            format!("orders/{file}"),
            Source {
                repository: "orders".into(),
                path: file.into(),
                sha: text::sha256(code.as_bytes()),
                content: crate::source_object::Content::Text(code.into()),
            },
        );
        attach(&input, &mut a).unwrap();
        assert_eq!(linked(&a, "http_calls"), 1, "{file}: {:?}", a.issues);
        assert!(reason(&a, "service_local_analysis_incomplete"));
    }
}

#[test]
fn unresolved_listener_cannot_make_another_listener_look_unique() {
    let caller = binding("request", "checkout", false, http("/orders"));
    let target = binding("serve", "orders", true, http("/orders"));
    let mut unknown = target.clone();
    unknown["id"] = json!("unknown");
    unknown["transport"]["authority"] = Value::Null;
    let (input, mut a) = fixture(&[
        ("checkout", json!([caller])),
        ("orders", json!([target, unknown])),
    ]);
    attach(&input, &mut a).unwrap();
    assert_eq!(linked(&a, "http_calls"), 0);
    assert!(reason(&a, "service_target_ambiguous"));
}

#[test]
fn configuration_in_another_pin_requires_exact_authorized_bytes() {
    let mut caller = binding("request", "checkout", false, http("/orders"));
    caller["configuration"][0]["directory"] = json!("deploy");
    let target = binding("serve", "orders", true, http("/orders"));
    let (mut input, mut a) = fixture(&[
        ("checkout", json!([caller])),
        ("orders", json!([target])),
        ("deploy", json!([])),
    ]);
    attach(&input, &mut a).unwrap();
    assert_eq!(linked(&a, "http_calls"), 1);
    let config_repo = a
        .services
        .edges
        .iter()
        .find(|e| e.relation == "http_calls")
        .unwrap()
        .evidence["source"]["citations"][2]["repository_id"]
        .clone();
    assert_eq!(config_repo, "deploy");
    input.sources.remove("deploy/deployment.txt");
    let (_, mut next) = fixture(&[
        ("checkout", json!([])),
        ("orders", json!([])),
        ("deploy", json!([])),
    ]);
    attach(&input, &mut next).unwrap();
    assert_eq!(linked(&next, "http_calls"), 0);
    assert!(reason(&next, "service_source_unavailable_or_stale"));
}

#[test]
fn event_fanout_is_bounded_and_never_claims_complete_resolution_after_truncation() {
    let senders: Vec<_> = (0..65)
        .map(|i| binding(&format!("send-{i}"), "sender", false, event(None)))
        .collect();
    let receivers: Vec<_> = (0..65)
        .map(|i| {
            binding(
                &format!("receive-{i}"),
                "receiver",
                true,
                event(Some(&format!("group-{i}"))),
            )
        })
        .collect();
    let (input, mut a) = fixture(&[("sender", json!(senders)), ("receiver", json!(receivers))]);
    attach(&input, &mut a).unwrap();
    assert_eq!(a.services.edges.len(), MAX_RELATIONSHIPS);
    assert!(reason(&a, "service_relationship_limit"));
    assert!(
        a.services
            .nodes
            .values()
            .filter(|n| n["direction"] == "outbound")
            .all(|n| n["resolution"]["status"] != "configured_targets")
    );
}
