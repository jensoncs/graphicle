//! Explain analysis failures without confusing reference counts with packages.
use super::{Analysis, s};
use graphicle_memory::text;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Counts every issue before the public example list is truncated. Native Go
/// reasons suffix their package identity after ':', which is not a new cause.
pub(super) fn histogram(issues: &[Value]) -> Value {
    let mut counts = BTreeMap::new();
    for issue in issues {
        let reason = s(issue, "reason");
        let reason = reason.split_once(':').map_or(reason, |(reason, _)| reason);
        *counts
            .entry((reason, s(&issue["detail"], "cause")))
            .or_insert(0usize) += 1;
    }
    let mut groups = vec![];
    let mut other_count = 0;
    let mut other_groups = 0;
    for ((reason, cause), count) in counts {
        if groups.len() == 128 {
            other_count += count;
            other_groups += 1;
            continue;
        }
        let mut group = json!({"reason":reason,"count":count});
        if !cause.is_empty() {
            group["cause"] = json!(cause);
        }
        groups.push(group);
    }
    json!({"counted":issues.len(),"groups":groups,"other_count":other_count,"other_groups":other_groups})
}

pub(super) fn summary(analysis: &Analysis, example_tokens: usize) -> Value {
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
    json!({
        "go":{
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = histogram(&issues);
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
        let ordinary = histogram(&issues[..1002]);
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
