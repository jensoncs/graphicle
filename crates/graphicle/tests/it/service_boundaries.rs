use super::*;

const CODE: &str = "package service\nfunc Handle() int { return 1 }\n";
const CONFIG: &str = "deployment=prod\n";
const CONTRACT: &str = "operation contract v1\n";

fn cite(file: &str, code: &str, start: usize, end: usize) -> Value {
    json!({"file":file,"sha256":text::sha256(code.as_bytes()),"byte_start":start,"byte_end":end})
}
fn binding(id: &str, service: &str, inbound: bool, transport: Value, code: &str) -> Value {
    let start = code.find("func").unwrap();
    json!({"id":id,"service":{"namespace":"company","name":service,"environment":"prod","configuration":"deployment-v1"},
        "direction":if inbound{"inbound"}else{"outbound"},"transport":transport,
        "owner":cite("service.go",code,start,code.trim_end().len()),
        "site":cite("service.go",code,code.find("return").unwrap(),code.trim_end().len()-2),
        "configuration":[cite("deployment.txt",CONFIG,0,CONFIG.len())],
        "contract":cite("contract.txt",CONTRACT,0,CONTRACT.len())})
}
fn pin(f: &Fixture, repo: &str, name: &str, commit: char, code: &str, bindings: Value) -> Value {
    let manifest=json!({"format":"graphicle.service-boundaries.v1","inventory_complete":true,"bindings":bindings}).to_string();
    let module = format!("module company/{name}\ngo 1.23\n");
    f.pin(
        repo,
        name,
        commit,
        &[
            ("go.mod", &module),
            ("service.go", code),
            ("deployment.txt", CONFIG),
            ("contract.txt", CONTRACT),
            ("graphicle-services.json", &manifest),
        ],
    )
}
fn http() -> Value {
    json!({"protocol":"http","authority":"https://orders.company","method":"POST","path":"/orders"})
}
fn grpc() -> Value {
    json!({"protocol":"grpc","authority":"grpcs://pricing.company","service":"pricing.v1.Price","method":"Quote"})
}
fn event(group: Option<&str>) -> Value {
    json!({"protocol":"event","broker":"company/pubsub","topic":"quoted-orders","schema":"Order/v1","group":group})
}
fn issue(packet: &Value, reason: &str) -> bool {
    packet["coverage"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == reason)
}

#[test]
fn service_boundaries_mixed_protocol_review_and_delta_invalidation() {
    let f = Fixture::new();
    let repos: Vec<_> = ["checkout", "orders", "pricing", "billing"]
        .iter()
        .map(|name| f.repo(name))
        .collect();
    let declarations = |i, code| match i {
        0 => json!([binding("send", "checkout", false, http(), code)]),
        1 => json!([
            binding("serve", "orders", true, http(), code),
            binding("quote", "orders", false, grpc(), code)
        ]),
        2 => json!([
            binding("quote", "pricing", true, grpc(), code),
            binding("publish", "pricing", false, event(None), code)
        ]),
        _ => json!([binding(
            "consume",
            "billing",
            true,
            event(Some("billing")),
            code
        )]),
    };
    let names = ["checkout", "orders", "pricing", "billing"];
    let base: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, name)| pin(&f, &repos[i], name, 'a', CODE, declarations(i, CODE)))
        .collect();
    let mut head = base.clone();
    let changed = CODE.replace("return 1", "return 2");
    head[2] = pin(
        &f,
        &repos[2],
        "pricing",
        'b',
        &changed,
        declarations(2, &changed),
    );
    let mut args = request(&repos[2], base.clone(), head.clone());
    args["max_depth"] = json!(16);
    args["max_tokens"] = json!(64000);
    let first = f.run(&args);
    if let Ok(directory) = std::env::var("GRAPHICLE_SERVICE_EVIDENCE_DIR") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(
            directory.join("fixture-packet.json"),
            serde_json::to_vec_pretty(&first).unwrap(),
        )
        .unwrap();
        let labels = json!({"head":first["head"],"callers":[
            {"repository_id":repos[0],"binding":"send","protocol":"http","targets":[[repos[1],"serve"]]},
            {"repository_id":repos[1],"binding":"quote","protocol":"grpc","targets":[[repos[2],"quote"]]},
            {"repository_id":repos[2],"binding":"publish","protocol":"event","targets":[[repos[3],"consume"]]}]});
        std::fs::write(
            directory.join("fixture-labels.json"),
            serde_json::to_vec_pretty(&labels).unwrap(),
        )
        .unwrap();
    }
    let flows=first["execution_flows"]["paths"].as_array().unwrap();
    assert!(flows.iter().any(|flow| {
        let relations: std::collections::BTreeSet<_> = flow["steps"].as_array().unwrap().iter().map(|step| {
            let entry=first["entries"].as_array().unwrap().iter().find(|e|e["id"]==step["entry_id"]).unwrap();
            entry["reasons"][step["reason_index"].as_u64().unwrap() as usize]["steps"][step["step_index"].as_u64().unwrap() as usize]["relation"].as_str().unwrap()
        }).collect();
        relations.contains("http_calls") && relations.contains("grpc_calls") && relations.contains("event_delivers") && flow["observed"]==false
    }),"expected a composed HTTP → gRPC → event path: {first}");
    assert_eq!(first["service_graph"]["head"]["resolved_relationships"], 3);
    for repo in &repos {
        assert!(
            first["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == "head" && e["repository_id"] == *repo),
            "{first}"
        );
    }
    let entries = first["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .filter(|e| e.get("service_boundary").is_some())
            .all(
                |e| e["service_boundary"]["authority"] == "pinned_configuration_assertion"
                    && e["service_boundary"]["citations"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|c| c["snapshot"].is_string() && c["commit"].is_string())
            )
    );
    assert!(
        entries
            .iter()
            .filter(|e| e["side"] == "head")
            .all(|e| e["review_basis"]["complete"] == false)
    );
    args["max_depth"] = json!(0);
    let limited = f.run(&args);
    assert!(issue(&limited, "depth_limit"));
    args["max_depth"] = json!(16);
    // Removing the subscription must invalidate previously supplied context,
    // even though the handler bytes and compiler facts have not changed.
    head[3] = pin(&f, &repos[3], "billing", 'c', CODE, json!([]));
    args["head"] = json!(head);
    args["previous_packet_id"] = first["packet_id"].clone();
    let delta = f.run(&args);
    assert_eq!(delta["service_graph"]["head"]["resolved_relationships"], 2);
    assert!(issue(&delta, "service_target_unavailable"));
    let old = entries
        .iter()
        .find(|e| e["side"] == "head" && e["name"] == "billing::consume")
        .unwrap();
    assert!(
        delta["invalidated"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == old["id"])
    );
    args.as_object_mut().unwrap().remove("previous_packet_id");
    let fresh = f.run(&args);
    assert_eq!(fresh["service_graph"], delta["service_graph"]);
    assert_eq!(fresh["coverage"], delta["coverage"]);
}

#[test]
fn service_boundaries_never_read_or_link_an_inaccessible_repository() {
    let f = Fixture::new();
    let foreign = Fixture::new();
    let caller = f.repo("caller");
    let private = foreign.repo("private");
    let source = pin(
        &f,
        &caller,
        "caller",
        'a',
        CODE,
        json!([binding("send", "caller", false, http(), CODE)]),
    );
    let secret = pin(
        &foreign,
        &private,
        "private",
        'b',
        CODE,
        json!([binding(
            "PRIVATE_HANDLER",
            "private-service",
            true,
            http(),
            CODE
        )]),
    );
    let args = request(
        &caller,
        vec![source.clone(), secret.clone()],
        vec![source, secret],
    );
    let result = f.run(&args);
    assert_eq!(result["service_graph"]["head"]["resolved_relationships"], 0);
    assert_eq!(result["coverage"]["complete"], false);
    assert!(issue(&result, "unavailable_snapshot"));
    assert!(issue(&result, "service_target_unavailable"));
    assert!(!result.to_string().contains("PRIVATE_HANDLER"));
    assert!(!result.to_string().contains("private-service"));
}

#[test]
fn service_boundaries_reject_stale_evidence_after_a_push() {
    let f = Fixture::new();
    let repo = f.repo("stale");
    let b = binding("send", "caller", false, http(), CODE);
    let base = pin(&f, &repo, "caller", 'a', CODE, json!([b]));
    let head = pin(
        &f,
        &repo,
        "caller",
        'b',
        &CODE.replace("return 1", "return 2"),
        json!([b]),
    );
    let packet = f.run(&request(&repo, vec![base], vec![head]));
    assert!(issue(&packet, "service_source_unavailable_or_stale"));
    assert_eq!(packet["service_graph"]["head"]["boundaries"], 0);
    assert_eq!(packet["coverage"]["complete"], false);
}
