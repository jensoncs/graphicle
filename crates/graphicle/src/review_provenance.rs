//! Packet-local, content-addressed configured service assertions. Definitions
//! retain exact pinned citations; this encoding adds no runtime evidence.
use super::*;

#[derive(Default)]
pub(super) struct Catalog {
    definitions: BTreeMap<String, Value>,
    costs: BTreeMap<String, usize>,
}

impl Catalog {
    fn intern(&mut self, mut value: Value) -> String {
        for field in ["source", "target"] {
            if let Some(child) = value.as_object_mut().and_then(|v| v.remove(field)) {
                let id = self.intern(child);
                value[format!("{field}_ref")] = json!(id);
            }
        }
        let id = crate::digest(&value);
        self.costs
            .entry(id.clone())
            .or_insert_with(|| text::count(&json!({&id: &value}).to_string()) + 4);
        self.definitions.insert(id.clone(), value);
        id
    }

    pub(super) fn prepare(entries: &mut [Value]) -> Self {
        let mut catalog = Self::default();
        let mut compact = entries.to_vec();
        let mut before = 0;
        let mut after = 0;
        for entry in &mut compact {
            if let Some(boundary) = entry.get_mut("service_boundary") {
                before += text::count(&boundary.to_string());
                let mut assertion = boundary.clone();
                assertion.as_object_mut().unwrap().remove("resolution");
                let id = catalog.intern(assertion);
                // Keep identity, transport and resolution directly readable.
                // Only the repeated source citations move into the table.
                boundary.as_object_mut().unwrap().remove("citations");
                boundary.as_object_mut().unwrap().remove("manifest");
                boundary["provenance_ref"] = json!(id);
                after += text::count(&boundary.to_string());
            }
            for field in ["reasons", "_execution_flow_reasons"] {
                for reason in entry
                    .get_mut(field)
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    for step in reason
                        .get_mut("steps")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        if step["evidence"] != "configured_static" {
                            continue;
                        }
                        if let Some(value) = step.as_object_mut().unwrap().remove("provenance") {
                            // Flow annotations are alternative versions, not extra uses.
                            if field == "reasons" {
                                before += text::count(&value.to_string());
                            }
                            let id = catalog.intern(value);
                            step["provenance_ref"] = json!(id);
                            if field == "reasons" {
                                after += text::count(&step["provenance_ref"].to_string()) + 4;
                            }
                        }
                    }
                }
            }
        }
        // No caller-specific service settings: use references only when shared
        // assertions save tokens across the candidate evidence.
        if before <= after + catalog.costs.values().sum::<usize>() + 32 {
            return Self::default();
        }
        entries.clone_from_slice(&compact);
        for entry in entries {
            entry.as_object_mut().unwrap().remove("revision");
            entry["revision"] = json!(crate::digest(entry));
        }
        catalog
    }

    pub(super) fn enabled(&self) -> bool {
        !self.definitions.is_empty()
    }

    fn include(&self, id: &str, refs: &mut BTreeSet<String>) {
        if !refs.insert(id.to_owned()) {
            return;
        }
        let definition = &self.definitions[id];
        for field in ["source_ref", "target_ref"] {
            if let Some(child) = definition[field].as_str() {
                self.include(child, refs);
            }
        }
    }

    pub(super) fn references(&self, entry: &Value, flow: Option<&Value>) -> BTreeSet<String> {
        let mut refs = BTreeSet::new();
        if let Some(id) = entry["service_boundary"]["provenance_ref"].as_str() {
            self.include(id, &mut refs);
        }
        for reasons in [Some(&entry["reasons"]), flow].into_iter().flatten() {
            for reason in reasons.as_array().into_iter().flatten() {
                for step in reason["steps"].as_array().into_iter().flatten() {
                    if let Some(id) = step["provenance_ref"].as_str() {
                        self.include(id, &mut refs);
                    }
                }
            }
        }
        refs
    }

    pub(super) fn cost(&self, refs: &BTreeSet<String>) -> usize {
        refs.iter().map(|id| self.costs[id]).sum()
    }

    pub(super) fn emit(&self, report: &mut Value, refs: &BTreeSet<String>) {
        if !self.enabled() {
            return;
        }
        let definitions: BTreeMap<_, _> =
            refs.iter().map(|id| (id, &self.definitions[id])).collect();
        report["service_provenance"] =
            json!({"format":"graphicle.service-provenance.v1", "definitions":definitions});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assertion(binding: &str) -> Value {
        json!({"binding":binding,"authority":"pinned_configuration_assertion","runtime_observed":false,
            "citations":[{"repository_id":"repo","commit":"a".repeat(40),"snapshot":"b".repeat(64),
                "policy_sha256":"c".repeat(64),"file":"服务.go","sha256":"d".repeat(64),"byte_start":0,"byte_end":100}],
            "manifest":{"file":"graphicle-services.json","sha256":"e".repeat(64)}})
    }

    fn report() -> Value {
        let source = assertion("out");
        let target = assertion("in");
        let reason = json!({"reason":"dependency","path":["a","b"],"steps":[
            {"evidence":"configured_static","relation":"http_calls","caller":"a","callee":"b",
             "provenance":{"source":source,"target":target,"evidence":"configured_static"}}]});
        let entries: Vec<_> = (0..12)
            .map(|i| {
                let mut flow = reason.clone();
                flow["execution_flow"] = json!({"kind":"potential_static","observed":false});
                json!({"id":format!("e{i}"),"side":"head","revision":"old","text":"func Work() {}","service_boundary":source,
                "reasons":[reason],"_execution_flow_reasons":[flow]})
            })
            .collect();
        json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},
            "coverage":{"complete":true,"issues":[]}})
    }

    #[test]
    fn automatic_compaction_is_lossless_and_charges_shared_definitions() {
        let plain = report();
        let mut compact = plain.clone();
        compact["_compact_provenance"] = json!(true);
        let (raw, _) = pack(plain, 32000, None, None).unwrap();
        let (full, receipt) = pack(compact.clone(), 32000, None, None).unwrap();
        assert!(full["tokens"].as_u64().unwrap() < raw["tokens"].as_u64().unwrap());
        assert_eq!(full["context_order"], raw["context_order"]);
        let definitions = &full["service_provenance"]["definitions"];
        assert_eq!(definitions.as_object().unwrap().len(), 3);
        for (key, value) in definitions.as_object().unwrap() {
            assert_eq!(key, &crate::digest(value));
        }
        let ref_id = full["entries"][0]["reasons"][0]["steps"][0]["provenance_ref"]
            .as_str()
            .unwrap();
        let mut expanded = definitions[ref_id].clone();
        for field in ["source", "target"] {
            let id = expanded
                .as_object_mut()
                .unwrap()
                .remove(&format!("{field}_ref"))
                .unwrap();
            expanded[field] = definitions[id.as_str().unwrap()].clone();
        }
        assert_eq!(
            expanded,
            raw["entries"][0]["reasons"][0]["steps"][0]["provenance"]
        );
        let (delta, next) = pack(compact.clone(), 32000, Some(&receipt), None).unwrap();
        assert_eq!(delta["service_provenance"], full["service_provenance"]);
        assert_eq!(receipt["entries"], next["entries"]);
        assert!(delta["entries"].as_array().unwrap().is_empty());
        for budget in [2048, 4096, 8000] {
            let (bounded, _) = pack(compact.clone(), budget, None, None).unwrap();
            assert!(text::count(&bounded.to_string()) <= budget);
            let defs = &bounded["service_provenance"]["definitions"];
            for e in bounded["entries"].as_array().unwrap() {
                for r in e["reasons"].as_array().unwrap() {
                    for step in r["steps"].as_array().unwrap() {
                        if let Some(id) = step["provenance_ref"].as_str() {
                            assert!(defs.get(id).is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn code_only_and_unshared_small_assertions_stay_inline() {
        for reasons in [
            json!([]),
            json!([{"steps":[{"evidence":"configured_static","provenance":{"binding":"one"}}]}]),
        ] {
            let mut entries = vec![json!({"id":"one","reasons":reasons,"revision":"original"})];
            let original = entries.clone();
            assert!(!Catalog::prepare(&mut entries).enabled());
            assert_eq!(entries, original);
        }
    }

    #[test]
    fn tight_budget_automatically_keeps_inline_required_code_and_stable_deltas() {
        let mut input = report();
        for entry in input["entries"].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("service_boundary");
        }
        input["entries"][0]["reasons"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"reason":"change_seed"}));
        let mut candidates = input["entries"].as_array().unwrap().clone();
        Catalog::prepare(&mut candidates);
        assert_eq!(candidates[0]["reasons"][0], json!({"reason":"change_seed"}));
        input["_compact_provenance"] = json!(true);
        let (full, receipt) = pack(input.clone(), 1500, None, None).unwrap();
        assert!(full.get("service_provenance").is_none());
        assert_eq!(full["coverage"]["current_head"]["selected"], 1);
        let (delta, next) = pack(input, 1500, Some(&receipt), None).unwrap();
        assert_eq!(receipt["entries"], next["entries"]);
        assert_eq!(full["context_order"], delta["context_order"]);
        assert!(delta.get("service_provenance").is_none());
    }

    #[test]
    fn changed_citation_changes_reference_and_entry_revision() {
        let mut first = report()["entries"].as_array().unwrap().clone();
        let mut next = first.clone();
        next[0]["reasons"][0]["steps"][0]["provenance"]["source"]["citations"][0]["sha256"] =
            json!("changed");
        Catalog::prepare(&mut first);
        Catalog::prepare(&mut next);
        assert_ne!(first[0]["revision"], next[0]["revision"]);
        assert_ne!(
            first[0]["reasons"][0]["steps"][0]["provenance_ref"],
            next[0]["reasons"][0]["steps"][0]["provenance_ref"]
        );
    }
}
