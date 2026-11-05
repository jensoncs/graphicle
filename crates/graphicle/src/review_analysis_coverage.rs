//! Analysis-only coverage for an explicit, narrower model. This never grants
//! review reuse, changes the v2 impact verdict, or proves runtime safety.
use super::*;

pub(super) const FORMAT: &str = "graphicle.changed-go-coverage.v1";

pub(super) fn assess(report: &Value, components: &components::Components) -> Value {
    let mut changed = BTreeSet::new();
    let mut unmapped = 0usize;
    for file in report["file_changes"].as_array().into_iter().flatten() {
        if matches!(
            s(file, "classification"),
            "excluded_by_profile" | "non_compiler_input"
        ) {
            continue;
        }
        if let Some(owners) = components.owned_files.get(s(file, "file")) {
            changed.extend(owners.iter().copied());
        } else {
            // Includes changed non-Go assets, embedded inputs, build manifests
            // and dependency implementations. None can certify this Go scope.
            unmapped += 1;
        }
    }
    let mut blocking_component = 0usize;
    let mut blocking_global = 0usize;
    let mut outside_component = 0usize;
    let mut outside_scope = 0usize;
    for issue in report["coverage"]["issues"]
        .as_array()
        .into_iter()
        .flatten()
    {
        match s(issue, "reason") {
            // The contract explicitly stops at resolved dependency APIs and Go
            // semantics. Changed inputs outside that model still block above.
            "dependency_effects_unmodeled"
            | "unsupported_source_language_or_contract"
            | "unmodeled_binary_input"
            | "basic_analysis_limitations"
            | "basic_unresolved_import"
            | "python_environment_incomplete" => outside_scope += 1,
            "unresolved_edge" => {
                let detail = &issue["detail"];
                match components.files.get(&(
                    s(detail, "consumer_build").to_owned(),
                    s(detail, "file").to_owned(),
                )) {
                    Some(owners) if !owners.is_empty() => {
                        if owners.is_disjoint(&changed) {
                            outside_component += 1;
                        } else {
                            blocking_component += 1;
                        }
                    }
                    None | Some(_) => blocking_global += 1,
                }
            }
            // Admission, compiler failure, incomplete discovery, traversal
            // limits, initialization and unknown future reasons fail closed.
            // A failed graph cannot prove that a package is disconnected.
            _ => blocking_global += 1,
        }
    }
    let binding_changed = report["dependency_binding_changed"] == true;
    let status = if unmapped > 0 || binding_changed || blocking_component > 0 || blocking_global > 0
    {
        "incomplete"
    } else if changed.is_empty() {
        "not_applicable"
    } else {
        "complete"
    };
    let fingerprints: Vec<_> = changed.iter().map(|i| &components.hashes[*i]).collect();
    json!({
        "format": FORMAT,
        "status": status,
        "scope": "owned_go_import_components_and_dependency_apis",
        "scope_sha256": crate::digest(&json!([FORMAT,report["base"],report["head"],report["profile"],fingerprints])),
        "changed_components": changed.len(),
        "blocking_component_issues": blocking_component,
        "blocking_global_issues": blocking_global,
        "outside_component_issues": outside_component,
        "outside_scope_issues": outside_scope,
        "unmapped_changed_files": unmapped,
        "binding_changed": binding_changed,
        "exclusions": ["dependency_implementations_and_callbacks", "non_go_semantics", "runtime_behavior"],
        "review_skipping_permitted": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn components() -> components::Components {
        components::Components {
            component: BTreeMap::new(),
            files: BTreeMap::from([
                (("app".into(), "app/main.go".into()), BTreeSet::from([0])),
                (
                    ("app".into(), "app/other/other.go".into()),
                    BTreeSet::from([1]),
                ),
            ]),
            owned_files: BTreeMap::from([("app/main.go".into(), BTreeSet::from([0]))]),
            hashes: vec!["changed".into(), "other".into()],
        }
    }
    fn report(issues: Vec<Value>) -> Value {
        json!({"impact":"incomplete","base":[],"head":[],"profile":{},
            "file_changes":[{"file":"app/main.go","classification":"declaration_change"}],
            "coverage":{"issues":issues,"complete":false}})
    }
    fn unresolved(world: &str, file: &str) -> Value {
        issue(
            "unresolved_edge",
            json!({"consumer_build":world,"file":file,"cause":"interface_dispatch"}),
        )
    }

    #[test]
    fn changed_go_coverage_uses_explicit_boundaries_and_known_components() {
        let input = report(vec![
            unresolved("app", "app/other/other.go"),
            issue("dependency_effects_unmodeled", json!({})),
            issue(
                "unsupported_source_language_or_contract",
                json!({"file":"app/settings.json"}),
            ),
        ]);
        let before = input.clone();
        let result = assess(&input, &components());
        assert_eq!(result["status"], "complete");
        assert_eq!(result["outside_component_issues"], 1);
        assert_eq!(result["outside_scope_issues"], 2);
        assert_eq!(result["review_skipping_permitted"], false);
        assert_eq!(input, before);
        for issue in [
            unresolved("app", "app/main.go"),
            unresolved("other-build", "app/other/other.go"),
            issue("type_check_failed:other", json!("app")),
            issue("source_admission_incomplete", json!("app")),
            issue("future_unknown_limit", json!({})),
        ] {
            assert_eq!(
                assess(&report(vec![issue]), &components())["status"],
                "incomplete"
            );
        }
    }

    #[test]
    fn changed_go_coverage_never_promotes_unmapped_or_rebound_inputs() {
        let mut r = report(vec![]);
        for file in [
            "app/settings.json",
            "app/go.mod",
            "app/vendor/lib/lib.go",
            "app/asset.bin",
        ] {
            r["file_changes"] = json!([{"file":"app/main.go"},{"file":file}]);
            let result = assess(&r, &components());
            assert_eq!(result["status"], "incomplete");
            assert_eq!(result["unmapped_changed_files"], 1);
        }
        r["file_changes"] = json!([]);
        assert_eq!(assess(&r, &components())["status"], "not_applicable");
        r["dependency_binding_changed"] = json!(true);
        assert_eq!(assess(&r, &components())["status"], "incomplete");
    }

    #[test]
    fn changed_go_coverage_schema_rejects_contradictory_claims() {
        let schema: Value = serde_json::from_str(RISK_SCHEMA_JSON).unwrap();
        let validator = jsonschema::validator_for(&schema["$defs"]["analysis_coverage"]).unwrap();
        let coverage = assess(&report(vec![]), &components());
        assert!(validator.is_valid(&coverage));
        for (field, value) in [
            ("changed_components", json!(0)),
            ("blocking_component_issues", json!(1)),
            ("blocking_global_issues", json!(1)),
            ("unmapped_changed_files", json!(1)),
            ("binding_changed", json!(true)),
            ("review_skipping_permitted", json!(true)),
        ] {
            let mut invalid = coverage.clone();
            invalid[field] = value;
            assert!(!validator.is_valid(&invalid), "accepted {field}");
        }
        let mut invalid = coverage.clone();
        invalid["status"] = json!("not_applicable");
        assert!(!validator.is_valid(&invalid));
        let mut empty = report(vec![]);
        empty["file_changes"] = json!([]);
        assert!(validator.is_valid(&assess(&empty, &components())));
        assert!(validator.is_valid(&assess(
            &report(vec![unresolved("app", "app/main.go")]),
            &components()
        )));
    }
}
