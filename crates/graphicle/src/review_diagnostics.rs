//! Explain analysis failures without confusing reference counts with packages.
use super::{Analysis, s};
use graphicle_memory::text;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn file_extension(path: &str) -> String {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    // Shebangs can make arbitrary suffixes unsupported. Keep labels bounded.
    if extension.len() > 32 {
        "<overlong>".into()
    } else {
        extension.to_ascii_lowercase()
    }
}

/// Counts every issue before the public example list is truncated. Native Go
/// reasons suffix their package identity after ':', which is not a new cause.
pub(super) fn histogram(issues: &[Value], example_tokens: usize) -> Value {
    let mut counts = BTreeMap::new();
    let mut unsupported_count = 0;
    for issue in issues {
        let reason = s(issue, "reason");
        let reason = reason.split_once(':').map_or(reason, |(reason, _)| reason);
        let detail = &issue["detail"];
        let unsupported = reason == "unsupported_source_language_or_contract";
        let dimensions = if unsupported {
            Some((s(detail, "extension"), s(detail, "side")))
        } else if reason == "basic_unresolved_import" {
            Some((s(detail, "language"), ""))
        } else {
            None
        };
        let entry = counts
            .entry((reason, s(detail, "cause"), dimensions))
            .or_insert((0usize, None::<&Value>));
        entry.0 += 1;
        if unsupported {
            unsupported_count += 1;
            if entry.1.is_none() && !s(detail, "file").is_empty() {
                entry.1 = Some(detail);
            }
        }
    }
    let mut groups = vec![];
    let mut candidates = vec![];
    let mut other_count = 0;
    let mut other_groups = 0;
    for ((reason, cause, dimensions), (count, example)) in counts {
        if groups.len() == 128 {
            other_count += count;
            other_groups += 1;
            continue;
        }
        let mut group = json!({"reason":reason,"count":count});
        if !cause.is_empty() {
            group["cause"] = json!(cause);
        }
        if let Some((extension, side)) = dimensions {
            if reason == "basic_unresolved_import" {
                group["language"] = json!(extension);
            } else {
                group["extension"] = json!(extension);
                group["side"] = json!(side);
            }
            if let Some(example) = example {
                candidates.push((
                    count,
                    json!({"file":example["file"],"side":side,"extension":extension,"cause":cause}),
                ));
            }
        }
        groups.push(group);
    }
    let mut result = json!({"counted":issues.len(),"groups":groups,"other_count":other_count,"other_groups":other_groups});
    if unsupported_count > 0 {
        // One representative per group; prefer prevalent types and avoid using
        // the limited sample twice for the same path on base and head.
        candidates.sort_by_key(|(count, example)| (std::cmp::Reverse(*count), example.to_string()));
        let mut seen = BTreeSet::new();
        let mut examples = vec![];
        let mut used = 0;
        for (_, example) in candidates {
            let cost = text::count(&example.to_string()) + 1;
            if examples.len() == 8 || used + cost > example_tokens {
                continue;
            }
            let key = (
                s(&example, "file").to_owned(),
                s(&example, "extension").to_owned(),
                s(&example, "cause").to_owned(),
            );
            if !seen.insert(key) {
                continue;
            }
            used += cost;
            examples.push(example);
        }
        result["unsupported_count"] = json!(unsupported_count);
        result["unsupported_examples_omitted"] = json!(unsupported_count - examples.len());
        result["unsupported_examples"] = json!(examples);
    }
    result
}

pub(super) fn summary(analysis: &Analysis, example_tokens: usize) -> Value {
    let mut import_files = BTreeSet::new();
    let mut import_causes = BTreeMap::<&str, usize>::new();
    let mut import_modules = BTreeMap::new();
    for issue in &analysis.issues {
        let detail = &issue["detail"];
        if issue["reason"] != "basic_unresolved_import" || detail["language"] != "python" {
            continue;
        }
        let input = &detail["input"];
        import_files.insert((s(detail, "directory"), s(input, "source")));
        *import_causes.entry(s(input, "reason")).or_default() += 1;
        *import_modules
            .entry((s(input, "module"), s(input, "reason")))
            .or_insert(0usize) += 1;
    }
    let mut modules: Vec<_> = import_modules
        .into_iter()
        .map(|((module, cause), count)| json!({"module":module,"cause":cause,"count":count}))
        .collect();
    modules.sort_by_key(|m| {
        (
            std::cmp::Reverse(m["count"].as_u64().unwrap()),
            m.to_string(),
        )
    });
    let packages = |prefix: &str| {
        analysis
            .issues
            .iter()
            .filter_map(|issue| {
                s(issue, "reason")
                    .strip_prefix(prefix)
                    .map(|package| (issue["detail"].as_str().unwrap_or(""), package))
            })
            .collect::<BTreeSet<_>>()
            .len()
    };
    let mut python_files = BTreeSet::new();
    let mut python_causes = BTreeMap::<String, usize>::new();
    let mut python_examples = vec![];
    for issue in &analysis.issues {
        if issue["reason"] != "python_environment_incomplete" {
            continue;
        }
        let detail = &issue["detail"];
        if !python_files.insert(s(detail, "file")) {
            continue;
        }
        let mut causes: BTreeSet<_> = detail["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if detail["local_os_module"] == true {
            causes.insert("local_os_module");
        }
        if detail["issues"].is_null() {
            causes.insert("environment_analysis_unavailable");
        }
        for cause in &causes {
            *python_causes.entry((*cause).to_owned()).or_default() += 1;
        }
        python_examples.push(json!({"file":detail["file"],"causes":causes}));
    }
    // Bound examples by tokens as well as count. Counts always use the full
    // analysis; omitted examples never alter completeness or reuse decisions.
    let sample = |values: &[Value], budget: usize| {
        let mut examples = vec![];
        let mut used = 0;
        let mut groups = BTreeMap::<&str, Vec<&Value>>::new();
        for value in values {
            groups.entry(s(value, "cause")).or_default().push(value);
        }
        for index in 0..8 {
            for values in groups.values() {
                let Some(value) = values.get(index) else {
                    continue;
                };
                let cost = text::count(&value.to_string()) + 1;
                if examples.len() == 8 || used + cost > budget {
                    continue;
                }
                used += cost;
                examples.push((*value).clone());
            }
        }
        examples
    };
    let import_budget = if modules.is_empty() {
        0
    } else {
        example_tokens / 4
    };
    let module_examples = sample(&modules, import_budget);
    let example_tokens = example_tokens - import_budget;
    let boundaries: Vec<_> = analysis
        .calls
        .iter()
        .filter(|(_, call)| !call.boundary.is_null())
        .map(|(world, call)| {
            json!({"consumer_build":world,"file":call.file,"line":call.line,
            "relation":call.relation,"target":call.boundary})
        })
        .collect();
    let boundary_budget = if boundaries.is_empty() {
        0
    } else {
        example_tokens / 4
    };
    let boundary_examples = sample(&boundaries, boundary_budget);
    let example_tokens = example_tokens - boundary_budget;
    let go_examples = sample(
        &analysis.diagnostics,
        if python_files.is_empty() {
            example_tokens
        } else {
            example_tokens / 2
        },
    );
    let python_examples = sample(
        &python_examples,
        if analysis.diagnostics.is_empty() {
            example_tokens
        } else {
            example_tokens / 2
        },
    );
    let mut result = json!({
        "go":{
            "owned_package_instances":analysis.packages.iter().filter(|p| p["analysis_role"] == "owned").count(),
            "dependency_package_instances":analysis.packages.iter().filter(|p| p["analysis_role"] == "dependency").count(),
            "resolved_dependency_references":boundaries.len(),
            "dependency_boundary_examples_omitted":boundaries.len().saturating_sub(boundary_examples.len()),
            "dependency_boundary_examples":boundary_examples,
            "incomplete_package_instances":packages("incomplete_package:"),
            "discovery_incomplete_package_instances":packages("incomplete_discovery:"),
            "type_check_failed_package_instances":packages("type_check_failed:"),
            "unavailable_external_package_instances":packages("unavailable_external_package:"),
            "incomplete_package_references":analysis.issues.iter().filter(|i|i["reason"]=="unresolved_edge" && i["detail"]["cause"]=="incomplete_package").count(),
            "diagnostic_count":analysis.diagnostic_count,
            "diagnostic_examples_omitted":analysis.diagnostic_count.saturating_sub(go_examples.len()),
            "examples":go_examples
        },
        "python_environment":{
            "incomplete_files":python_files.len(),"files_by_cause":python_causes,
            "file_examples_omitted":python_files.len().saturating_sub(python_examples.len()),
            "examples":python_examples
        }
    });
    if !import_files.is_empty() {
        result["python_imports"] = json!({
            "unresolved_occurrences":import_causes.values().sum::<usize>(),
            "files":import_files.len(),"by_cause":import_causes,
            "module_groups":modules.len(),"module_groups_omitted":modules.len()-module_examples.len(),
            "modules":module_examples
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsupported(file: &str, side: &str, cause: &str) -> Value {
        json!({"reason":"unsupported_source_language_or_contract","detail":{
            "file":file,"side":side,"extension":file_extension(file),"cause":cause
        }})
    }

    #[test]
    fn unsupported_counts_are_full_and_independent_of_example_budget() {
        let mut issues = vec![];
        for side in ["base", "head"] {
            for i in 0..24 {
                issues.push(unsupported(&format!("db/{i}.SQL"), side, "code_extension"));
            }
            issues.push(unsupported("api.proto", side, "contract_extension"));
            issues.push(unsupported("Dockerfile", side, "build_filename"));
        }
        let full = histogram(&issues, 10000);
        let empty = histogram(&issues, 0);
        assert_eq!(full["groups"], empty["groups"]);
        assert_eq!(full["counted"], 52);
        assert_eq!(full["unsupported_count"], 52);
        assert_eq!(empty["unsupported_examples"], json!([]));
        assert_eq!(empty["unsupported_examples_omitted"], 52);
        let groups = full["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 6);
        for side in ["base", "head"] {
            assert!(
                groups.contains(&json!({"reason":"unsupported_source_language_or_contract",
                "cause":"code_extension","extension":"sql","side":side,"count":24}))
            );
        }
        // One representative per type, deduplicated across sides.
        assert_eq!(full["unsupported_examples"].as_array().unwrap().len(), 3);
        assert_eq!(full["unsupported_examples"][0]["extension"], "sql");
        assert_eq!(full["unsupported_examples_omitted"], 49);
    }

    #[test]
    fn unsupported_samples_and_group_overflow_are_bounded_and_accounted() {
        let mut issues: Vec<_> = (0..140)
            .map(|i| unsupported(&format!("script.ext{i:03}"), "head", "shebang"))
            .collect();
        issues.push(unsupported(
            &format!("{}.sql", "a/".repeat(1000)),
            "head",
            "code_extension",
        ));
        let result = histogram(&issues, 1000);
        let groups = result["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 128);
        assert_eq!(result["other_groups"], 13);
        assert_eq!(result["other_count"], 13);
        assert_eq!(
            groups
                .iter()
                .map(|g| g["count"].as_u64().unwrap())
                .sum::<u64>()
                + 13,
            141
        );
        assert_eq!(result["unsupported_count"], 141);
        let examples = result["unsupported_examples"].as_array().unwrap();
        assert_eq!(examples.len(), 8);
        assert_eq!(result["unsupported_examples_omitted"], 133);
        assert!(
            examples
                .iter()
                .map(|e| text::count(&e.to_string()) + 1)
                .sum::<usize>()
                <= 1000
        );
        assert!(examples.iter().all(|e| s(e, "file").len() < 100));
        assert_eq!(file_extension("schema.SQL"), "sql");
        assert_eq!(file_extension("Dockerfile"), "");
        assert_eq!(
            file_extension(&format!("file.{}", "x".repeat(33))),
            "<overlong>"
        );
    }

    #[test]
    fn histogram_counts_all_causes_and_bounds_unknown_groups() {
        let mut issues = vec![
            json!({"reason":"incomplete_package:one"}),
            json!({"reason":"incomplete_package:two"}),
        ];
        issues.extend(
            (0..1000).map(
                |_| json!({"reason":"unresolved_edge","detail":{"cause":"incomplete_package"}}),
            ),
        );
        issues.extend((0..200).map(|i| json!({"reason":format!("other_{i}")})));
        let result = histogram(&issues, 0);
        assert_eq!(result["counted"], 1202);
        let groups = result["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 128);
        assert_eq!(
            groups
                .iter()
                .find(|g| g["reason"] == "incomplete_package")
                .unwrap()["count"],
            2
        );
        assert_eq!(
            groups
                .iter()
                .map(|g| g["count"].as_u64().unwrap())
                .sum::<u64>()
                + result["other_count"].as_u64().unwrap(),
            1202
        );
        let ordinary = histogram(&issues[..1002], 0);
        assert_eq!(
            ordinary["groups"][1],
            json!({"reason":"unresolved_edge","cause":"incomplete_package","count":1000})
        );
    }

    #[test]
    fn python_counts_files_and_causes_before_example_trimming() {
        let mut analysis = Analysis {
            diagnostics: vec![],
            diagnostic_count: 0,
            symbols: BTreeMap::new(),
            calls: vec![],
            packages: vec![],
            issues: vec![],
            analyzer: String::new(),
            toolchain: String::new(),
            inputs: vec![],
            raw: None,
            services: super::super::services::Links::default(),
        };
        let first = json!({"reason":"python_environment_incomplete","detail":{"file":"app/a.py","issues":["python_environment_binding_ambiguous","python_environment_binding_ambiguous"],"local_os_module":true}});
        analysis.issues = vec![
            first.clone(),
            first,
            json!({"reason":"python_environment_incomplete","detail":{"file":"app/b.py","issues":null,"local_os_module":false}}),
        ];
        let report = summary(&analysis, 0);
        assert_eq!(report["python_environment"]["incomplete_files"], 2);
        assert_eq!(
            report["python_environment"]["files_by_cause"]["python_environment_binding_ambiguous"],
            1
        );
        assert_eq!(
            report["python_environment"]["files_by_cause"]["local_os_module"],
            1
        );
        assert_eq!(
            report["python_environment"]["files_by_cause"]["environment_analysis_unavailable"],
            1
        );
        assert_eq!(report["python_environment"]["file_examples_omitted"], 2);
        assert_eq!(report["python_environment"]["examples"], json!([]));
    }
}
