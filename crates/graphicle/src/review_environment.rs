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
    /// Directories of other workloads nested in this one's code root: their
    /// code runs in another pod, so it never reads this environment.
    #[serde(skip)]
    excluded: Vec<String>,
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
            "model":"literal_go_python_reads_and_kubernetes_values_v4","runtime_observed":false})
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
        // Chart values live in `<workload>/infra/<environment>/`. Several
        // environments describe one workload: only production is scoped, the
        // others stay broad file dependencies. Values under a subdirectory
        // belong to a nested workload, left out of this one's scope.
        let all_chart = candidates
            .iter()
            .all(|(_, source, _)| source.content.text().is_some_and(extract::chart_values));
        let owner = |path: &str| path.split_once("/infra/").map(|(dir, _)| dir.to_owned());
        let root = if pin.modules.len() == 1 && pin.modules[0] != "." {
            format!("{}/{}", pin.directory, pin.modules[0])
        } else {
            pin.directory.clone()
        };
        let nested: BTreeSet<String> = candidates
            .iter()
            .filter_map(|(path, ..)| owner(path))
            .filter(|dir| dir.starts_with(&format!("{root}/")))
            .collect();
        // A nested directory with Go code may build into the root module.
        let separable = all_chart
            && candidates.iter().all(|(path, ..)| owner(path).is_some())
            && !input.sources.keys().any(|path| {
                path.ends_with(".go")
                    && nested
                        .iter()
                        .any(|dir| path.starts_with(&format!("{dir}/")))
            });
        let excluded: Vec<String> = if separable {
            nested.into_iter().collect()
        } else {
            vec![]
        };
        let owned = |path: &str| {
            !excluded
                .iter()
                .any(|dir| path.starts_with(&format!("{dir}/")))
        };
        if separable {
            candidates.retain(|(path, ..)| owner(path).as_deref() == Some(root.as_str()));
            let production: Vec<_> = candidates
                .iter()
                .filter(|(path, ..)| path.split('/').any(|part| part == "production"))
                .cloned()
                .collect();
            if candidates.len() > 1 && production.len() == 1 {
                candidates = production;
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
                    && owned(path)
                    && matches!(
                        Path::new(path).file_name().and_then(|n| n.to_str()),
                        Some("pyproject.toml" | "setup.py" | "setup.cfg" | "requirements.txt")
                    )
            })
            .filter_map(|path| Path::new(path).parent())
            .collect();
        let has_python = a.inputs.iter().any(|i| {
            s(i, "file").starts_with(&prefix)
                && owned(s(i, "file"))
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
                excluded: excluded.clone(),
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

/// A ConfigMap that supplies some of a workload's env values.
struct MapInput<'a> {
    /// The source's key in `Input::sources`.
    file: String,
    source: &'a Source,
    data: extract::ConfigMapData,
    /// Every data key is one of the scoped workload's references and nothing
    /// else names the ConfigMap, so a data change reaches only those readers.
    /// ponytail: "names" is a substring test over the directory's sources;
    /// parse references if shared names cost too much narrowing.
    narrowable: bool,
}

/// The one ConfigMap of the workload's namespace behind each
/// `configMapKeyRef`: in the deployment's directory, else in exactly one other
/// admitted pin (an Atlas snapshot, `scripts/atlas_config_snapshot.py`).
/// Missing or ambiguous fails closed.
/// ponytail: name and namespace identify the object within one cluster; a pin
/// for another cluster's ConfigMap of the same name would be taken as this one.
fn config_maps<'a>(
    input: &'a Input,
    directory: &str,
    deployment: &str,
    parsed: &extract::DeploymentEnvironment,
) -> std::result::Result<BTreeMap<String, MapInput<'a>>, &'static str> {
    let names: BTreeSet<&str> = parsed
        .references
        .values()
        .map(|(map, _)| map.as_str())
        .collect();
    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    let prefix = format!("{directory}/");
    let pin_of = |path: &str| {
        input
            .pins
            .iter()
            .map(|pin| &pin.directory)
            .find(|dir| path.starts_with(&format!("{dir}/")))
            .cloned()
    };
    let mut found = BTreeMap::<String, Vec<(&String, &Source, extract::ConfigMapData)>>::new();
    for (path, source) in input.sources.iter().filter(|(path, _)| {
        pin_of(path).is_some()
            && *path != deployment
            && matches!(
                Path::new(path).extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml")
            )
    }) {
        let Some(text) = source.content.text() else {
            continue;
        };
        if let Ok(data) = extract::config_map(text)
            && names.contains(data.name.as_str())
            && parsed.workload["namespace"] == data.namespace
        {
            found
                .entry(data.name.clone())
                .or_default()
                .push((path, source, data));
        }
    }
    let mut maps = BTreeMap::new();
    for name in names {
        let candidates = found.remove(name).unwrap_or_default();
        let (local, other): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|(path, ..)| path.starts_with(&prefix));
        let chosen = if local.is_empty() { other } else { local };
        let Ok([(file, source, data)]) = <[_; 1]>::try_from(chosen) else {
            return Err("environment_value_from_unresolved");
        };
        let map_prefix = format!("{}/", pin_of(file).unwrap());
        let referenced: BTreeSet<&str> = parsed
            .references
            .values()
            .filter(|(map, _)| map == name)
            .map(|(_, key)| key.as_str())
            .collect();
        if referenced.iter().any(|key| !data.values.contains_key(*key)) {
            return Err("environment_value_from_unresolved");
        }
        let named_elsewhere = input.sources.iter().any(|(path, other)| {
            (path.starts_with(&prefix) || path.starts_with(&map_prefix))
                && *path != deployment
                && path != file
                && other.content.text().is_none_or(|text| text.contains(name))
        });
        let narrowable = !named_elsewhere
            && !parsed.config_maps_elsewhere.contains(name)
            && data
                .values
                .keys()
                .all(|key| referenced.contains(key.as_str()));
        maps.insert(
            name.to_owned(),
            MapInput {
                file: file.clone(),
                source,
                data,
                narrowable,
            },
        );
    }
    Ok(maps)
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
    let hosts = addresses::index(input, &mut links.issues);
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
        "model":"literal_go_python_reads_and_kubernetes_values_v4",
        "limitations":"Explicit pinned ownership or single-module/workload source candidates; Typed Go reads and static Python import-bound file consumers only. ConfigMap `configMapKeyRef` values resolve from the deployment's directory, else from exactly one other admitted pin (such as an Atlas snapshot); secret and pod-field references bind readers without a value; a URL value's host addresses the one application that admitted chart `ingress.hosts`, else an admitted host map (`graphicle-host-map.json`), names (no edge when several do); k8s-application chart values are read by that chart's env template (production when several environments exist; workloads nested under `<dir>/infra/` are left out of the root workload's scope); envFrom does not, and Atlas config only through an admitted snapshot. No image defaults, indirect library behavior, general Helm rendering, runtime environment or deployment verification."
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
        let deployment_directory = scope
            .deployment
            .directory
            .clone()
            .unwrap_or_else(|| pin.directory.clone());
        let (parsed, maps) = match parsed
            .map(|p| config_maps(input, &deployment_directory, &file, &p).map(|maps| (p, maps)))
        {
            Some(Ok((parsed, maps))) => (Some(parsed), maps),
            Some(Err(reason)) => {
                failures.insert(reason);
                (None, BTreeMap::new())
            }
            None => (None, BTreeMap::new()),
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
            let owns = |path: &str| {
                path.starts_with(&prefix)
                    && !scope
                        .excluded
                        .iter()
                        .any(|dir| path.starts_with(&format!("{dir}/")))
            };
            let has_python = a.inputs.iter().any(|i| {
                owns(s(i, "file")) && s(i, "file").ends_with(".py") && i["language"] == "python"
            });
            let has_go = input
                .sources
                .keys()
                .any(|path| owns(path) && path.ends_with(".go"));
            if (has_go || !has_python) && !a.packages.iter().any(|p| p["consumer_build"] == world) {
                failures.insert("environment_compiler_scope_unavailable");
            }
            if a.issues.iter().any(|i| {
                s(i, "reason") == "python_environment_incomplete" && owns(s(&i["detail"], "file"))
            }) {
                failures.insert("environment_python_analysis_incomplete");
            }
            if has_python
                && a.issues.iter().any(|i| {
                    s(i, "reason") == "basic_unresolved_import"
                        && s(&i["detail"], "directory") == pin.directory
                        && s(&i["detail"]["input"], "source").ends_with(".py")
                        && owns(&format!(
                            "{}/{}",
                            pin.directory,
                            s(&i["detail"]["input"], "source")
                        ))
                        && s(&i["detail"]["input"], "module") != "os"
                })
            {
                failures.insert("environment_python_import_unresolved");
            }
            // Other application languages cannot silently disappear from coverage.
            if input.sources.iter().any(|(path, source)| {
                owns(path)
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
                let literal_inputs = parsed.values.iter().map(|(name, value)| {
                    (
                        true,
                        name,
                        value,
                        file.clone(),
                        source,
                        content,
                        parsed.spans[name].clone(),
                    )
                });
                let map_inputs = parsed.references.iter().map(|(name, (map, key))| {
                    let map = &maps[map];
                    (
                        true,
                        name,
                        &map.data.values[key],
                        map.file.clone(),
                        map.source,
                        map.source.content.text().unwrap(),
                        map.data.spans[key].clone(),
                    )
                });
                // A secret or pod field has no value here; its reference stands in.
                let opaque_inputs = parsed.opaque.iter().map(|(name, (reference, span))| {
                    (
                        false,
                        name,
                        reference,
                        file.clone(),
                        source,
                        content,
                        span.clone(),
                    )
                });
                for (addressable, name, value, file, source, content, span) in
                    literal_inputs.chain(map_inputs).chain(opaque_inputs)
                {
                    let id = crate::digest(&json!([
                        "environment-input-v1",
                        pin.repository_id,
                        scope.id,
                        name
                    ]));
                    // The pin holding the cited file: a ConfigMap may be in another.
                    let source_pin = input
                        .pins
                        .iter()
                        .find(|p| {
                            p.repository_id == source.repository
                                && file.starts_with(&format!("{}/", p.directory))
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
                    // A secret or pod field has no value to address anything.
                    if addressable {
                        addresses::link(
                            input,
                            &hosts,
                            a,
                            links,
                            addresses::Address {
                                input: &id,
                                world: &world,
                                key: name,
                                value,
                                scope: &scope.id,
                            },
                        );
                    }
                    nodes.insert(name.clone(), id);
                }
            }
            // A callback, external helper or stored function may inspect the
            // environment too. Keep broad scope unless the only unresolved
            // call sites are the standard-library environment operations we model.
            let in_scope = |w: &str, call: &Call| {
                w == world || (call.evidence == "python_import_binding" && owns(&call.file))
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
            report["declared_keys"] =
                json!(parsed.values.len() + parsed.references.len() + parsed.opaque.len());
            report["literal_reads"] = json!(literal);
            report["resolved_reads"] = json!(resolved);
            report["unresolved_reads"] = json!(unresolved);
            report["runtime_overrides"] = json!(overrides);
            if failures.is_empty() {
                links.environment.files.insert(
                    file.clone(),
                    (scope_revision.clone(), crate::digest(&parsed.masked)),
                );
                for map in maps.values().filter(|map| map.narrowable) {
                    links.environment.files.insert(
                        map.file.clone(),
                        (scope_revision.clone(), crate::digest(&map.data.masked)),
                    );
                }
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
