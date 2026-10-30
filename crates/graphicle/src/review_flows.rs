//! Flow views reuse the selected dependency paths. Never treat references to
//! types/values/callables as invocations or claim paths are runtime traces.
use super::*;

pub(super) fn annotate(entries: &mut [Value], base: &Input, head: &Input) {
    for entry in entries {
        let input = if entry["side"] == "head" { head } else { base };
        let Some(mut reasons) = entry["reasons"].as_array().cloned() else {
            continue;
        };
        for reason in &mut reasons {
            let Some(steps) = reason["steps"].as_array() else {
                continue;
            };
            if steps.is_empty()
                || !steps.iter().all(|s| {
                    matches!(
                        s["relation"].as_str(),
                        Some(
                            "calls"
                                | "may_call"
                                | "invokes_boundary"
                                | "handles_boundary"
                                | "http_calls"
                                | "grpc_calls"
                                | "event_delivers"
                        )
                    )
                })
            {
                continue;
            }
            let mut ordered = steps.clone();
            if reason["reason"] == "potential_consumer" {
                ordered.reverse();
            }
            if !ordered.windows(2).all(|w| w[0]["callee"] == w[1]["caller"]) {
                continue;
            }
            let mut complete = true;
            for step in &mut ordered {
                if step["evidence"] == "configured_static" {
                    continue;
                }
                let Some(source) = input.sources.get(s(step, "source_file")) else {
                    complete = false;
                    continue;
                };
                let Some(pin) = input.pins.iter().find(|p| {
                    p.repository_id == source.repository
                        && s(step, "source_file").starts_with(&format!("{}/", p.directory))
                }) else {
                    complete = false;
                    continue;
                };
                step["citation"] = json!({"repository_id":source.repository,"commit":pin.commit,"snapshot":pin.snapshot,"policy_sha256":pin.policy_sha256,"file":source.path,"sha256":source.sha,"byte_start":step["byte_start"],"byte_end":step["byte_end"]});
                if step["relation"] == "may_call" {
                    step["evidence"] = json!("function_value_candidate");
                }
            }
            // Replace the same steps with ordered steps and describe their order;
            // do not duplicate source evidence or bypass the normal packet budget.
            let mut nodes = vec![ordered[0]["caller"].clone()];
            nodes.extend(ordered.iter().map(|s| s["callee"].clone()));
            reason["path"] = json!(nodes);
            reason["steps"] = json!(ordered);
            reason["execution_flow"] = json!({"kind":"potential_static","observed":false,"order":"caller_to_callee","citations_complete":complete,"feasibility_proven":false});
        }
        if reasons.iter().any(|r| r.get("execution_flow").is_some()) {
            // Internal candidates, removed before code is costed. Enriched
            // citations enter the packet only together with a fitted path.
            entry["_execution_flow_reasons"] = json!(reasons);
        }
    }
}

/// Compose already-selected invocation segments. Steps reference cited reasons
/// instead of retransmitting their source/evidence. This runs after code packing
/// and is independent of the prior receipt, so deltas retain the same full view.
pub(super) fn pack(
    report: &mut Value,
    selected: &mut [(&str, Value)],
    annotations: &BTreeMap<String, Value>,
    allowance: usize,
) -> usize {
    let mut segments = Vec::new();
    let mut omitted = 0;
    let mut upgrades = BTreeMap::new();
    for (ei, (collection, entry)) in selected.iter().enumerate() {
        if *collection != "entries" {
            continue;
        }
        let reasons = annotations.get(s(entry, "id")).unwrap_or(&entry["reasons"]);
        for (ri, reason) in reasons.as_array().into_iter().flatten().enumerate() {
            if reason.get("execution_flow").is_none() {
                continue;
            }
            if segments.len() >= 128 {
                omitted += 1;
                continue;
            }
            let extra = if reason == &entry["reasons"][ri] {
                0
            } else {
                text::count(&reason.to_string())
                    .saturating_sub(text::count(&entry["reasons"][ri].to_string()))
                    + 16
            };
            upgrades.insert((s(entry, "id").to_owned(), ri), (ei, reason.clone(), extra));
            let steps: Vec<_> = reason["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(si, _)| json!({"entry_id":entry["id"],"reason_index":ri,"step_index":si}))
                .collect();
            segments.push(json!({"kind":"potential_static","observed":false,"side":entry["side"],"node_ids":reason["path"],"steps":steps}));
        }
    }
    let max_depth = report["coverage"]["max_depth"].as_u64().unwrap_or(8) as usize;
    let mut candidates = segments.clone();
    let mut joined_paths: BTreeSet<_> = segments
        .iter()
        .map(|v| json!([v["side"], v["node_ids"]]).to_string())
        .collect();
    let mut cursor = 0;
    while cursor < candidates.len() && candidates.len() < 512 {
        let a = candidates[cursor].clone();
        cursor += 1;
        for b in &segments {
            if candidates.len() >= 512 {
                omitted += 1;
                break;
            }
            let x = a["node_ids"].as_array().unwrap();
            let y = b["node_ids"].as_array().unwrap();
            if a["side"] != b["side"]
                || x.last() != y.first()
                || x.iter().any(|id| y.iter().skip(1).any(|other| other == id))
            {
                continue;
            }
            if x.len() + y.len() - 2 > max_depth {
                omitted += 1;
                continue;
            }
            let mut joined = a.clone();
            joined["node_ids"] = json!(x.iter().chain(y.iter().skip(1)).collect::<Vec<_>>());
            joined["steps"] = json!(
                a["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .chain(b["steps"].as_array().unwrap())
                    .collect::<Vec<_>>()
            );
            if joined_paths.insert(json!([joined["side"], joined["node_ids"]]).to_string()) {
                candidates.push(joined);
            }
        }
    }
    if cursor < candidates.len() {
        omitted += candidates.len() - cursor;
    }
    candidates.sort_by_key(|v| {
        (
            v["side"] != "head",
            std::cmp::Reverse(v["node_ids"].as_array().unwrap().len()),
            v.to_string(),
        )
    });
    let mut kept = Vec::<Value>::new();
    // The empty wrapper is already in the packer's metadata cost. Only charge
    // complete paths and the newly referenced citation upgrades, once each.
    let mut used = 0;
    let mut applied = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for v in candidates {
        if !seen.insert(json!([v["side"], v["node_ids"]]).to_string()) {
            continue;
        }
        let references: BTreeSet<_> = v["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| {
                (
                    s(step, "entry_id").to_owned(),
                    step["reason_index"].as_u64().unwrap() as usize,
                )
            })
            .collect();
        let cost = text::count(&v.to_string())
            + 8
            + references
                .iter()
                .filter(|key| !applied.contains(*key))
                .map(|key| upgrades[key].2)
                .sum::<usize>();
        if kept.len() >= 16 || used + cost > allowance {
            omitted += 1;
            continue;
        }
        used += cost;
        applied.extend(references);
        kept.push(v);
    }
    let mut changed = BTreeSet::new();
    for key in applied {
        let (ei, reason, extra) = &upgrades[&key];
        if *extra > 0 {
            selected[*ei].1["reasons"][key.1] = reason.clone();
            changed.insert(*ei);
        }
    }
    for ei in changed {
        let entry = &mut selected[ei].1;
        entry.as_object_mut().unwrap().remove("revision");
        entry["revision"] = json!(crate::digest(entry));
    }
    // Always expose the limits; no-path does not establish no impact.
    let reserved = report["execution_flows"]["coverage"]["reserved_tokens"]
        .as_u64()
        .unwrap_or(0);
    report["execution_flows"] = json!({"paths":kept,"coverage":{"complete":false,"observed_execution_available":false,"selection":"selected_invocation_segments","omitted":omitted,"max_paths":16,"reserved_tokens":reserved,"used_tokens":used}});
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: &str, side: &str, from: &str, to: &str) -> (&'static str, Value) {
        (
            "entries",
            json!({"id":id,"side":side,"reasons":[{"execution_flow":{"kind":"potential_static"},"path":[from,to],"steps":[{"caller":from,"callee":to}]}]}),
        )
    }

    #[test]
    fn composition_connects_multiple_segments_without_crossing_sides_or_cycles() {
        let mut selected = vec![
            segment("one", "head", "a", "b"),
            segment("two", "head", "b", "c"),
            segment("three", "head", "c", "d"),
            segment("old", "base", "d", "e"),
            segment("cycle", "head", "d", "a"),
        ];
        let mut report = json!({"coverage":{"max_depth":3}});
        pack(&mut report, &mut selected, &BTreeMap::new(), 10000);
        let paths = report["execution_flows"]["paths"].as_array().unwrap();
        assert!(
            paths
                .iter()
                .any(|p| p["node_ids"] == json!(["a", "b", "c", "d"]))
        );
        for path in paths {
            let ids = path["node_ids"].as_array().unwrap();
            assert!(ids.len() <= 4);
            assert_eq!(
                ids.iter()
                    .map(Value::to_string)
                    .collect::<BTreeSet<_>>()
                    .len(),
                ids.len()
            );
            for step in path["steps"].as_array().unwrap() {
                let entry = &selected
                    .iter()
                    .find(|(_, e)| e["id"] == step["entry_id"])
                    .unwrap()
                    .1;
                assert_eq!(entry["side"], path["side"]);
            }
        }
        assert_eq!(report["execution_flows"]["coverage"]["complete"], false);
    }

    #[test]
    fn flow_budget_omissions_are_explicit_and_do_not_replace_code() {
        let mut selected = vec![segment("one", "head", "a", "b")];
        let mut report = json!({"entries":[{"id":"required","text":"required code"}],"coverage":{"max_depth":8}});
        pack(&mut report, &mut selected, &BTreeMap::new(), 0);
        assert_eq!(report["execution_flows"]["paths"], json!([]));
        assert_eq!(report["execution_flows"]["coverage"]["omitted"], 1);
        assert_eq!(report["entries"][0]["text"], "required code");
    }

    #[test]
    fn a_path_and_its_citation_upgrades_are_admitted_atomically() {
        let mut selected = vec![segment("one", "head", "a", "b")];
        let mut reasons = selected[0].1["reasons"].clone();
        reasons[0]["steps"][0]["citation"] = json!({"repository_id":"repo","commit":"b".repeat(40),"snapshot":"c".repeat(64),"file":"main.go","sha256":"d".repeat(64),"byte_start":1,"byte_end":5});
        selected[0].1["reasons"][0]
            .as_object_mut()
            .unwrap()
            .remove("execution_flow");
        selected[0].1["revision"] = json!("original");
        let original = selected.clone();
        let annotations = BTreeMap::from([("one".to_owned(), reasons.clone())]);
        let mut report = json!({"coverage":{"max_depth":8}});
        assert_eq!(pack(&mut report, &mut selected, &annotations, 100), 0);
        assert_eq!(selected, original);
        assert_eq!(report["execution_flows"]["paths"], json!([]));
        assert!(pack(&mut report, &mut selected, &annotations, 2000) > 100);
        assert_eq!(selected[0].1["reasons"], reasons);
        assert_ne!(selected[0].1["revision"], "original");
        assert_eq!(
            report["execution_flows"]["paths"].as_array().unwrap().len(),
            1
        );
    }
}
