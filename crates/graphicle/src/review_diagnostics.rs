//! Explain analysis failures without confusing reference counts with packages.
use super::{Analysis, s};
use graphicle_memory::text;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

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
