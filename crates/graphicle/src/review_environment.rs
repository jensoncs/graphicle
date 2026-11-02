//! Environment inputs share the pinned service graph and its provenance rules.
//! Explicit scopes assert ownership; automatic associations are source candidates.
//! Neither proves runtime deployment. Narrowing
//! covers only replacements of literal values with otherwise identical YAML.
use super::*;
use graphicle::extractors::environment as extract;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Deployment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
    file: String,
    container: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    id: String,
    service: Option<services::Service>,
    #[serde(skip)]
    automatic: bool,
    module: String,
    deployment: Deployment,
}
#[derive(Default)]
pub(super) struct State {
    files: BTreeMap<String, (String, String)>,
    reports: Vec<Value>,
}
impl State {
    pub fn active(&self) -> bool {
        !self.reports.is_empty()
    }
    pub fn summary(&self) -> Value {
        json!({"enabled":true,"scopes":self.reports,"narrowable_files":self.files.len(),
            "model":"literal_go_python_reads_and_kubernetes_values_v2","runtime_observed":false})
    }
}
pub(super) fn covers_change(file: &str, a: &Analysis, b: &Analysis) -> bool {
    a.services
        .environment
        .files
        .get(file)
        .is_some_and(|old| b.services.environment.files.get(file) == Some(old))
}

/// Default discovery stays within one repository. An existing service manifest
/// is authoritative, including an invalid manifest or an explicit empty scope list.
fn discover(input: &Input, a: &Analysis, links: &mut services::Links) -> Vec<(Pin, Scope, bool)> {
    let mut scopes = vec![];
    for pin in &input.pins {
        if input
            .sources
            .contains_key(&format!("{}/{}", pin.directory, services::MANIFEST))
        {
            continue;
        }
        let prefix = format!("{}/", pin.directory);
        if !a.calls.iter().any(|(_, c)| {
            c.file.starts_with(&prefix)
                && matches!(
                    c.relation.as_str(),
                    "reads_environment" | "writes_environment"
                )
        }) {
            continue;
        }
        let mut candidates = vec![];
        let mut incomplete = false;
        for (path, source) in input.sources.iter().filter(|(path, _)| {
            path.starts_with(&prefix)
                && matches!(
                    Path::new(path).extension().and_then(|s| s.to_str()),
                    Some("yaml" | "yml" | "json")
                )
        }) {
            let Some(content) = source.content.text() else {
                incomplete = true;
                continue;
            };
            match extract::containers(content) {
                Ok(containers) => candidates.push((path, source, containers)),
                Err("environment_workload_unsupported") => (),
                Err(_) => incomplete = true,
            }
        }
        if candidates.is_empty() {
            continue;
        }
        let go_modules = input
            .sources
            .keys()
            .filter(|path| path.starts_with(&prefix) && path.ends_with("/go.mod"))
            .count();
        let python_roots: BTreeSet<_> = input
            .sources
            .keys()
            .filter(|path| {
                path.starts_with(&prefix)
                    && matches!(
                        Path::new(path).file_name().and_then(|n| n.to_str()),
                        Some("pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt")
                    )
            })
            .filter_map(|path| Path::new(path).parent())
            .collect();
        let has_python = a.inputs.iter().any(|i| {
            s(i, "file").starts_with(&prefix)
                && s(i, "file").ends_with(".py")
                && i["language"] == "python"
        });
        let modules = if go_modules > 0 {
            go_modules
        } else if has_python {
            python_roots.len().max(1)
        } else {
            0
        };
        if incomplete
            || candidates.len() != 1
            || candidates[0].2.len() != 1
            || pin.modules.len() != 1
            || modules != 1
            || python_roots.len() > 1
        {
            links.issues.push(issue("environment_auto_scope_ambiguous",json!({"directory":pin.directory,"workloads":candidates.len(),"incomplete_discovery":incomplete,"modules":modules})));
            continue;
        }
        let (_, source, containers) = &candidates[0];
        scopes.push((
            pin.clone(),
            Scope {
                id: format!(
                    "automatic-{}",
                    crate::digest(&json!([source.path, containers[0]]))
                ),
                service: None,
                automatic: true,
                module: pin.modules[0].clone(),
                deployment: Deployment {
                    directory: None,
                    file: source.path.clone(),
                    container: containers[0].clone(),
                },
            },
            true,
        ));
    }
    scopes
}

pub(super) fn attach(
    input: &Input,
    a: &mut Analysis,
    links: &mut services::Links,
    mut scopes: Vec<(Pin, Scope, bool)>,
) -> Result<()> {
    scopes.extend(discover(input, a, links));
    if scopes.is_empty() {
        return Ok(());
    }
    if scopes.len() > 128 {
        links
            .issues
            .push(issue("environment_scope_limit", json!({"limit":128})));
        return Ok(());
    }
    let root = |pin: &Pin, scope: &Scope| {
        if scope.module == "." {
            pin.directory.clone()
        } else {
            format!("{}/{}", pin.directory, scope.module)
        }
    };
    let path = |pin: &Pin, scope: &Scope| {
        format!(
            "{}/{}",
            scope
                .deployment
                .directory
                .as_ref()
                .unwrap_or(&pin.directory),
            scope.deployment.file
        )
    };
    let mut roots = BTreeMap::<String, usize>::new();
    let mut files = BTreeMap::<String, usize>::new();
    let mut ids = BTreeMap::<(String, String), usize>::new();
    for (pin, scope, _) in &scopes {
        *roots.entry(root(pin, scope)).or_default() += 1;
        *files.entry(path(pin, scope)).or_default() += 1;
        *ids.entry((pin.directory.clone(), scope.id.clone()))
            .or_default() += 1;
    }
    // This qualifier remains even when direct literal reads resolve. Inherited
    // image defaults, indirect library reads and runtime deployment are not proved.
    links.issues.push(issue("environment_analysis_limitations", json!({
        "model":"literal_go_python_reads_and_kubernetes_values_v2",
        "limitations":"Explicit pinned ownership or single-module/workload source candidates; Typed Go reads and static Python import-bound file consumers only. No image defaults, indirect library behavior, Helm rendering, runtime environment or deployment verification."
    })));
    for (pin, scope, inventory_complete) in scopes {
        let world = root(&pin, &scope);
        let file = path(&pin, &scope);
        let detail =
            json!({"scope":scope.id,"service":scope.service,"file":file,"consumer_build":world});
        let authority = if scope.automatic {
            "same_repository_candidate"
        } else {
            "pinned_configuration_assertion"
        };
        let mut report = detail.clone();
        report["authority"] = json!(authority);
        report["declared_keys"] = Value::Null;
        report["literal_reads"] = Value::Null;
        report["resolved_reads"] = Value::Null;
        report["unresolved_reads"] = Value::Null;
        report["runtime_overrides"] = Value::Null;
        report["narrowing_eligible"] = json!(false);
        let mut failures = BTreeSet::<&str>::new();
        if ![&scope.id, &scope.deployment.container]
            .into_iter()
            .all(|s| services::identifier(s))
            || (!scope.automatic && scope.service.is_none())
            || scope.service.as_ref().is_some_and(|service| {
                ![
                    &service.name,
                    &service.namespace,
                    &service.environment,
                    &service.configuration,
                ]
                .into_iter()
                .all(|s| services::identifier(s))
            })
            || !pin.modules.contains(&scope.module)
            || (scope.module != "." && store::path(&scope.module).is_err())
            || store::path(&scope.deployment.file).is_err()
        {
            failures.insert("environment_scope_invalid");
        }
        if roots[&world] != 1
            || files[&file] != 1
            || ids[&(pin.directory.clone(), scope.id.clone())] != 1
        {
            failures.insert("environment_scope_ambiguous");
        }
        let source = input.sources.get(&file).filter(|source| {
            input.pins.iter().any(|p| {
                p.directory
                    == *scope
                        .deployment
                        .directory
                        .as_ref()
                        .unwrap_or(&pin.directory)
                    && p.repository_id == source.repository
            })
        });
        if source.is_none() {
            failures.insert("environment_source_unavailable");
        }
        let parsed = if failures.is_empty() {
            match source
                .and_then(|s| s.content.text())
                .ok_or("environment_source_not_text")
                .and_then(|text| extract::deployment(text, &scope.deployment.container))
            {
                Ok(parsed) => Some(parsed),
                Err(reason) => {
                    failures.insert(reason);
                    None
                }
            }
        } else {
            None
        };
        if let Some(parsed) = parsed {
            let source = source.unwrap();
            let content = source.content.text().unwrap();
            if scope
                .service
                .as_ref()
                .is_some_and(|service| parsed.workload["namespace"] != service.namespace)
            {
                failures.insert("environment_namespace_mismatch");
            }
            if !input.issues.is_empty() {
                failures.insert("environment_admission_incomplete");
            }
            if !inventory_complete {
                failures.insert("environment_inventory_incomplete");
            }
            let prefix = format!("{world}/");
            let has_python = a.inputs.iter().any(|i| {
                s(i, "file").starts_with(&prefix)
                    && s(i, "file").ends_with(".py")
                    && i["language"] == "python"
            });
            let has_go = input
                .sources
                .keys()
                .any(|path| path.starts_with(&prefix) && path.ends_with(".go"));
            if (has_go || !has_python) && !a.packages.iter().any(|p| p["consumer_build"] == world) {
                failures.insert("environment_compiler_scope_unavailable");
            }
            if a.issues.iter().any(|i| {
                s(i, "reason") == "python_environment_incomplete"
                    && s(&i["detail"], "file").starts_with(&prefix)
            }) {
                failures.insert("environment_python_analysis_incomplete");
            }
            if has_python
                && a.issues.iter().any(|i| {
                    s(i, "reason") == "basic_unresolved_import"
                        && s(&i["detail"], "directory") == pin.directory
                        && s(&i["detail"]["input"], "source").ends_with(".py")
                        && format!("{}/{}", pin.directory, s(&i["detail"]["input"], "source"))
                            .starts_with(&prefix)
                        && s(&i["detail"]["input"], "module") != "os"
                })
            {
                failures.insert("environment_python_import_unresolved");
            }
            // Other application languages cannot silently disappear from coverage.
            if input.sources.iter().any(|(path, source)| {
                path.starts_with(&format!("{world}/"))
                    && unsupported_source(path, source.content.text().unwrap_or(""))
                    && !matches!(
                        Path::new(path).extension().and_then(|e| e.to_str()),
                        Some(
                            "yaml"
                                | "yml"
                                | "json"
                                | "toml"
                                | "tf"
                                | "tfvars"
                                | "hcl"
                                | "py"
                                | "pyi"
                        )
                    )
            }) {
                failures.insert("environment_consumer_language_unsupported");
            }
            if a.issues.iter().any(|i| i["detail"] == world) {
                failures.insert("environment_compiler_scope_incomplete");
            }
            let scope_revision = crate::digest(&json!([
                pin.repository_id,
                pin.directory,
                source.repository,
                scope,
                parsed.workload
            ]));
            let mut nodes = BTreeMap::new();
            // Namespace mismatches are not usable even as potential links.
            if !failures.contains("environment_namespace_mismatch") {
                for (name, value) in &parsed.values {
                    let id = crate::digest(&json!([
                        "environment-input-v1",
                        pin.repository_id,
                        scope.id,
                        name
                    ]));
                    let span = &parsed.spans[name];
                    let source_pin = input
                        .pins
                        .iter()
                        .find(|p| {
                            p.directory
                                == *scope
                                    .deployment
                                    .directory
                                    .as_ref()
                                    .unwrap_or(&pin.directory)
                        })
                        .unwrap();
                    let symbol = Symbol {
                        key: id.clone(),
                        file: file.clone(),
                        package: file.clone(),
                        name: name.clone(),
                        kind: "environment_input".into(),
                        parent: String::new(),
                        start: span.start,
                        end: span.end,
                        body_start: span.start,
                        line: content[..span.start]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1,
                        end_line: content[..span.end].bytes().filter(|b| *b == b'\n').count() + 1,
                        revision: crate::digest(&json!([scope_revision, name, value])),
                        signature: crate::digest(&json!([scope_revision, name])),
                        implementation: text::sha256(value.as_bytes()),
                        correspondence: id.clone(),
                    };
                    let manifest = input.sources.get(&format!("{}/{}",pin.directory,services::MANIFEST))
                        .map(|manifest| json!({"file":services::MANIFEST,"repository_id":pin.repository_id,"sha256":manifest.sha,"commit":pin.commit,"snapshot":pin.snapshot,"policy_sha256":pin.policy_sha256}));
                    let provenance = json!({"scope":scope.id,"service":scope.service,"workload":parsed.workload,"key":name,
                        "authority":authority,"runtime_observed":false,
                        "source":{"repository_id":source.repository,"file":source.path,"file_sha256":source.sha,"byte_start":span.start,"byte_end":span.end,"commit":source_pin.commit,"snapshot":source_pin.snapshot,"policy_sha256":source_pin.policy_sha256},
                        "manifest":manifest});
                    a.symbols.insert(id.clone(), (world.clone(), symbol));
                    links.nodes.insert(id.clone(), provenance);
                    nodes.insert(name.clone(), id);
                }
            }
            // A callback, external helper or stored function may inspect the
            // environment too. Keep broad scope unless the only unresolved
            // call sites are the standard-library environment operations we model.
            let in_scope = |w: &str, call: &Call| {
                w == world
                    || (call.evidence == "python_import_binding" && call.file.starts_with(&prefix))
            };
            let environment_sites: BTreeSet<_> = a
                .calls
                .iter()
                .filter(|(w, c)| {
                    in_scope(w, c)
                        && matches!(
                            c.relation.as_str(),
                            "reads_environment" | "writes_environment"
                        )
                })
                .map(|(_, c)| (&c.file, c.start, c.end))
                .collect();
            if a.calls.iter().any(|(w, c)| {
                *w == world
                    && !c.reason.is_empty()
                    && matches!(
                        c.relation.as_str(),
                        "calls" | "may_call" | "references_callable"
                    )
                    && !environment_sites.contains(&(&c.file, c.start, c.end))
            }) {
                failures.insert("environment_indirect_consumer_unresolved");
            }
            let mut literal = 0;
            let mut resolved = 0;
            let mut unresolved = 0;
            let mut overrides = 0;
            for (w, call) in &a.calls {
                if !in_scope(w, call) {
                    continue;
                }
                if call.relation == "writes_environment" {
                    overrides += 1;
                    failures.insert("environment_runtime_override");
                }
                if call.relation != "reads_environment" {
                    continue;
                }
                if !extract::key(&call.spelling) {
                    unresolved += 1;
                    failures.insert("environment_dynamic_read");
                    continue;
                }
                literal += 1;
                let caller = key(w, &call.caller);
                let Some(target) = nodes
                    .get(&call.spelling)
                    .filter(|_| a.symbols.contains_key(&caller))
                else {
                    unresolved += 1;
                    failures.insert("environment_read_unbound");
                    continue;
                };
                if links.edges.len() >= 200_000 {
                    failures.insert("environment_edge_limit");
                    unresolved += 1;
                    continue;
                }
                resolved += 1;
                links.edges.push(services::Edge {
                    from: caller, to: target.clone(), relation: "reads_environment",
                    evidence: json!({"configuration":links.nodes[target],"read":call_evidence(w,call)}),
                });
            }
            report["declared_keys"] = json!(parsed.values.len());
            report["literal_reads"] = json!(literal);
            report["resolved_reads"] = json!(resolved);
            report["unresolved_reads"] = json!(unresolved);
            report["runtime_overrides"] = json!(overrides);
            if failures.is_empty() {
                links.environment.files.insert(
                    file.clone(),
                    (scope_revision, crate::digest(&parsed.masked)),
                );
                report["narrowing_eligible"] = json!(true);
            }
        }
        report["issues"] = json!(failures);
        links.environment.reports.push(report);
        for reason in failures {
            links.issues.push(issue(reason, detail.clone()));
        }
    }
    if a.symbols.len() > 200_000 {
        return Err(Error::bad("Review graph bound exceeded (200000 symbols)"));
    }
    Ok(())
}
