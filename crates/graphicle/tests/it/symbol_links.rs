//! Reviewed memory read from the code symbols it is linked to, and code
//! read from the memory.
//!
//! A code window selected for a question lifts the reviewed memory linked to
//! a symbol inside it, and a selected memory lifts its symbols' current
//! windows, only with `symbol_links: true`; left out, the links are counted;
//! `false` reads none. The memory's text shares no word with the question,
//! so only the link can bring it. A stale memory is never lifted or listed.
//! `get_node` lists a symbol's linked memories and documents. Answers take
//! these switches, and `notes_first`, as recall does.
use axum::Json;
use graphicle_runtime::{
    graph_tools,
    jobs::{self, Worker},
    model,
    store::{Store, s},
};
use serde_json::{Value, json};
use tempfile::TempDir;

const A: &str = "import os\n\n\ndef alpha():\n    return 1\n\n\ndef beta():\n    return 2\n";
const B: &str = "from a import alpha, beta\n\n\ndef run():\n    return alpha() + beta()\n";

struct Fixture {
    _dir: TempDir,
    store: Store,
    workspace: String,
    key: String,
    repo: String,
}

impl Fixture {
    /// A project holding `files`, synced and projected.
    fn new(files: &[(&str, &str)]) -> Self {
        crate::edition_setup();
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("Links").unwrap();
        let workspace = s(&ws, "id").to_string();
        let key = s(&ws, "capability").to_string();
        let registered = store
            .register(
                &workspace,
                &key,
                &json!({"project_key":"links","name":"Links"}),
            )
            .unwrap();
        let repo = s(&registered, "id").to_string();
        let f = Self {
            _dir: dir,
            store,
            workspace,
            key,
            repo,
        };
        f.sync(files);
        f
    }
    fn args(&self, mut args: Value) -> Value {
        args["repository_id"] = json!(self.repo);
        args
    }
    /// Sync `files` and run the projection and link jobs.
    fn sync(&self, files: &[(&str, &str)]) {
        let status = self
            .store
            .project_status(&self.workspace, &self.key, &self.args(json!({})))
            .unwrap();
        let manifest: Vec<Value> = files
            .iter()
            .map(|(name, text)| json!({"path":name,"sha256":graphicle_memory::text::sha256(text.as_bytes()),"bytes":text.len()}))
            .collect();
        let begin = self
            .store
            .sync_prepare(
                &self.workspace,
                &self.key,
                &self.args(json!({"expected_snapshot":status["repository"]["snapshot"],"manifest":manifest})),
            )
            .unwrap();
        if begin["replayed"] != true {
            let upload: Vec<Value> = files
                .iter()
                .map(|(name, text)| json!({"path":name,"content":text}))
                .collect();
            self.store
                .sync_upload(
                    &self.workspace,
                    &self.key,
                    &self.args(json!({"sync_id":begin["sync_id"],"files":upload})),
                )
                .unwrap();
            self.store
                .sync_commit(
                    &self.workspace,
                    &self.key,
                    &self.args(json!({"sync_id":begin["sync_id"]})),
                )
                .unwrap();
        }
        jobs::drain_projection(&self.store, &self.repo).unwrap();
    }
    /// A reviewed memory named for `symbols`, quoting nothing.
    fn note(&self, title: &str, body: &str, symbols: Value) -> Value {
        self.store
            .note(
                &self.workspace,
                &self.key,
                &self.args(json!({"title":title,"body":body,"kind":"decision","symbols":symbols})),
            )
            .unwrap()
    }
    fn context(&self, question: &str, switches: Value) -> Value {
        let mut args = self.args(json!({"question":question}));
        for (key, value) in switches.as_object().unwrap() {
            args[key] = value.clone();
        }
        self.store
            .project_context(&self.workspace, &self.key, &args)
            .unwrap()
    }
    fn node(&self, label: &str) -> Value {
        graph_tools::call(
            &self.store,
            &self.workspace,
            &self.key,
            &self.args(json!({"tool":"get_node","label":label})),
        )
        .unwrap()
    }
}

fn includes(packet: &Value, memory: &Value) -> bool {
    packet["memories"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == memory["id"])
}

#[test]
fn memory_debug_explains_caps_and_packing_without_changing_content() {
    let f = Fixture::new(&[("a.py", A)]);
    for i in 0..6 {
        f.note(
            &format!("Quokka ledger {i}"),
            &format!("{i} {}", "quokka ".repeat(600)),
            json!([{"path":"a.py","qualified_name":"alpha"}]),
        );
    }
    let question = "Which constraints apply to alpha?";
    let plain = f.context(question, json!({"symbol_links":true,"max_tokens":256}));
    assert!(plain.get("memory_candidates_debug").is_none());
    let debug = f.context(
        question,
        json!({"symbol_links":true,"max_tokens":256,"debug_candidates":400}),
    );
    for key in ["memories", "evidence", "markdown", "tokens"] {
        assert_eq!(plain[key], debug[key], "{key}");
    }
    let trace = &debug["memory_candidates_debug"];
    assert_eq!(trace["eligible_candidates"], 6);
    assert_eq!(trace["truncated"], false);
    let rows = trace["candidates"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .filter(|r| r["link_final_rank"].as_u64().is_some_and(|rank| rank > 4))
            .count(),
        2
    );
    assert_eq!(rows.iter().filter(|r| !r["pack_rank"].is_null()).count(), 4);
    assert!(
        rows.iter()
            .any(|r| !r["pack_rank"].is_null() && r["delivered"] == false)
    );
    assert!(!trace.to_string().contains("quokka"));
    let bounded = f.context(question, json!({"symbol_links":true,"debug_candidates":1}));
    assert_eq!(
        bounded["memory_candidates_debug"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(bounded["memory_candidates_debug"]["truncated"], true);
    let disabled = f.context(
        question,
        json!({"symbol_links":true,"debug_candidates":400,"memory_retrieval":false}),
    );
    assert_eq!(
        disabled["memory_candidates_debug"]["eligible_candidates"],
        0
    );
}

#[test]
fn literal_call_values_do_not_expand_linked_decisions() {
    let f = Fixture::new(&[(
        "codec.py",
        "def encode_payload(value):\n    return str(value)\n",
    )]);
    let note = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"codec.py","qualified_name":"encode_payload"}]),
    );
    let value = f.context(
        "What does encode_payload(17) return?",
        json!({"symbol_links":true}),
    );
    assert!(!includes(&value, &note), "{value:#}");
    assert_eq!(value["symbol_links"]["literal_call_value"]["applied"], true);
    assert!(
        value["symbol_links"]["literal_call_value"]["withheld_links"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(!value["evidence"].as_array().unwrap().is_empty());
    let rationale = f.context(
        "What does encode_payload(17) return and why?",
        json!({"symbol_links":true}),
    );
    assert!(includes(&rationale, &note), "{rationale:#}");
    assert_eq!(
        rationale["symbol_links"]["literal_call_value"]["applied"],
        false
    );
}

#[test]
fn covered_prose_topics_do_not_expand_to_dependency_notes() {
    let f = Fixture::new(&[(
        "codec.py",
        "def encode_item(value):\n    # quokka ledger\n    return normalize_item(value)\n\ndef normalize_item(value):\n    return str(value)\n\ndef probe_item():\n    return True\n",
    )]);
    let anchor = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"codec.py","qualified_name":"encode_item"}]),
    );
    let dependency = f.note(
        "Orchid charter",
        "Preserve the orchid charter.",
        json!([{"path":"codec.py","qualified_name":"normalize_item"}]),
    );
    let neighbour = f.note(
        "Cedar orchard",
        "Keep the cedar orchard fenced.",
        json!([{"path":"codec.py","qualified_name":"probe_item"}]),
    );
    let question = "What is the quokka ledger?";
    let packet = f.context(
        question,
        json!({"symbol_links":true,"debug_candidates":400}),
    );
    assert!(
        includes(&packet, &anchor) && !includes(&packet, &dependency),
        "{packet:#}"
    );
    assert!(!includes(&packet, &neighbour), "{packet:#}");
    assert_eq!(
        packet["symbol_links"]["query_symbol_scope"]["basis"],
        "ranked_memories"
    );
    let trace = packet["memory_candidates_debug"]["candidates"]
        .as_array()
        .unwrap();
    assert!(trace.iter().any(|c| {
        c["id"] == neighbour["id"]
            && c["link_outcomes"]["outside_symbol_scope"]
                .as_u64()
                .unwrap_or(0)
                > 0
    }));
    let plain = f.context(question, json!({"symbol_links":true}));
    for key in ["memories", "evidence", "markdown", "tokens"] {
        assert_eq!(packet[key], plain[key], "{key}");
    }
    let multiple = f.context(
        "What are the quokka ledger and cedar orchard?",
        json!({"symbol_links":true}),
    );
    assert!(
        includes(&multiple, &anchor)
            && !includes(&multiple, &dependency)
            && includes(&multiple, &neighbour),
        "{multiple:#}"
    );
    let code = f.context(
        "Which constraints apply to `encode_item`?",
        json!({"symbol_links":true}),
    );
    assert!(includes(&code, &anchor) && includes(&code, &dependency), "{code:#}");
    let explicit = f.context(
        "What governs the quokka ledger and `missing_item`?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        explicit["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    assert!(includes(&explicit, &neighbour), "{explicit:#}");
    // An additional topic absent from the ranked notes must keep fallback,
    // even when the first topic has a complete, live symbol anchor.
    let partial = f.context(
        "What is the quokka ledger and operational observability policy?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        partial["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    assert!(includes(&partial, &neighbour), "{partial:#}");
    let inventory = f.context(
        "What decisions govern this pipeline?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        inventory["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    let unlinked = f.note(
        "Quokka stewardship",
        "The quokka ledger needs approval.",
        json!([]),
    );
    let fallback = f.context(question, json!({"symbol_links":true}));
    assert!(includes(&fallback, &unlinked), "{fallback:#}");
    assert_eq!(
        fallback["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    assert!(includes(&fallback, &neighbour), "{fallback:#}");
}

#[test]
fn shell_scope_keeps_sourced_dependencies_and_literal_caps_keep_only_code() {
    let files = [
        ("deliver.sh", "source \"./lib/policy.sh\"\ndeliver_payload() {\n retry_delay \"$1\" | consume_payload\n}\nconsume_payload() { cat; }\n"),
        ("lib/policy.sh", "retry_delay() {\n local delay=$((1 << attempt))\n if (( delay > 45 )); then\n delay=45\n fi\n printf '%s\\n' \"$delay\"\n}\n"),
        ("preview.sh", "deliver_payload() { echo preview; }\n"),
    ];
    let f = Fixture::new(&files);
    let owner = f.note("Delivery agreement","A change must preserve literal payload bytes.",json!([{"path":"deliver.sh","qualified_name":"deliver_payload"}]));
    let lease = f.note("Delay allowance","The peer permits forty-five seconds to recover.",json!([{"path":"lib/policy.sh","qualified_name":"retry_delay"}]));
    let pipe = f.note("Consumer agreement","A change must preserve failure propagation.",json!([{"path":"deliver.sh","qualified_name":"consume_payload"}]));
    let preview = f.note("Alternate delivery agreement","A change to deliver_payload must preserve the preview prefix.",json!([{"path":"preview.sh","qualified_name":"deliver_payload"}]));
    let question = "Which constraints must a change to deliver_payload in deliver.sh preserve?";
    let packet = f.context(question,json!({"symbol_links":true,"debug_candidates":400}));
    assert!(includes(&packet,&owner) && includes(&packet,&lease) && includes(&packet,&pipe),"{packet:#}");
    assert!(!includes(&packet,&preview),"{packet:#}");
    assert_eq!(packet["memory_scope_support"]["basis"],"shell_dependencies");
    let broad = f.context("Which constraints must a change to deliver_payload preserve?",json!({"symbol_links":true}));
    assert!(includes(&broad,&preview),"{broad:#}");
    for mode in [json!(false),Value::Null,json!(true)] {
        let value = f.context("What cap value does lib/policy.sh assign to delay?",json!({"symbol_links":mode}));
        assert!(value["memories"].as_array().unwrap().is_empty(),"{value:#}");
        assert!(!value["evidence"].as_array().unwrap().is_empty());
        assert_eq!(value["memory_value_request"]["key"],"delay");
    }
    let expanded = f.context(question,json!({"symbol_links":true,"needs":["preview prefix"]}));
    assert!(expanded.get("memory_scope_support").is_none(),"{expanded:#}");
    assert!(includes(&expanded,&preview),"{expanded:#}");
    let mut dynamic = files;
    dynamic[0].1 = "source \"$POLICY\"\ndeliver_payload() { retry_delay \"$1\"; }\n";
    f.sync(&dynamic);
    let unknown = f.context(question,json!({"symbol_links":true}));
    assert!(unknown.get("memory_scope_support").is_none(),"{unknown:#}");
    assert!(includes(&unknown,&preview),"{unknown:#}");
}

#[test]
fn rationale_focus_can_start_from_a_linked_reason_before_the_cap() {
    let f = Fixture::new(&[("delivery.py","def send_packet(attempt):\n    return backoff_delay(attempt)\n\ndef backoff_delay(attempt):\n    return min(2 ** attempt, 60)\n")]);
    let lease = f.note("Peer lifetime","The remote session expires after one minute. Longer pauses abandon the connection.",json!([{"path":"delivery.py","qualified_name":"backoff_delay"}]));
    let audit = f.note("Ledger storage","Keep fiscal archives immutable.",json!([{"path":"delivery.py","qualified_name":"send_packet"}]));
    let packet = f.context("Why does send_packet cap the backoff at sixty seconds?",json!({"symbol_links":true,"debug_candidates":400}));
    assert!(includes(&packet,&lease) && !includes(&packet,&audit),"{packet:#}");
    assert_eq!(packet["memory_rationale_support"]["anchor_route"],"symbol_link","{packet:#}");
    let broad = f.context("Which constraints must a change to send_packet preserve?",json!({"symbol_links":true}));
    assert!(includes(&broad,&lease) && includes(&broad,&audit),"{broad:#}");
}

#[test]
fn same_symbol_quantified_policy_does_not_override_a_supported_rationale() {
    let f = Fixture::new(&[("billing.py","def invoice_total(items):\n    return round(sum(items), 2)\n")]);
    let settlement = f.note("Settlement boundary","Preserve two decimal places at the outbound boundary.",json!([{"path":"billing.py","qualified_name":"invoice_total"}]));
    let audit = f.note("Audit evidence","Finance retains original line items for seven years in an archive.",json!([{"path":"billing.py","qualified_name":"invoice_total"}]));
    let extra = f.note("Processor requirement","Card networks reject fractional minor units.",json!([{"path":"billing.py","qualified_name":"invoice_total"}]));
    let packet = f.context("Why does invoice_total round to two decimal places?",json!({"symbol_links":true}));
    assert!(includes(&packet,&settlement) && includes(&packet,&extra) && !includes(&packet,&audit),"{packet:#}");
    let broad = f.context("Which constraints must a change to invoice_total preserve?",json!({"symbol_links":true}));
    assert!(includes(&broad,&settlement) && includes(&broad,&extra) && includes(&broad,&audit),"{broad:#}");
}

#[test]
fn rationale_support_filters_both_routes_without_overriding_expanded_requests() {
    let f = Fixture::new(&[("queue.py", "def limit():\n    return floor()\n\ndef floor():\n    return 60\n")]);
    let cap = f.note("Backlog reservation", "Reserved capacity covers at most five minutes of backlog.",
        json!([{"path":"queue.py","qualified_name":"limit"}]));
    let floor = f.note("Replay horizon", "Clients need sixty seconds to reconnect.",
        json!([{"path":"queue.py","qualified_name":"floor"}]));
    let reason = f.note("Peer covenant", "Slow peers need room to acknowledge the batch.",
        json!([{"path":"queue.py","qualified_name":"limit"}]));
    let question = "Why does limit() cap backlog at 300 seconds?";
    for mode in [json!(false), Value::Null, json!(true)] {
        let packet = f.context(question, json!({"symbol_links":mode,"debug_candidates":400}));
        assert!(includes(&packet, &cap), "{packet:#}");
        assert!(!includes(&packet, &floor), "{packet:#}");
        assert_eq!(packet["memory_rationale_support"]["basis"], "rationale_duration_agreement", "{packet:#}");
        if mode == true { assert!(includes(&packet, &reason), "{packet:#}"); }
    }
    for switches in [json!({"needs":["reconnection floor"]}), json!({"query_variants":[{"text":"reconnect clients","kind":"query"}]})] {
        let mut switches = switches; switches["symbol_links"] = json!(true);
        let packet = f.context(question, switches);
        assert!(packet.get("memory_rationale_support").is_none(), "{packet:#}");
        assert!(includes(&packet, &floor), "{packet:#}");
    }
}

#[test]
fn helm_helpers_and_value_fields_keep_only_their_dependency_notes() {
    let files = [
        ("Chart.yaml", "apiVersion: v2\nname: parcel\nversion: 0.1.0\n"),
        ("values.yaml", "replicaCount: 2\nservice:\n  port: 8080\nresources:\n  cpu: 100m\n"),
        ("templates/_helpers.tpl", "{{ define \"parcel.labels\" }}\napp: {{ .Release.Name }}\n{{ end }}\n"),
        ("templates/service.yaml", "kind: Service\nselector:\n  {{ include \"parcel.labels\" . }}\nport: {{ .Values.service.port }}\n"),
        ("templates/deployment.yaml", "kind: Deployment\nreplicas: {{ .Values.replicaCount }}\nport: {{ .Values.service.port }}\n"),
    ];
    let f = Fixture::new(&files);
    let labels = f.note("Release isolation", "Selectors must preserve tenant boundaries.",
        json!([{"path":"templates/_helpers.tpl","qualified_name":"parcel.labels"}]));
    let port = f.note("Listener compatibility", "The target must agree with the container port.",
        json!([{"path":"values.yaml","qualified_name":"Values.service.port"}]));
    let replicas = f.note("Availability floor", "Replicas must preserve replacement capacity.",
        json!([{"path":"values.yaml","qualified_name":"Values.replicaCount"}]));
    let cpu = f.note("Compute reservation", "Capacity must preserve a core per replica.",
        json!([{"path":"values.yaml","qualified_name":"Values.resources.cpu"}]));
    for (question, wanted) in [
        ("Which constraints must a change to parcel.labels preserve?", vec![&labels]),
        ("Which constraints must a change to templates/service.yaml preserve?", vec![&labels, &port]),
        ("Which constraints govern .Values.replicaCount?", vec![&replicas]),
        ("Which constraints govern .Values.service?", vec![&port]),
    ] {
        let packet = f.context(question, json!({"symbol_links":true,"debug_candidates":400}));
        assert_eq!(packet["memory_scope_support"]["basis"], "helm_dependencies", "{packet:#}");
        for note in [&labels, &port, &replicas, &cpu] {
            assert_eq!(includes(&packet, note), wanted.iter().any(|n| n["id"] == note["id"]), "{question}: {packet:#}");
        }
    }
    // Computed names and indirect field access preserve the same dependencies.
    for template in [
        r#"{{ include (printf "%s.labels" "parcel") . }}{{ get .Values.service "port" }}"#,
        r#"{{ $helper := "parcel.labels" }}{{ $values := .Values }}{{ include $helper . }}{{ index $values "service" "port" }}"#,
        r#"{{ tpl "{{ include \"parcel.labels\" . }}" . }}{{ .Values | dig "service" "port" 80 }}"#,
        r#"{{ $root := . }}{{ with .Values.service }}{{ .port }}{{ include "parcel.labels" $root }}{{ end }}"#,
    ] {
        let mut dynamic = files;
        dynamic[3].1 = template;
        f.sync(&dynamic);
        let packet = f.context("Which constraints must a change to templates/service.yaml preserve?", json!({"symbol_links":true}));
        assert_eq!(packet["memory_scope_support"]["basis"],"helm_dependencies","{packet:#}");
        assert!(includes(&packet,&labels) && includes(&packet,&port),"{packet:#}");
        assert!(!includes(&packet,&replicas) && !includes(&packet,&cpu),"{packet:#}");
    }
    // The graph must refresh references when a template changes, even though
    // its old helper and values declarations are unchanged.
    let mut changed = files;
    changed[3].1 = "kind: Service\nreplicas: {{ .Values.replicaCount }}\n";
    f.sync(&changed);
    let packet = f.context("Which constraints must a change to templates/service.yaml preserve?", json!({"symbol_links":true}));
    assert!(includes(&packet, &replicas), "{packet:#}");
    assert!(!includes(&packet, &labels) && !includes(&packet, &port), "{packet:#}");
    changed[3].1 = "kind: Service\n{{ include .Values.helper . }}\n";
    f.sync(&changed);
    let packet = f.context("Which constraints must a change to templates/service.yaml preserve?", json!({"symbol_links":true}));
    assert!(packet.get("memory_scope_support").is_none(), "{packet:#}");
    changed[2].1 = "{{ define \"parcel.labels\" }}{{ tpl .Values.labels . }}{{ end }}\n";
    f.sync(&changed);
    let packet = f.context("Which constraints must a change to parcel.labels preserve?", json!({"symbol_links":true}));
    assert!(packet.get("memory_scope_support").is_none(), "{packet:#}");
    assert_eq!(packet["symbol_links"]["query_symbol_scope"]["applied"], false, "{packet:#}");
}

#[test]
fn unsupported_helm_structures_preserve_sync_and_text_retrieval() {
    let f = Fixture::new(&[
        ("Chart.yaml", "apiVersion: v2\nname: parcel\nversion: 0.1.0\n"),
        ("values.yaml", "? [region, zone]\n: custom-placement\n"),
        ("templates/service.yaml", "kind: Service\nport: {{ .Values.port }}\n"),
    ]);
    let packet = f.context("Which constraints govern templates/service.yaml?", json!({"symbol_links":true}));
    assert!(packet.get("memory_scope_support").is_none(), "{packet:#}");
    assert!(!packet["evidence"].as_array().unwrap().is_empty(), "{packet:#}");
}

#[test]
fn configuration_owner_and_named_title_exclude_consumer_notes_but_keep_change_queries_broad() {
    let f = Fixture::new(&[
        ("main.tf", "module \"queue\" {\n source = \"./queue\"\n retention = var.retention_seconds\n}\n"),
        ("variables.tf", "variable \"retention_seconds\" {\n default = 60\n}\n"),
        ("worker.hcl", "job \"worker\" {\n group \"processors\" {\n count = 2\n }\n}\n"),
    ]);
    let contract = f.note("Queue handoff contract", "The queue output stays in seconds.",
        json!([{"path":"main.tf","qualified_name":"module.queue"}]));
    let floor = f.note("Replay horizon", "The queue must keep at least sixty seconds of backlog.",
        json!([{"path":"variables.tf","qualified_name":"var.retention_seconds"}]));
    let availability = f.note("Worker availability", "The processors count must preserve rolling replacement room.",
        json!([{"path":"worker.hcl","qualified_name":"job.worker"}]));
    for mode in [json!(false), Value::Null, json!(true)] {
        let title = f.context("What is the queue handoff contract?", json!({"symbol_links":mode}));
        assert!(includes(&title,&contract) && !includes(&title,&floor), "{title:#}");
        assert_eq!(title["memory_prose_support"]["basis"], "exact_note_title");
        let owner = f.context("Which constraint governs var.retention_seconds in variables.tf?", json!({"symbol_links":mode}));
        assert!(includes(&owner,&floor) && !includes(&owner,&contract), "{owner:#}");
        assert_eq!(owner["memory_scope_support"]["basis"], "configuration_input");
        let count = f.context("What count is assigned to the processors group in worker.hcl?", json!({"symbol_links":mode}));
        assert!(!includes(&count,&availability), "{count:#}");
        assert!(count["memories"].as_array().unwrap().is_empty(), "{count:#}");
        assert!(!count["evidence"].as_array().unwrap().is_empty());
        assert_eq!(count["memory_value_request"]["key"], "count");
    }
    let change = f.context("Which constraints must a change to module.queue preserve?", json!({"symbol_links":true}));
    assert!(includes(&change,&contract) && includes(&change,&floor), "{change:#}");
    let expanded = f.context("Which constraint governs var.retention_seconds in variables.tf?", json!({"symbol_links":true,"needs":["queue handoff contract"]}));
    assert!(expanded.get("memory_scope_support").is_none(), "{expanded:#}");
    assert!(includes(&expanded,&contract) && includes(&expanded,&floor), "{expanded:#}");
}

#[test]
fn configuration_values_keep_source_without_design_memories() {
    let f = Fixture::new(&[("settings.tf", "variable \"timeout\" {\n  default = 60\n}\n")]);
    let note = f.note("Timeout allowance", "The timeout must preserve room for recovery.",
        json!([{"path":"settings.tf","qualified_name":"var.timeout"}]));
    let question = "What default value does settings.tf assign to timeout?";
    for mode in [Value::Null, json!(false), json!(true)] {
        let packet = f.context(question, json!({"symbol_links":mode,"debug_candidates":400}));
        assert!(packet["memories"].as_array().unwrap().is_empty(), "{packet:#}");
        assert!(!packet["evidence"].as_array().unwrap().is_empty(), "{packet:#}");
        assert_eq!(packet["memory_value_request"]["key"], "timeout");
        assert!(packet["memory_candidates_debug"]["candidates"].as_array().unwrap()
            .iter().any(|c| c["id"] == note["id"] && c["selection_filter"] == "configuration_value_request"));
    }
    for (question, switches) in [
        (question, json!({"symbol_links":true,"needs":["recovery room"]})),
        ("What default value does missing.tf assign to timeout?", json!({"symbol_links":true})),
        ("What default value does settings.tf assign to missing_setting?", json!({"symbol_links":true})),
        ("What default value does settings.tf assign to timeout and why?", json!({"symbol_links":true})),
    ] {
        let packet = f.context(question, switches);
        assert!(packet.get("memory_value_request").is_none(), "{packet:#}");
        assert!(includes(&packet, &note), "{packet:#}");
    }
}

#[test]
fn hcl_job_constraints_reach_referenced_variables() {
    let f = Fixture::new(&[
        ("worker.hcl", "job \"worker\" {\n  group \"pool\" {\n    shutdown_delay = \"${var.drain_seconds}s\"\n  }\n}\n"),
        ("variables.hcl", "variable \"drain_seconds\" {\n  default = 45\n}\n"),
        ("other.hcl", "job \"metrics\" {\n  type = \"batch\"\n}\n"),
    ]);
    let drain = f.note("Checkpoint covenant", "Persist outstanding checkpoints before termination.",
        json!([{"path":"variables.hcl","qualified_name":"var.drain_seconds"}]));
    let unrelated = f.note("Reporting cadence", "Archive summaries hourly.",
        json!([{"path":"other.hcl","qualified_name":"job.metrics"}]));
    let owner = f.store.note(&f.workspace, &f.key, &f.args(json!({
        "title":"Worker handoff agreement", "body":"Keep a consumer accepting work.",
        "kind":"decision", "dependencies":["worker.hcl"]
    }))).unwrap();
    let question = "Which constraints must a change to job.worker preserve?";
    let on = f.context(question, json!({"symbol_links":true}));
    assert!(includes(&on, &drain), "{on:#}");
    assert!(!includes(&on, &unrelated), "{on:#}");
    assert_eq!(on["symbol_links"]["query_symbol_scope"]["applied"], true);
    let off = f.context(question, json!({"symbol_links":false}));
    assert!(!includes(&off, &drain), "{off:#}");
    let prose = f.context("What is the worker handoff agreement?", json!({"symbol_links":true}));
    assert!(includes(&prose, &owner), "{prose:#}");
    assert!(!includes(&prose, &drain), "{prose:#}");
    assert_eq!(prose["symbol_links"]["query_symbol_scope"]["basis"], "ranked_memories");
}

#[test]
fn unsupported_attribute_questions_need_more_than_a_symbol_match() {
    let f = Fixture::new(&[(
        "a.py",
        "def alpha():\n    return wait()\n\ndef wait():\n    return 1\n",
    )]);
    let existing = f.note(
        "Alpha result",
        "Keep alpha results stable.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let question = "Which backup schedule is required by alpha()?";
    for mode in [Value::Null, json!(false), json!(true)] {
        let packet = f.context(
            question,
            json!({"symbol_links":mode,"debug_candidates":400}),
        );
        assert!(
            packet["memories"].as_array().unwrap().is_empty(),
            "{packet:#}"
        );
        assert!(
            !packet["evidence"].as_array().unwrap().is_empty(),
            "{packet:#}"
        );
        assert_eq!(
            packet["memory_topic_support"]["status"],
            "unsupported_in_candidates"
        );
        assert!(
            packet["memory_candidates_debug"]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == existing["id"] && c["selection_filter"] == "unsupported_topic")
        );
        let plain = f.context(question, json!({"symbol_links":mode}));
        for key in [
            "memories",
            "evidence",
            "markdown",
            "tokens",
            "memory_topic_support",
        ] {
            assert_eq!(packet[key], plain[key], "{key}");
        }
    }
    for broad in [
        "Which constraints are required by alpha()?",
        "Which compatibility constraints govern alpha()?",
        "Which backup schedule and other policies are required by alpha()?",
        "Why does alpha wait until dawn?",
    ] {
        let packet = f.context(broad, json!({"symbol_links":true}));
        assert!(packet.get("memory_topic_support").is_none(), "{packet:#}");
        assert!(includes(&packet, &existing), "{packet:#}");
    }
    let expanded = f.context(
        question,
        json!({"symbol_links":true,"needs":["stable results"]}),
    );
    assert!(
        expanded.get("memory_topic_support").is_none(),
        "{expanded:#}"
    );
    assert!(includes(&expanded, &existing), "{expanded:#}");
    // Support may live in a callee's note, and inflections must still match.
    // Reuse the unchanged question to prove this is evidence-driven abstention.
    let supporting = f.note(
        "Backup scheduling",
        "Take snapshots nightly.",
        json!([{"path":"a.py","qualified_name":"wait"}]),
    );
    let packet = f.context(question, json!({"symbol_links":true}));
    assert_eq!(packet["memory_topic_support"]["status"], "supported");
    assert!(includes(&packet, &supporting), "{packet:#}");
}

#[test]
fn source_topic_evidence_preserves_dependency_memory() {
    let f = Fixture::new(&[(
        "a.py",
        "def alpha():\n    # backup schedule is computed here\n    return wait()\n\ndef wait():\n    return 1\n",
    )]);
    let note = f.note(
        "Restoration covenant",
        "A cold restore needs an intact snapshot.",
        json!([{"path":"a.py","qualified_name":"wait"}]),
    );
    let packet = f.context(
        "Which backup schedule is required by alpha()?",
        json!({"symbol_links":true}),
    );
    assert_eq!(packet["memory_topic_support"]["status"], "supported");
    assert!(includes(&packet, &note), "{packet:#}");
}

#[test]
fn explicit_symbols_do_not_lift_unrelated_neighbour_notes() {
    let f = Fixture::new(&[("a.py", A)]);
    let alpha = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let beta = f.note(
        "Wombat archive",
        "Preserve the wombat archive.",
        json!([{"path":"a.py","qualified_name":"beta"}]),
    );
    let one = f.context(
        "Which constraints apply to `alpha`?",
        json!({"symbol_links":true}),
    );
    assert!(includes(&one, &alpha), "{one:#}");
    assert!(!includes(&one, &beta), "{one:#}");
    assert_eq!(one["symbol_links"]["query_symbol_scope"]["applied"], true);
    let both = f.context(
        "Which constraints apply to `alpha` and `beta`?",
        json!({"symbol_links":true}),
    );
    assert!(
        includes(&both, &alpha) && includes(&both, &beta),
        "{both:#}"
    );
    let unresolved = f.context(
        "Which constraints apply to `alpha` and `unknown`?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        unresolved["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    assert!(
        includes(&unresolved, &alpha) && includes(&unresolved, &beta),
        "{unresolved:#}"
    );
}

#[test]
fn symbol_scope_keeps_callers_and_transitive_callees_across_files() {
    let f = Fixture::new(&[
        (
            "client.py",
            "from codec import encode_item\n\ndef publish_item(value):\n    return encode_item(value)\n",
        ),
        (
            "codec.py",
            "def encode_item(value):\n    return normalize_item(value)\n\ndef normalize_item(value):\n    return str(value)\n\ndef health_probe():\n    return True\n",
        ),
    ]);
    let caller = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"client.py","qualified_name":"publish_item"}]),
    );
    let callee = f.note(
        "Wombat archive",
        "Preserve the wombat archive.",
        json!([{"path":"codec.py","qualified_name":"normalize_item"}]),
    );
    let unrelated = f.note(
        "Cedar orchard",
        "Keep the cedar orchard fenced.",
        json!([{"path":"codec.py","qualified_name":"health_probe"}]),
    );
    for name in ["publish_item", "normalize_item"] {
        let packet = f.context(
            &format!("Which constraints govern {name}?"),
            json!({"symbol_links":true}),
        );
        assert!(
            includes(&packet, &caller) && includes(&packet, &callee),
            "{packet:#}"
        );
        assert!(!includes(&packet, &unrelated), "{packet:#}");
        assert_eq!(
            packet["symbol_links"]["query_symbol_scope"]["applied"],
            true
        );
    }
}

#[test]
fn symbol_scope_does_not_guess_ambiguous_names() {
    let f = Fixture::new(&[
        ("left/a.py", A),
        ("right/a.py", &A.replace("return 1", "return 10")),
    ]);
    let left = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"left/a.py","qualified_name":"alpha"}]),
    );
    let right = f.note(
        "Wombat archive",
        "Preserve the wombat archive.",
        json!([{"path":"right/a.py","qualified_name":"alpha"}]),
    );
    let ambiguous = f.context(
        "Which constraints apply to `alpha`?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        ambiguous["symbol_links"]["query_symbol_scope"]["applied"],
        true
    );
    assert!(
        includes(&ambiguous, &left) && includes(&ambiguous, &right),
        "{ambiguous:#}"
    );
    let exact = f.context(
        "Which constraints apply to `alpha` in left/a.py?",
        json!({"symbol_links":true}),
    );
    assert!(
        includes(&exact, &left) && !includes(&exact, &right),
        "{exact:#}"
    );
}

#[test]
fn method_nesting_does_not_make_sibling_notes_relevant() {
    let f = Fixture::new(&[(
        "writer.py",
        "class Writer:\n    def serialize(self, value):\n        return str(value)\n\n    def probe(self):\n        return True\n",
    )]);
    let selected = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"writer.py","qualified_name":"Writer.serialize"}]),
    );
    let sibling = f.note(
        "Cedar orchard",
        "Keep the cedar orchard fenced.",
        json!([{"path":"writer.py","qualified_name":"Writer.probe"}]),
    );
    let owner = f.note(
        "Orchid charter",
        "Preserve the orchid charter.",
        json!([{"path":"writer.py","qualified_name":"Writer"}]),
    );
    let packet = f.context(
        "Which constraints apply to Writer.serialize?",
        json!({"symbol_links":true}),
    );
    assert!(includes(&packet, &selected), "{packet:#}");
    assert!(includes(&packet, &owner), "{packet:#}");
    assert!(!includes(&packet, &sibling), "{packet:#}");
}

#[test]
fn equivalent_durations_can_rescue_a_late_linked_constraint() {
    let f = Fixture::new(&[(
        "sender.py",
        "def send_record():\n    return delay_record()\n\ndef delay_record():\n    return 120\n",
    )]);
    for i in 0..4 {
        f.note(
            &format!("Archive policy {i}"),
            &format!("Preserve fiscal document {i}."),
            json!([{"path":"sender.py","qualified_name":"send_record"}]),
        );
    }
    let needed = f.note(
        "Peer lifecycle",
        "The remote lease ends after 120 seconds.",
        json!([{"path":"sender.py","qualified_name":"delay_record"}]),
    );
    let packet = f.context(
        "Why does send_record wait two minutes?",
        json!({"symbol_links":true,"debug_candidates":400}),
    );
    assert!(includes(&packet, &needed), "{packet:#}");
    assert_eq!(packet["symbol_links"]["candidate_memories_before_filters"], 5);
    assert_eq!(packet["symbol_links"]["candidate_memories"], 1);
    assert_eq!(packet["memory_rationale_support"]["withheld_linked"], 4);
    assert_eq!(packet["symbol_links"]["memory_limit"], 4);
    let trace = packet["memory_candidates_debug"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == needed["id"])
        .unwrap();
    assert_eq!(trace["link_final_rank"], 1);
}

#[test]
fn project_decision_inventories_use_the_request_allowance_and_packing_budget() {
    let f = Fixture::new(&[("service.py", A)]);
    let notes: Vec<Value> = (0..6)
        .map(|i| {
            f.note(
                &format!("Ledger {i}"),
                &format!("Preserve the quokka archive {i}."),
                json!([{"path":"service.py","qualified_name":"alpha"}]),
            )
        })
        .collect();
    let question = "List all the decisions for this service.";
    let packet = f.context(
        question,
        json!({"symbol_links":true,"debug_candidates":400}),
    );
    assert!(notes.iter().all(|n| includes(&packet, n)), "{packet:#}");
    assert_eq!(packet["symbol_links"]["memory_limit"], 8);
    assert_eq!(packet["memory_candidates_debug"]["linked_memory_limit"], 8);
    let small = f.context(question, json!({"symbol_links":true,"max_tokens":256}));
    assert!(small["tokens"].as_u64().unwrap() <= 256);
    assert!(small["omitted"]["memories"].as_u64().unwrap() > 0);
}

#[test]
fn an_oversized_dependency_component_keeps_window_expansion() {
    let mut code = String::new();
    for i in 0..130 {
        code.push_str(&format!("def step_{i}():\n    return step_{}()\n\n", i + 1));
    }
    code.push_str("def step_130():\n    return 1\n");
    let f = Fixture::new(&[("chain.py", &code)]);
    let note = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"chain.py","qualified_name":"step_0"}]),
    );
    let packet = f.context(
        "Which constraints govern step_0?",
        json!({"symbol_links":true}),
    );
    assert_eq!(
        packet["symbol_links"]["query_symbol_scope"]["applied"],
        false
    );
    assert!(includes(&packet, &note), "{packet:#}");
}

#[test]
fn a_code_window_lifts_its_linked_memory_only_when_asked() {
    let f = Fixture::new(&[("a.py", A), ("b.py", B)]);
    let memory = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let question = "What does alpha return?";
    // Left out: the link is counted and nothing is lifted.
    let counted = f.context(question, json!({}));
    assert!(!includes(&counted, &memory), "{:#}", counted["memories"]);
    let report = &counted["symbol_links"];
    assert_eq!(report["status"], "counted", "{report:#}");
    assert_eq!(report["withheld"], 1, "{report:#}");
    assert_eq!(report["lifted_memories"], 0);
    assert_eq!(counted["mechanisms"]["symbol_links_enabled"], false);
    assert_eq!(counted["mechanisms"]["links_used"]["memory"], 0);
    // false: no link is read.
    let off = f.context(question, json!({"symbol_links":false}));
    assert!(!includes(&off, &memory));
    assert_eq!(off["symbol_links"], json!({"status":"disabled"}));
    // true: the code window lifts the memory, one hop.
    let on = f.context(question, json!({"symbol_links":true}));
    let lifted = on["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == memory["id"])
        .cloned()
        .unwrap_or_else(|| panic!("{:#}", on["symbol_links"]));
    assert_eq!(lifted["via"], "link");
    let link = &lifted["link"];
    assert_eq!(link["direction"], "code_to_memory");
    assert_eq!(link["qualified_name"], "alpha");
    assert_eq!(link["rung"], "named");
    assert!(
        link["from"].as_str().unwrap().starts_with("file:a.py"),
        "{link}"
    );
    assert!(s(&on, "markdown").contains("Quokka ledger"));
    let report = &on["symbol_links"];
    assert_eq!(report["status"], "followed", "{report:#}");
    assert_eq!(
        (&report["lifted_memories"], &report["memories_included"]),
        (&json!(1), &json!(1)),
        "{report:#}"
    );
    let mechanisms = &on["mechanisms"];
    assert_eq!(mechanisms["symbol_links_enabled"], true);
    assert_eq!(mechanisms["links_used"]["memory"], 1);
    assert_eq!(mechanisms["links_used"]["by_rung"]["named"], 1);
    // The same packet otherwise: the code evidence is unchanged.
    let ids = |packet: &Value| -> Vec<Value> {
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!([e["id"], e["byte_start"], e["byte_end"]]))
            .collect()
    };
    assert_eq!(ids(&on), ids(&counted));
    assert_eq!(ids(&off), ids(&counted));

    // An edit elsewhere in the file keeps the memory current and linked.
    let elsewhere = A.replace("return 2", "return 20");
    f.sync(&[("a.py", &elsewhere), ("b.py", B)]);
    assert!(includes(
        &f.context(question, json!({"symbol_links":true})),
        &memory
    ));
    // An edit inside alpha withholds it: never lifted.
    f.sync(&[
        ("a.py", &elsewhere.replace("return 1", "return 10")),
        ("b.py", B),
    ]);
    let stale = f.context(question, json!({"symbol_links":true}));
    assert!(!includes(&stale, &memory), "{:#}", stale["memories"]);
    assert_eq!(stale["mechanisms"]["memories_withheld"]["by_symbol"], 1);
    assert_eq!(stale["mechanisms"]["links_used"]["memory"], 0);
}

/// The identifier route names, in one line, the current reviewed memories
/// linked to the symbols it cites; a call that leaves memory out gets no
/// such line.
#[test]
fn the_identifier_route_names_linked_reviewed_memory() {
    let f = Fixture::new(&[("a.py", A), ("b.py", B)]);
    let memory = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let packet = f.context("where is `alpha` defined", json!({"identifier_route":true}));
    assert_eq!(packet["route"]["path"], "identifier", "{packet:#}");
    assert_eq!(
        packet["linked_memories"][0]["id"], memory["id"],
        "{packet:#}"
    );
    let markdown = s(&packet, "markdown");
    assert!(
        markdown.contains("Reviewed memory linked to these symbols: Quokka ledger (memory "),
        "{markdown}"
    );
    let off = f.context(
        "where is `alpha` defined",
        json!({"identifier_route":true,"memory_retrieval":false}),
    );
    assert_eq!(off["route"]["path"], "identifier");
    assert!(!s(&off, "markdown").contains("Quokka ledger"), "{off:#}");
    // A symbol no memory links: no line.
    let beta = f.context("who calls `beta`", json!({"identifier_route":true}));
    assert!(
        !s(&beta, "markdown").contains("Reviewed memory"),
        "{beta:#}"
    );
}

#[test]
fn a_selected_memory_lifts_the_window_of_its_symbol() {
    let f = Fixture::new(&[("a.py", A), ("b.py", B)]);
    let memory = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"beta"}]),
    );
    let lifted = |packet: &Value| {
        packet["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e.get("symbol_links").is_some())
            .cloned()
    };
    // With the graph on, the property graph of sources and reviewed knowledge
    // already brings a.py (the memory's file) whole: its link is covered and
    // nothing is lifted twice.
    let covered = f.context("quokka ledger", json!({"symbol_links":true}));
    assert!(includes(&covered, &memory));
    assert!(lifted(&covered).is_none());
    let report = &covered["symbol_links"];
    assert_eq!(
        (&report["covered"], &report["lifted_windows"]),
        (&json!(2), &json!(0)),
        "{report:#}"
    );
    // With the graph off, only the link can bring the code.
    let counted = f.context("quokka ledger", json!({"graph_retrieval":false}));
    assert!(includes(&counted, &memory), "{:#}", counted["memories"]);
    assert!(lifted(&counted).is_none());
    assert_eq!(counted["evidence"], json!([]), "{:#}", counted["evidence"]);
    assert_eq!(
        counted["symbol_links"]["withheld"], 1,
        "{:#}",
        counted["symbol_links"]
    );
    let on = f.context(
        "quokka ledger",
        json!({"graph_retrieval":false,"symbol_links":true}),
    );
    let window = lifted(&on).unwrap_or_else(|| panic!("{:#}", on["symbol_links"]));
    assert_eq!(window["id"], "file:a.py");
    let facts = &window["symbol_links"];
    assert_eq!(facts["direction"], "memory_to_code");
    assert_eq!(facts["from"], format!("memory:{}", s(&memory, "id")));
    assert_eq!(facts["qualified_name"], "beta");
    let (a, b) = (
        window["byte_start"].as_u64().unwrap() as usize,
        window["byte_end"].as_u64().unwrap() as usize,
    );
    // The cited window holds beta (the packet may widen a short window).
    assert!(
        A[a..b].contains("def beta():\n    return 2\n"),
        "{window:#}"
    );
    assert_eq!(on["symbol_links"]["lifted_windows"], 1);
    assert_eq!(on["symbol_links"]["cited"], 1);
    assert_eq!(on["mechanisms"]["links_used"]["memory"], 1);
    // A memory without code links lifts nothing and reads no link.
    let g = Fixture::new(&[("a.py", A)]);
    g.store
        .note(
            &g.workspace,
            &g.key,
            &g.args(json!({"title":"Quokka ledger","body":"Keep the quokka ledger frozen."})),
        )
        .unwrap();
    let plain = g.context("quokka ledger", json!({"symbol_links":true}));
    assert_eq!(plain["symbol_links"], json!({"status":"no_links"}));
    // Its packet keeps its evidence and memories whatever the switch says.
    let default = g.context("quokka ledger", json!({}));
    assert_eq!(default["symbol_links"], json!({"status":"no_links"}));
    assert_eq!(plain["evidence"], default["evidence"]);
    assert_eq!(plain["memories"], default["memories"]);
    assert_eq!(plain["markdown"], default["markdown"]);
}

#[test]
fn get_node_lists_the_linked_memory_and_documents() {
    let readme = "# Notes\n\nCall `alpha` before anything else.\n";
    let files = [("a.py", A), ("b.py", B), ("README.md", readme)];
    let f = Fixture::new(&files);
    let memory = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let reply = f.node("alpha");
    let node = &reply["nodes"][0];
    assert_eq!(node["label"], "alpha()", "{reply:#}");
    let linked = &node["linked"];
    let memories = linked["memories"].as_array().unwrap();
    assert_eq!(memories.len(), 1, "{linked:#}");
    assert_eq!(memories[0]["id"], memory["id"]);
    assert_eq!(memories[0]["title"], "Quokka ledger");
    assert_eq!(memories[0]["rung"], "named");
    let documents = linked["documents"].as_array().unwrap();
    assert_eq!(documents.len(), 1, "{linked:#}");
    assert_eq!(documents[0]["document_id"], "file:README.md");
    let text = graph_tools::citations_text(&reply);
    assert!(
        text.contains(&format!(
            "reviewed memory linked: Quokka ledger ({})",
            s(&memory, "id")
        )),
        "{text}"
    );
    assert!(
        text.contains("documents mentioning it: file:README.md"),
        "{text}"
    );
    // Nothing links to beta.
    assert_eq!(
        f.node("beta")["nodes"][0]["linked"],
        json!({"memories":[],"documents":[]})
    );
    // A stale memory is not listed.
    f.sync(&[
        ("a.py", &A.replace("return 1", "return 10")),
        ("b.py", B),
        ("README.md", readme),
    ]);
    assert_eq!(f.node("alpha")["nodes"][0]["linked"]["memories"], json!([]));
}

/// The answer model quotes the first source it was given.
async fn inference(Json(request): Json<Value>) -> Json<Value> {
    let input: Value = serde_json::from_str(request["input"].as_str().unwrap()).unwrap();
    let source = &input["sources"][0];
    let answer = json!({"answer":"from the first source","abstain":false,
        "evidence":[{"id":source["id"],"quote":source["text"]}],"unresolved":[]});
    Json(
        json!({"status":"completed","model":"fixture","usage":{"input_tokens":150,"output_tokens":80},"output":[{"type":"message","content":[{"type":"output_text","text":answer.to_string()}]}]}),
    )
}

impl Fixture {
    /// Queue a `basic` answer and run it as a worker would: its retrieval
    /// packet.
    async fn basic(&self, config: &model::Config, mut args: Value) -> Value {
        args["kind"] = json!("basic");
        args["repository_id"] = json!(self.repo);
        let queued = self
            .store
            .enqueue(&self.workspace, &self.key, &args, config)
            .unwrap();
        Worker::new(self.store.clone(), config.clone())
            .unwrap()
            .once()
            .await
            .unwrap();
        let done = self
            .store
            .job_status(
                &self.workspace,
                &self.key,
                &json!({"repository_id":self.repo,"job_id":queued["id"]}),
            )
            .unwrap();
        assert_eq!(done["state"], "complete", "{done}");
        done["result"]["context"].clone()
    }
}

/// The answer model of [`inference`], served on a loopback address, and
/// the configuration that names it.
async fn answer_model() -> (model::Config, tokio::task::JoinHandle<()>) {
    // IPv6 loopback first: some build machines refuse IPv4 loopback.
    let mut bound = None;
    for address in ["[::1]:0", "127.0.0.1:0"] {
        if let Ok(listener) = tokio::net::TcpListener::bind(address).await {
            bound = Some(listener);
            break;
        }
    }
    let listener = bound.expect("a loopback address to serve the answer model on");
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route("/v1/responses", axum::routing::post(inference));
        axum::serve(listener, app).await.unwrap();
    });
    let config: model::Config = serde_json::from_value(json!({"default_model":"fast","models":{
        "fast":{"protocol":"responses","base_url":format!("http://{address}/v1"),"model":"fixture","input_limit":16000,"output_limit":4096}
    }}))
    .unwrap();
    (config, server)
}

/// Answers take `notes_first` like `context`: left out, the answer's
/// packet is 0.1.2's; `true` orders a question in prose notes first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_take_the_notes_first_switch() {
    let f = Fixture::new(&[
        ("a.py", A),
        ("b.py", B),
        ("README.md", "# Notes\n\nalpha returns one.\n"),
    ]);
    let (config, server) = answer_model().await;
    let question = "What does alpha return?";
    let plain = f.basic(&config, json!({"question":question})).await;
    assert_eq!(
        plain["notes_first"],
        json!({"status":"off","defaulted":true}),
        "{}",
        plain["notes_first"]
    );
    let first = f
        .basic(&config, json!({"question":question,"notes_first":true}))
        .await;
    assert_eq!(
        first["notes_first"]["status"], "reranked",
        "{}",
        first["notes_first"]
    );
    assert_eq!(first["notes_first"]["defaulted"], false);
    assert_eq!(first["mechanisms"]["switches"]["notes_first"], true);
    server.abort();
}

/// Answers take `symbol_links` whatever `doc_links` says: with documents'
/// links off, `symbol_links: true` still lifts the reviewed memory linked to
/// the code window, reports `followed`, and the echo shows the values in
/// force. Left out, the link is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn answers_follow_memory_links_with_document_links_off() {
    let f = Fixture::new(&[("a.py", A), ("b.py", B)]);
    let memory = f.note(
        "Quokka ledger",
        "Keep the quokka ledger frozen.",
        json!([{"path":"a.py","qualified_name":"alpha"}]),
    );
    let (config, server) = answer_model().await;
    let question = "What does alpha return?";
    let counted = f
        .basic(&config, json!({"question":question,"doc_links":false}))
        .await;
    assert!(!includes(&counted, &memory), "{:#}", counted["memories"]);
    assert_eq!(
        counted["symbol_links"]["status"], "counted",
        "{:#}",
        counted["symbol_links"]
    );
    assert_eq!(counted["symbol_links"]["withheld"], 1);
    assert_eq!(counted["mechanisms"]["switches"]["symbol_links"], "counted");
    let on = f
        .basic(
            &config,
            json!({"question":question,"doc_links":false,"symbol_links":true}),
        )
        .await;
    let lifted = on["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == memory["id"])
        .cloned()
        .unwrap_or_else(|| panic!("{:#}", on["symbol_links"]));
    assert_eq!(lifted["via"], "link");
    assert_eq!(lifted["link"]["direction"], "code_to_memory");
    let report = &on["symbol_links"];
    assert_eq!(report["status"], "followed", "{report:#}");
    assert_eq!(report["lifted_memories"], 1, "{report:#}");
    let mechanisms = &on["mechanisms"];
    assert_eq!(mechanisms["symbol_links_enabled"], true);
    assert_eq!(mechanisms["links_used"]["memory"], 1);
    assert_eq!(mechanisms["switches"]["symbol_links"], true);
    assert_eq!(mechanisms["switches"]["doc_links"], false);
    assert_eq!(on["doc_links"]["status"], "disabled");
    assert_eq!(on["integration"]["symbol_links"], true);
    server.abort();
}
