//! Basic, deliberately file-granular impact over admitted bytes. Dependencies
//! are potential relationships, never compiler-resolved calls or deployment facts.
//! No project code, package manager, Terraform, Helm or shell command is run.
use super::*;
use graphicle_memory::structure;

const VERSION: &str = "basic-source-impact-v1";

fn ancestor_file(input: &Input, path: &str, name: &str) -> Option<String> {
    let mut dir = Path::new(path).parent()?;
    // A pin directory is the trust boundary: never search a sibling repository.
    loop {
        let candidate = dir.join(name).to_str()?.to_owned();
        if input.sources.contains_key(&candidate) {
            return Some(candidate);
        }
        if dir.components().count() <= 1 {
            return None;
        }
        dir = dir.parent()?;
    }
}

pub(super) fn language(input: &Input, path: &str) -> Option<&'static str> {
    let source = input.sources.get(path)?;
    let content = source.content.text()?;
    let name = Path::new(path).file_name()?.to_str()?;
    // Files inside a chart may be templates even when they end in .sh or .py.
    if ancestor_file(input, path, "Chart.yaml").is_some()
        || (path.contains("/templates/")
            && matches!(
                Path::new(path).extension().and_then(|e| e.to_str()),
                Some("yaml" | "yml" | "tpl")
            ))
    {
        return Some("helm");
    }
    Some(
        match Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
        {
            "tf" | "tfvars" => "terraform",
            "py" | "pyi" => "python",
            "rs" => "rust",
            "sh" | "bash" => "bash",
            _ if name.ends_with(".tf.json")
                || name.ends_with(".tfvars.json")
                || name == ".terraform.lock.hcl" =>
            {
                "terraform"
            }
            _ if matches!(
                name,
                "Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml"
            ) =>
            {
                "rust"
            }
            _ if matches!(
                name,
                "pyproject.toml"
                    | "setup.py"
                    | "setup.cfg"
                    | "requirements.txt"
                    | "uv.lock"
                    | "poetry.lock"
            ) =>
            {
                "python"
            }
            _ if content.lines().next().is_some_and(|line| {
                line.starts_with("#!")
                    && line
                        .split(['/', ' ', '\t'])
                        .any(|part| matches!(part, "bash" | "sh"))
            }) =>
            {
                "bash"
            }
            _ => return None,
        },
    )
}

fn world(path: &str) -> String {
    format!("basic:{}", path.split('/').next().unwrap_or(""))
}
fn file_key(path: &str) -> String {
    format!("file:{path}")
}

/// Normalize lexically under this repository. Never touch the host filesystem.
fn relative(file: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.contains(['\\', ':', '\0']) {
        return None;
    }
    let mut parts: Vec<_> = file.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => (),
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            ".." => return None,
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn link(
    out: &mut Analysis,
    from: &str,
    to: &str,
    relation: &str,
    evidence: &str,
    line: usize,
    span: std::ops::Range<usize>,
) {
    if from == to {
        return;
    }
    if out.calls.len() >= 200_000 {
        if !out.issues.iter().any(|i| i["reason"] == "basic_edge_limit") {
            out.issues.push(issue("basic_edge_limit", json!(200_000)));
        }
        return;
    }
    out.calls.push((
        world(from),
        Call {
            boundary: Value::Null,
            caller: file_key(from),
            target: file_key(to),
            target_file: to.into(),
            target_package: to.into(),
            target_offset: 0,
            spelling: to.into(),
            file: from.into(),
            start: span.start,
            end: span.end,
            line,
            relation: relation.into(),
            reason: String::new(),
            evidence: evidence.into(),
            discarded_error: false,
        },
    ));
}

pub(super) fn extend(input: &Input, out: &mut Analysis) -> Result<()> {
    let files: BTreeMap<_, _> = input
        .sources
        .keys()
        .filter_map(|p| language(input, p).map(|l| (p.clone(), l)))
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    out.analyzer = format!("{}+{VERSION}", out.analyzer);
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut imports = BTreeMap::<String, serde_json::Map<String, Value>>::new();
    for (path, lang) in &files {
        checkpoint()?;
        *counts.entry(lang).or_default() += 1;
        let source = &input.sources[path];
        let content = source.content.text().unwrap();
        let owner = world(path);
        // File nodes preserve top-level code, decorators, comments, and parse
        // failures. A body-only slice must never hide a relevant input change.
        let symbol = Symbol {
            key: file_key(path),
            file: path.clone(),
            package: path.clone(),
            name: source.path.clone(),
            kind: "basic_file".into(),
            parent: String::new(),
            start: 0,
            end: content.len(),
            body_start: 0,
            line: 1,
            end_line: content.lines().count().max(1),
            revision: source.sha.clone(),
            signature: text::sha256(lang.as_bytes()),
            implementation: source.sha.clone(),
            correspondence: (*lang).into(),
        };
        out.symbols
            .insert(key(&owner, &symbol.key), (owner.clone(), symbol));
        let mut inventory = json!({"file":path,"role":"selected","package":path,"consumer_build":owner,"language":lang,"analysis_level":"basic_file"});
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if matches!(*lang, "python" | "rust" | "bash")
            && (matches!(ext, "py" | "rs" | "sh" | "bash") || *lang == "bash")
        {
            let parse_path = if *lang == "bash" {
                "source.sh"
            } else {
                source.path.as_str()
            };
            let mut parsed = structure::analyze(parse_path, content)
                .unwrap_or_else(|_| structure::skipped("basic parse failed"));
            if *lang == "python" && ext == "py" {
                let shadowed_os = input.sources.keys().any(|other| {
                    other.split('/').next() == path.split('/').next()
                        && (other.ends_with("/os.py") || other.ends_with("/os/__init__.py"))
                });
                let environment = &parsed["environment"];
                let incomplete = environment.is_null()
                    || environment["issues"]
                        .as_array()
                        .is_none_or(|issues| !issues.is_empty())
                    || shadowed_os;
                if incomplete {
                    out.issues.push(issue("python_environment_incomplete",json!({"file":path,"issues":environment["issues"],"local_os_module":shadowed_os})));
                }
                inventory["environment"] = environment.clone();
                for operation in environment["operations"].as_array().into_iter().flatten().cloned()
                    .chain(incomplete.then(||json!({"kind":"read","key":null,"byte_start":0,"byte_end":content.len(),"line":1}))) {
                    out.calls.push((owner.clone(),Call {
                        boundary: Value::Null,
                        caller:file_key(path),target:String::new(),target_file:String::new(),target_package:String::new(),target_offset:0,
                        spelling:if shadowed_os {String::new()}else{operation["key"].as_str().unwrap_or("").into()},
                        file:path.clone(),start:operation["byte_start"].as_u64().unwrap_or(0) as usize,end:operation["byte_end"].as_u64().unwrap_or(0) as usize,
                        line:operation["line"].as_u64().unwrap_or(1) as usize,
                        relation:if operation["kind"]=="write"{"writes_environment"}else{"reads_environment"}.into(),
                        reason:String::new(),evidence:"python_import_binding".into(),discarded_error:false,
                    }));
                }
            }
            let declarations: Vec<_> = parsed["symbols"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| {
                    let start = d["byte_start"].as_u64().unwrap_or(u64::MAX) as usize;
                    let end = d["byte_end"].as_u64().unwrap_or(u64::MAX) as usize;
                    start < end && content.get(start..end).is_some()
                })
                .take(128)
                .cloned()
                .collect();
            inventory["declarations"] = json!(declarations);
            inventory["declarations_omitted"] = json!(
                parsed["symbols"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .saturating_sub(declarations.len())
            );
            if !structure::declaration_spans_complete(&parsed) {
                out.issues.push(issue(
                    "basic_parse_incomplete",
                    json!({"file":path,"issues":parsed["issues"]}),
                ));
                parsed["imports"] = json!([]);
            }
            if matches!(*lang, "python" | "rust") {
                // This tier only consumes imports. Do not repeat unused call
                // resolution in structure::graph for every basic review.
                parsed["calls"] = json!([]);
                imports
                    .entry(path.split('/').next().unwrap().into())
                    .or_default()
                    .insert(source.path.clone(), parsed);
            }
        }
        out.inputs.push(inventory);
        let group = match *lang {
            "helm" => ancestor_file(input, path, "Chart.yaml"),
            "terraform" => Path::new(path)
                .parent()
                .map(|p| format!("terraform:{}", p.display())),
            // A build/packaging configuration can affect every source in its
            // nearest project. This is conservative scope, not a call relation.
            "rust" => ancestor_file(input, path, "Cargo.toml"),
            "python" => ancestor_file(input, path, "pyproject.toml")
                .or_else(|| ancestor_file(input, path, "setup.py"))
                .or_else(|| ancestor_file(input, path, "setup.cfg"))
                .or_else(|| ancestor_file(input, path, "requirements.txt")),
            _ => None,
        };
        if let Some(group) = group {
            groups.entry(group).or_default().push(path.clone());
        } else if *lang == "helm" {
            out.issues
                .push(issue("basic_missing_chart_metadata", json!({"file":path})));
        }
    }
    for (lang, count) in counts {
        out.issues.push(issue("basic_analysis_limitations",json!({"language":lang,"files":count,"granularity":"file",
            "compiler_resolution":false,"dependency_coverage_complete":false,
            "limitations":match lang {
                "terraform"=>"No plan, state, provider schemas, remote module resolution or expression evaluation",
                "helm"=>"Chart source scope only; no rendering, release values, cluster capabilities or remote chart resolution",
                "bash"=>"Literal script paths are candidates; working directory, PATH, expansions and dynamic commands are unresolved",
                "python"=>"Local import candidates and literal environment reads only; runtime environments, dynamic imports, rebinding and runtime dispatch are not resolved",
                _=>"Local module candidates only; Cargo resolution, cfg, macros, build scripts and trait dispatch are not resolved"
            }})));
    }
    for (group, members) in groups {
        let anchor = if files.contains_key(&group) {
            group.as_str()
        } else {
            &members[0]
        };
        let infrastructure = matches!(files[anchor], "helm" | "terraform");
        for member in &members {
            link(
                out,
                member,
                anchor,
                "configuration_scope",
                "shared_source_configuration",
                1,
                0..0,
            );
            // Terraform/Helm inputs form a single conservative module/chart
            // scope. Language project files depend on their manifest only.
            if infrastructure
                || Path::new(member)
                    .file_name()
                    .is_some_and(|n| n == "build.rs")
                || !matches!(
                    Path::new(member).extension().and_then(|e| e.to_str()),
                    Some("py" | "pyi" | "rs")
                )
            {
                link(
                    out,
                    anchor,
                    member,
                    "configuration_scope",
                    "shared_source_configuration",
                    1,
                    0..0,
                );
            }
        }
        if matches!(files[anchor], "helm" | "rust")
            && let Some(parent_dir) = Path::new(anchor).parent().and_then(Path::parent)
        {
            let manifest = if files[anchor] == "helm" {
                "Chart.yaml"
            } else {
                "Cargo.toml"
            };
            let probe = parent_dir.join(manifest).to_string_lossy().into_owned();
            if let Some(parent) = ancestor_file(input, &probe, manifest)
                && files.contains_key(&parent)
            {
                let (from, to, evidence) = if files[anchor] == "helm" {
                    (parent.as_str(), anchor, "vendored_chart_candidate")
                } else {
                    (
                        anchor,
                        parent.as_str(),
                        "ancestor_cargo_configuration_candidate",
                    )
                };
                link(out, from, to, "configuration_scope", evidence, 1, 0..0);
            }
        }
    }
    for (repo, analyses) in imports {
        checkpoint()?;
        if analyses.len() > structure::GRAPH_FILE_LIMIT {
            out.issues.push(issue(
                "basic_import_limit",
                json!({"directory":repo,"files":analyses.len()}),
            ));
            continue;
        }
        // Reuse the existing static import resolver. Its targets remain
        // candidates here: a source layout is not a pinned runtime/build path.
        let graph = structure::graph(&Value::Object(analyses))?;
        for edge in graph["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["kind"] == "imports")
        {
            let from = format!("{repo}/{}", s(edge, "source"));
            let to = format!("{repo}/{}", s(edge, "target"));
            if files.contains_key(&from) && files.contains_key(&to) {
                let line = edge["line"].as_u64().unwrap_or(1) as usize;
                let source = input.sources[&from].content.text().unwrap();
                let start = source
                    .split_inclusive('\n')
                    .take(line.saturating_sub(1))
                    .map(str::len)
                    .sum();
                let end = start
                    + source
                        .get(start..)
                        .unwrap_or("")
                        .split_inclusive('\n')
                        .next()
                        .unwrap_or("")
                        .len();
                link(
                    out,
                    &from,
                    &to,
                    "potential_import",
                    "static_source_candidate",
                    line,
                    start..end,
                );
            }
        }
        for unresolved in graph["unresolved"].as_array().into_iter().flatten() {
            out.issues.push(issue(
                "basic_unresolved_import",
                json!({"directory":repo,"cause":s(unresolved,"reason"),"language":files.get(&format!("{repo}/{}",s(unresolved,"source"))),"input":unresolved}),
            ));
        }
    }
    for (file, lang) in &files {
        checkpoint()?;
        if !matches!(*lang, "bash" | "terraform") {
            continue;
        }
        // A tfvars object's `module` key is a variable value, not a module
        // declaration. Values and lockfiles participate only in module scope.
        if *lang == "terraform" && !file.ends_with(".tf") && !file.ends_with(".tf.json") {
            continue;
        }
        let content = input.sources[file].content.text().unwrap();
        let grammar = if *lang == "terraform" && file.ends_with(".json") {
            "terraform_json"
        } else {
            lang
        };
        let parsed = graphicle::extractors::basic_dependencies::extract(grammar, content)?;
        for reason in parsed["issues"].as_array().into_iter().flatten() {
            out.issues.push(issue(
                "basic_dependency_parse_incomplete",
                json!({"file":file,"cause":reason}),
            ));
        }
        for dependency in parsed["dependencies"].as_array().into_iter().flatten() {
            let literal = dependency["path"].as_str();
            let local = literal
                .filter(|s| *lang == "bash" || s.starts_with("./") || s.starts_with("../"))
                .and_then(|s| relative(file, s));
            let targets: Vec<_> = if *lang == "terraform" {
                local
                    .as_ref()
                    .map(|dir| {
                        files
                            .iter()
                            .filter(|(p, l)| {
                                **l == "terraform"
                                    && Path::new(p).parent().and_then(Path::to_str)
                                        == Some(dir.as_str())
                            })
                            .map(|(p, _)| p.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                local
                    .filter(|p| files.contains_key(p))
                    .into_iter()
                    .collect()
            };
            if targets.is_empty() {
                out.issues.push(issue("basic_unresolved_dependency",json!({"file":file,"line":dependency["line"],"kind":dependency["kind"],"dynamic":dependency["dynamic"],"cause":"dynamic_external_or_missing_pinned_target"})));
            }
            for target in targets {
                link(
                    out,
                    file,
                    &target,
                    "potential_import",
                    if *lang == "bash" {
                        "literal_shell_path_candidate"
                    } else {
                        "literal_local_module"
                    },
                    dependency["line"].as_u64().unwrap_or(1) as usize,
                    dependency["byte_start"].as_u64().unwrap_or(0) as usize
                        ..dependency["byte_end"].as_u64().unwrap_or(0) as usize,
                );
            }
        }
    }
    Ok(())
}

pub(super) fn annotate(entry: &mut Value, analysis: &Analysis, file: &str) {
    if let Some(input) = analysis
        .inputs
        .iter()
        .find(|i| i["file"] == file && i["analysis_level"] == "basic_file")
    {
        for field in [
            "language",
            "analysis_level",
            "declarations",
            "declarations_omitted",
        ] {
            if let Some(value) = input.get(field) {
                entry[field] = value.clone();
            }
        }
        entry["test_classification"] = json!("not_analyzed");
        entry["test"] = Value::Null;
        entry["signature_revision"] = Value::Null;
        entry["implementation_revision"] = Value::Null;
        entry["revision"] = json!(crate::digest(entry));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(files: &[(&str, &str)]) -> Input {
        let sources = files
            .iter()
            .map(|(p, t)| {
                let (repo, path) = p.split_once('/').unwrap();
                (
                    (*p).into(),
                    Source {
                        repository: repo.into(),
                        path: path.into(),
                        sha: text::sha256(t.as_bytes()),
                        content: crate::source_object::Content::Text((*t).into()).into(),
                    },
                )
            })
            .collect();
        Input {
            pins: vec![],
            sources,
            issues: vec![],
            bindings: vec![],
        }
    }
    fn profile() -> Profile {
        Profile {
            goos: "linux".into(),
            goarch: "amd64".into(),
            tags: vec![],
            tests: false,
            workspace: String::new(),
        }
    }
    fn analysis(input: &Input) -> Analysis {
        analyze(
            input,
            &profile(),
            None,
            &mut crate::review_cache::PackagePool::default(),
            "test",
        )
        .unwrap()
    }
    fn report(base: &Input, head: &Input) -> Value {
        let request = Request {
            timeout_ms: None,
            compact_provenance: false,
            repository_id: "repo".into(),
            base: vec![],
            head: vec![],
            profile: profile(),
            max_depth: 8,
            max_tokens: 16000,
            previous_packet_id: None,
            business_context: None,
            reuse_text: false,
            reviewed_entries: vec![],
            provided_diff: vec![],
        };
        compare(base, head, &analysis(base), &analysis(head), &request).unwrap()
    }
    fn selected(report: &Value, side: &str, file: &str) -> bool {
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["side"] == side && e["file"] == file)
    }

    #[test]
    fn python_imports_include_unchanged_consumers_without_claiming_calls() {
        let caller = "from tax import calculate\ncalculate = replacement\ndef run():\n    return calculate()\n";
        let base = input(&[
            ("repo/app.py", caller),
            ("repo/tax.py", "def calculate():\n    return 1\n"),
            ("repo/unrelated.py", "def other(): return 3\n"),
        ]);
        let head = input(&[
            ("repo/app.py", caller),
            ("repo/tax.py", "def calculate():\n    return 2\n"),
            ("repo/unrelated.py", "def other(): return 3\n"),
        ]);
        let a = analysis(&head);
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "repo/app.py" && c.target_file == "repo/tax.py")
        );
        assert!(
            a.calls
                .iter()
                .all(|(_, c)| c.relation != "calls" && c.relation != "may_call")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "app.py"));
        assert!(!selected(&r, "head", "unrelated.py"));
        assert_eq!(r["coverage"]["complete"], false);
        assert!(
            !r["coverage"]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["reason"] == "unsupported_source_language_or_contract")
        );
        let e = r["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["file"] == "tax.py" && e["side"] == "head")
            .unwrap();
        assert_eq!(
            e["text"],
            head.sources["repo/tax.py"].content.text().unwrap()
        );
        assert_eq!(e["language"], "python");
        assert_eq!(e["declarations"][0]["name"], "calculate");
        assert!(e["signature_revision"].is_null());
        assert_eq!(r["changes"][0]["granularity"], "file");
        assert!(r["changes"][0]["signature_changed"].is_null());
    }

    #[test]
    fn imports_never_cross_repository_or_ambiguous_source_roots() {
        let data = input(&[
            ("one/app.py", "import tax\n"),
            ("two/tax.py", "def pay(): pass\n"),
            ("three/app.py", "import tax\n"),
            ("three/src/tax.py", "def pay(): pass\n"),
            ("three/lib/tax.py", "def pay(): pass\n"),
        ]);
        let a = analysis(&data);
        assert!(a.calls.is_empty());
        assert!(
            a.issues
                .iter()
                .any(|i| i["reason"] == "basic_unresolved_import")
        );
        assert!(
            !a.issues
                .iter()
                .any(|i| i["reason"] == "compiler_analysis_unavailable")
        );
    }

    #[test]
    fn rust_modules_and_manifest_changes_keep_source_context() {
        let base = input(&[
            (
                "repo/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            (
                "repo/src/lib.rs",
                "mod tax;\npub fn call() { tax::pay(); }\n",
            ),
            ("repo/src/tax.rs", "pub fn pay() {}\n"),
        ]);
        let mut head = input(&[
            (
                "repo/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.2.0\"\n",
            ),
            (
                "repo/src/lib.rs",
                "mod tax;\npub fn call() { tax::pay(); }\n",
            ),
            ("repo/src/tax.rs", "pub fn pay() {}\n"),
        ]);
        assert!(
            analysis(&base)
                .calls
                .iter()
                .any(|(_, c)| c.file == "repo/src/lib.rs" && c.target_file == "repo/src/tax.rs")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "src/lib.rs"));
        assert!(selected(&r, "head", "src/tax.rs"));
        head.sources.remove("repo/src/tax.rs");
        let deleted = report(&base, &head);
        assert!(selected(&deleted, "base", "src/tax.rs"));
        assert!(selected(&deleted, "head", "src/lib.rs"));
        assert!(
            deleted["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "deleted")
        );
    }

    #[test]
    fn shell_paths_are_candidates_and_dynamic_or_missing_sources_are_visible() {
        let data = input(&[
            (
                "repo/run",
                "#!/bin/bash\nsource ./lib.sh\nsource \"$CONFIG\"\nsource ../outside.sh\n# source fake.sh\necho 'source fake.sh'\n",
            ),
            ("repo/lib.sh", "work() { echo ok; }\n"),
            ("other/outside.sh", "echo other\n"),
        ]);
        let a = analysis(&data);
        assert_eq!(a.calls.len(), 1);
        assert_eq!(a.calls[0].1.target_file, "repo/lib.sh");
        assert_eq!(a.calls[0].1.evidence, "literal_shell_path_candidate");
        assert_eq!(
            a.issues
                .iter()
                .filter(|i| i["reason"] == "basic_unresolved_dependency")
                .count(),
            2
        );
    }

    #[test]
    fn terraform_local_modules_and_same_directory_inputs_form_potential_scope() {
        let root = "module \"child\" { source = \"./child\" }\nmodule \"remote\" { source = \"registry/vendor/mod\" }\n";
        let base = input(&[
            ("repo/main.tf", root),
            ("repo/variables.tf", "variable \"count\" { default = 1 }\n"),
            ("repo/child/main.tf", "output \"answer\" { value = 1 }\n"),
            ("repo/unrelated/main.tf", "output \"other\" { value = 0 }\n"),
        ]);
        let head = input(&[
            ("repo/main.tf", root),
            ("repo/variables.tf", "variable \"count\" { default = 1 }\n"),
            ("repo/child/main.tf", "output \"answer\" { value = 2 }\n"),
            ("repo/unrelated/main.tf", "output \"other\" { value = 0 }\n"),
        ]);
        let a = analysis(&head);
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "repo/main.tf" && c.target_file == "repo/child/main.tf")
        );
        assert!(
            a.issues
                .iter()
                .any(|i| i["reason"] == "basic_unresolved_dependency")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "main.tf"));
        assert!(selected(&r, "head", "variables.tf"));
        assert!(!selected(&r, "head", "unrelated/main.tf"));
    }

    #[test]
    fn helm_values_helpers_and_vendored_chart_remain_source_only() {
        let chart = "apiVersion: v2\nname: app\nversion: 0.1.0\n";
        let template = "kind: Deployment\nreplicas: {{ .Values.replicas }}\n";
        let base = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/values.yaml", "replicas: 1\n"),
            ("repo/app/templates/deploy.yaml", template),
            (
                "repo/app/templates/_helpers.tpl",
                "{{- define \"app.name\" -}}app{{- end -}}",
            ),
            ("repo/other/Chart.yaml", chart),
            ("repo/other/values.yaml", "replicas: 1\n"),
        ]);
        let head = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/values.yaml", "replicas: 2\n"),
            ("repo/app/templates/deploy.yaml", template),
            (
                "repo/app/templates/_helpers.tpl",
                "{{- define \"app.name\" -}}app{{- end -}}",
            ),
            ("repo/other/Chart.yaml", chart),
            ("repo/other/values.yaml", "replicas: 1\n"),
        ]);
        let r = report(&base, &head);
        assert!(selected(&r, "head", "app/templates/deploy.yaml"));
        assert!(selected(&r, "head", "app/templates/_helpers.tpl"));
        assert!(!selected(&r, "head", "other/values.yaml"));
        assert!(!r["entries"].as_array().unwrap().iter().any(|e| {
            e["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.get("execution_flow").is_some())
        }));
        let nested = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/charts/child/Chart.yaml", chart),
        ]);
        assert!(
            analysis(&nested)
                .calls
                .iter()
                .any(|(_, c)| c.file == "repo/app/Chart.yaml"
                    && c.target_file == "repo/app/charts/child/Chart.yaml")
        );
        let missing = input(&[("repo/templates/deploy.yaml", template)]);
        assert!(
            analysis(&missing)
                .issues
                .iter()
                .any(|i| i["reason"] == "basic_missing_chart_metadata")
        );
    }

    #[test]
    fn malformed_source_and_budget_exhaustion_never_claim_complete_impact() {
        let base = input(&[("repo/bad.py", "def broken(:\n pass\n")]);
        let head = input(&[(
            "repo/bad.py",
            &format!("def broken(:\n pass\n#{}", " evidence".repeat(5000)),
        )]);
        assert!(
            analysis(&head)
                .issues
                .iter()
                .any(|i| i["reason"] == "basic_parse_incomplete")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "bad.py"));
        match pack(r, 2000, None, None) {
            Ok((packet, _)) => assert_eq!(packet["coverage"]["complete"], false),
            Err(error) => assert!(error.to_string().contains("budget")),
        }
    }

    #[test]
    fn delta_invalidates_deleted_basic_entries_and_cites_exact_bytes() {
        let base = input(&[("repo/a.py", "def pay(): return 1\n")]);
        let initial = input(&[("repo/a.py", "def pay(): return 2\n")]);
        let next = input(&[("repo/b.py", "def pay(): return 2\n")]);
        let (first, receipt) = pack(report(&base, &initial), 16000, None, None).unwrap();
        let (delta, _) = pack(report(&base, &next), 16000, Some(&receipt), None).unwrap();
        let previous = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head")
            .unwrap();
        assert!(
            delta["invalidated"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == previous["id"])
        );
        let current = delta["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head")
            .unwrap();
        assert_eq!(current["file"], "b.py");
        assert_eq!(
            current["file_sha256"],
            text::sha256(s(current, "text").as_bytes())
        );
    }

    #[test]
    fn workspace_and_requirements_changes_retain_potential_consumers() {
        let before = input(&[
            ("rust/Cargo.toml", "[workspace]\nmembers = [\"app\"]\n"),
            (
                "rust/app/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/app/src/lib.rs", "pub fn run() {}\n"),
            ("python/requirements.txt", "example==1\n"),
            ("python/src/app.py", "def run(): pass\n"),
        ]);
        let after = input(&[
            (
                "rust/Cargo.toml",
                "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
            ),
            (
                "rust/app/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/app/src/lib.rs", "pub fn run() {}\n"),
            ("python/requirements.txt", "example==2\n"),
            ("python/src/app.py", "def run(): pass\n"),
        ]);
        let r = report(&before, &after);
        assert!(selected(&r, "head", "app/src/lib.rs"));
        assert!(selected(&r, "head", "src/app.py"));
    }

    #[test]
    fn terraform_json_and_build_script_dependencies_are_conservative() {
        let data = input(&[
            (
                "repo/main.tf.json",
                r#"{"module":{"child":{"source":"./child"}}}"#,
            ),
            (
                "repo/terraform.tfvars.json",
                r#"{"module":{"child":{"source":"./child"}}}"#,
            ),
            ("repo/child/main.tf", "output \"answer\" { value = 2 }\n"),
            (
                "rust/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/build.rs", "fn main() {}\n"),
            ("rust/src/lib.rs", "pub fn run() {}\n"),
        ]);
        let a = analysis(&data);
        assert!(
            a.calls.iter().any(
                |(_, c)| c.file == "repo/main.tf.json" && c.target_file == "repo/child/main.tf"
            )
        );
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "rust/Cargo.toml" && c.target_file == "rust/build.rs")
        );
        let invalid =
            graphicle::extractors::basic_dependencies::extract("bash", "source '\n").unwrap();
        assert!(!a.calls.iter().any(
            |(_, c)| c.file == "repo/terraform.tfvars.json" && c.relation == "potential_import"
        ));
        assert!(invalid["dependencies"].as_array().unwrap().is_empty());
        assert_eq!(invalid["issues"][0], "syntax_errors");
    }
}
