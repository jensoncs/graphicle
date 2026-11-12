//! Source-bound service wiring, evaluated only after snapshot authorization.
//!
//! Deployment identity is an explicit, pinned assertion: a path or RPC method
//! name alone cannot establish which service a call reaches. These relationships
//! are labelled configured_static, never compiler-extracted or runtime-observed.
use super::*;

pub(super) const MANIFEST: &str = "graphicle-services.json";
const FORMAT: &str = "graphicle.service-boundaries.v1";
const MAX_BOUNDARIES: usize = 1024;
const MAX_RELATIONSHIPS: usize = 4096;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Citation {
    /// Optional checkout name for separately pinned deployment/IDL repositories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    directory: Option<String>,
    file: String,
    sha256: String,
    byte_start: usize,
    byte_end: usize,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub(super) struct Service {
    pub(super) namespace: String,
    pub(super) name: String,
    pub(super) environment: String,
    pub(super) configuration: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "protocol", rename_all = "snake_case", deny_unknown_fields)]
enum Transport {
    Http {
        authority: Option<String>,
        method: String,
        path: String,
    },
    Grpc {
        authority: Option<String>,
        service: String,
        method: String,
    },
    Event {
        broker: Option<String>,
        topic: Option<String>,
        schema: String,
        group: Option<String>,
    },
}
impl Transport {
    fn protocol(&self) -> &'static str {
        match self {
            Self::Http { .. } => "http",
            Self::Grpc { .. } => "grpc",
            Self::Event { .. } => "event",
        }
    }
    fn known(&self) -> bool {
        match self {
            Self::Http { authority, .. } | Self::Grpc { authority, .. } => authority.is_some(),
            Self::Event { broker, topic, .. } => broker.is_some() && topic.is_some(),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Outbound,
    Inbound,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    id: String,
    service: Service,
    direction: Direction,
    transport: Transport,
    /// Exact declaration, not a name-based lookup.
    owner: Citation,
    /// Request/publish expression or handler/subscriber registration.
    site: Citation,
    /// Pinned route, dial target, broker and environment configuration.
    configuration: Vec<Citation>,
    /// RPC IDL or event schema. HTTP can optionally bind an OpenAPI contract.
    contract: Option<Citation>,
    #[serde(default)]
    dynamic: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    #[serde(default)]
    inventory_complete: bool,
    bindings: Vec<Binding>,
    #[serde(default)]
    environment_scopes: Vec<super::environment::Scope>,
}

pub(super) struct Edge {
    pub from: String,
    pub to: String,
    pub relation: &'static str,
    pub evidence: Value,
}

#[derive(Default)]
pub(super) struct Links {
    pub edges: Vec<Edge>,
    pub nodes: BTreeMap<String, Value>,
    pub manifests: BTreeMap<String, String>,
    pub issues: Vec<Value>,
    pub environment: super::environment::State,
}
impl Links {
    pub fn active(&self) -> bool {
        !self.manifests.is_empty() || self.environment.active()
    }
    pub fn summary(&self) -> Value {
        json!({"enabled":self.active(),"manifests":self.manifests,"boundaries":self.nodes.values().filter(|n| n.get("key").is_none()).count(),
            "resolved_relationships":self.edges.iter().filter(|e| matches!(e.relation,"http_calls"|"grpc_calls"|"event_delivers")).count(),
            "unresolved":self.issues.len(),"authority":if self.manifests.is_empty() && self.environment.active(){"same_repository_candidate"}else{"pinned_configuration_assertions"},
            "runtime_observed":false,"inventory_verified":false,
            "environment":self.environment.summary()})
    }
}

struct Endpoint {
    id: String,
    binding: Binding,
    provenance: Value,
}

pub(super) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
}

fn authority(value: &str) -> bool {
    // Scheme is part of identity (TLS and plaintext are different listeners).
    value.len() <= 512
        && url::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https" | "grpc" | "grpcs")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && matches!(url.path(), "" | "/")
                && url.query().is_none()
                && url.fragment().is_none()
        })
}
fn canonical_authority(value: &str) -> String {
    url::Url::parse(value)
        .map(|u| u.to_string().trim_end_matches('/').to_owned())
        .unwrap_or_default()
}

fn path_segments(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') || path.len() > 1024 || path.contains(['?', '#', '*', '\\']) {
        return None;
    }
    let segments: Vec<_> = path.split('/').skip(1).collect();
    segments
        .iter()
        .all(|s| !matches!(*s, "." | "..") && !s.chars().any(char::is_whitespace))
        .then_some(segments)
}
fn parameter(s: &str) -> bool {
    (s.starts_with('{') && s.ends_with('}') && s.len() > 2) || (s.starts_with(':') && s.len() > 1)
}
fn paths_match(request: &str, route: &str) -> bool {
    let (Some(a), Some(b)) = (path_segments(request), path_segments(route)) else {
        return false;
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a == &b || (!a.is_empty() && !b.is_empty() && (parameter(a) || parameter(b)))
        })
}

fn validate_binding(b: &Binding) -> bool {
    if ![
        &b.id,
        &b.service.namespace,
        &b.service.name,
        &b.service.environment,
        &b.service.configuration,
    ]
    .into_iter()
    .all(|s| identifier(s))
        || b.configuration.is_empty()
        || b.configuration.len() > 8
    {
        return false;
    }
    match &b.transport {
        Transport::Http {
            authority: host,
            method,
            path,
        } => {
            host.as_deref().is_none_or(|h| {
                authority(h) && (h.starts_with("http://") || h.starts_with("https://"))
            }) && matches!(
                method.as_str(),
                "GET"
                    | "POST"
                    | "PUT"
                    | "PATCH"
                    | "DELETE"
                    | "HEAD"
                    | "OPTIONS"
                    | "CONNECT"
                    | "TRACE"
            ) && path_segments(path).is_some()
        }
        Transport::Grpc {
            authority: host,
            service,
            method,
        } => {
            host.as_deref().is_none_or(|h| {
                authority(h) && (h.starts_with("grpc://") || h.starts_with("grpcs://"))
            }) && identifier(service)
                && !service.contains('/')
                && identifier(method)
                && !method.contains('/')
                && b.contract.is_some()
        }
        Transport::Event {
            broker,
            topic,
            schema,
            group,
        } => {
            broker.as_deref().is_none_or(identifier)
                && topic.as_deref().is_none_or(identifier)
                && identifier(schema)
                && b.contract.is_some()
                && group.as_deref().is_none_or(identifier)
                && (b.direction != Direction::Inbound || group.is_some())
        }
    }
}

fn cite(input: &Input, pin: &Pin, c: &Citation) -> Option<Value> {
    let pin = match &c.directory {
        Some(directory) => input.pins.iter().find(|p| p.directory == *directory)?,
        None => pin,
    };
    if store::path(&c.file).is_err() || store::hash(&c.sha256).is_err() {
        return None;
    }
    let source = input
        .sources
        .get(&format!("{}/{}", pin.directory, c.file))?;
    let content = source.content.text()?;
    if source.repository != pin.repository_id
        || source.sha != c.sha256
        || c.byte_start >= c.byte_end
        || content.get(c.byte_start..c.byte_end).is_none()
    {
        return None;
    }
    Some(
        json!({"repository_id":pin.repository_id,"commit":pin.commit,"snapshot":pin.snapshot,"policy_sha256":pin.policy_sha256,
        "file":c.file,"file_sha256":c.sha256,"byte_start":c.byte_start,"byte_end":c.byte_end,
        "line":content[..c.byte_start].bytes().filter(|b|*b==b'\n').count()+1}),
    )
}

fn compatible(a: &Binding, b: &Binding) -> bool {
    if a.direction != Direction::Outbound
        || b.direction != Direction::Inbound
        || a.service.environment != b.service.environment
        || a.service.namespace != b.service.namespace
    {
        return false;
    }
    // Version compatibility is not enough: use the exact admitted contract bytes.
    if a.contract.as_ref().map(|c| &c.sha256) != b.contract.as_ref().map(|c| &c.sha256) {
        return false;
    }
    if b.dynamic {
        return a.transport.protocol() == b.transport.protocol();
    }
    match (&a.transport, &b.transport) {
        (
            Transport::Http {
                authority: Some(a),
                method: am,
                path: ap,
            },
            Transport::Http {
                authority: Some(b),
                method: bm,
                path: bp,
            },
        ) => canonical_authority(a) == canonical_authority(b) && am == bm && paths_match(ap, bp),
        (
            Transport::Grpc {
                authority: Some(a),
                service: as_,
                method: am,
            },
            Transport::Grpc {
                authority: Some(b),
                service: bs,
                method: bm,
            },
        ) => canonical_authority(a) == canonical_authority(b) && as_ == bs && am == bm,
        (
            Transport::Event {
                broker: Some(a),
                topic: Some(at),
                schema: as_,
                ..
            },
            Transport::Event {
                broker: Some(b),
                topic: Some(bt),
                schema: bs,
                ..
            },
        ) => a == b && at == bt && as_ == bs,
        // An unresolved listener is a candidate, never an excuse to select a
        // different, conveniently known listener with the same operation.
        (
            Transport::Http {
                method: am,
                path: ap,
                ..
            },
            Transport::Http {
                authority: None,
                method: bm,
                path: bp,
            },
        ) => am == bm && paths_match(ap, bp),
        (
            Transport::Grpc {
                service: a,
                method: am,
                ..
            },
            Transport::Grpc {
                authority: None,
                service: b,
                method: bm,
            },
        ) => a == b && am == bm,
        (
            Transport::Event {
                broker: a,
                topic: at,
                schema: as_,
                ..
            },
            Transport::Event {
                broker: b,
                topic: bt,
                schema: bs,
                ..
            },
        ) => (a == b || b.is_none()) && (at == bt || bt.is_none()) && as_ == bs,
        _ => false,
    }
}

/// Adds source-backed boundary nodes to the same graph used by native calls.
/// Nothing is persisted independently: a new pin set always relinks, including
/// when compiler facts hit the cache. Removed/changed wiring changes node
/// revisions and path evidence, driving ordinary packet invalidations.
pub(super) fn attach(input: &Input, a: &mut Analysis) -> Result<()> {
    // Keep the cross-world links basic review made (generated code).
    let mut links = Links {
        edges: std::mem::take(&mut a.services.edges),
        ..Links::default()
    };
    let mut endpoints = vec![];
    let mut parsed = BTreeMap::new();
    let mut attempted = 0;
    let mut environment_scopes = vec![];
    let mut owner_index: BTreeMap<(String, usize, usize), BTreeSet<String>> = BTreeMap::new();
    for (id, (_, symbol)) in &a.symbols {
        if matches!(symbol.kind.as_str(), "function" | "method" | "closure") {
            owner_index
                .entry((symbol.file.clone(), symbol.start, symbol.end))
                .or_default()
                .insert(id.clone());
        }
    }
    for pin in &input.pins {
        let path = format!("{}/{}", pin.directory, MANIFEST);
        let Some(source) = input.sources.get(&path) else {
            continue;
        };
        links.manifests.insert(path.clone(), source.sha.clone());
        let manifest = (source.content.bytes().len() <= 512 * 1024)
            .then(|| serde_json::from_slice::<Manifest>(source.content.bytes()).ok())
            .flatten();
        let Some(m) = manifest.filter(|m| {
            (m.format == "graphicle.service-boundaries.v2"
                || m.format == FORMAT && m.environment_scopes.is_empty())
                && m.bindings.len() <= MAX_BOUNDARIES
                && m.environment_scopes.len() <= 128
        }) else {
            links
                .issues
                .push(issue("service_manifest_invalid", json!({"file":path})));
            continue;
        };
        if !m.inventory_complete {
            links
                .issues
                .push(issue("service_inventory_incomplete", json!({"file":path})));
        }
        environment_scopes.extend(
            m.environment_scopes
                .into_iter()
                .map(|scope| (pin.clone(), scope, m.inventory_complete)),
        );
        let mut ids = BTreeSet::new();
        let duplicates: BTreeSet<_> = m
            .bindings
            .iter()
            .filter(|b| !ids.insert(b.id.clone()))
            .map(|b| b.id.clone())
            .collect();
        for b in m.bindings {
            let detail = json!({"file":path,"binding":b.id});
            if attempted >= MAX_BOUNDARIES {
                links.issues.push(issue(
                    "service_boundary_limit",
                    json!({"limit":MAX_BOUNDARIES}),
                ));
                break;
            }
            attempted += 1;
            if !validate_binding(&b) || duplicates.contains(&b.id) {
                links
                    .issues
                    .push(issue("service_binding_invalid_or_duplicate", detail));
                continue;
            }
            if b.owner
                .directory
                .as_ref()
                .is_some_and(|d| d != &pin.directory)
                || b.site
                    .directory
                    .as_ref()
                    .is_some_and(|d| d != &pin.directory)
                || !pin
                    .modules
                    .iter()
                    .any(|m| m == "." || b.owner.file.starts_with(&format!("{m}/")))
            {
                links
                    .issues
                    .push(issue("service_owner_outside_scope", detail));
                continue;
            }
            let evidence: Option<Vec<Value>> = std::iter::once(&b.owner)
                .chain(std::iter::once(&b.site))
                .chain(&b.configuration)
                .chain(b.contract.iter())
                .map(|c| cite(input, pin, c))
                .collect();
            let Some(evidence) = evidence else {
                links
                    .issues
                    .push(issue("service_source_unavailable_or_stale", detail));
                continue;
            };
            let owner_file = format!("{}/{}", pin.directory, b.owner.file);
            // Reuse exact syntactic declaration spans for other languages. This
            // identifies the cited owner, not its unknown local call graph.
            // Go must keep using the native build-selected declarations.
            if Path::new(&b.owner.file)
                .extension()
                .is_some_and(|e| e != "go")
            {
                let source = &input.sources[&owner_file];
                let content = source.content.text().unwrap();
                if !parsed.contains_key(&owner_file) {
                    parsed.insert(
                        owner_file.clone(),
                        graphicle_memory::structure::analyze(&b.owner.file, content)?,
                    );
                    links.issues.push(issue(
                        "service_local_analysis_incomplete",
                        json!({"file":owner_file}),
                    ));
                }
                let facts = &parsed[&owner_file];
                let matches: Vec<_> = facts["symbols"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|symbol| {
                        symbol["byte_start"] == b.owner.byte_start
                            && symbol["byte_end"] == b.owner.byte_end
                            && matches!(s(symbol, "kind"), "function" | "method")
                    })
                    .collect();
                if graphicle_memory::structure::declaration_spans_complete(facts)
                    && matches.len() == 1
                {
                    let symbol = matches[0];
                    let id = crate::digest(&json!([
                        "service-owner-v1",
                        pin.repository_id,
                        b.owner.file,
                        symbol["name"],
                        symbol["kind"]
                    ]));
                    owner_index
                        .entry((owner_file.clone(), b.owner.byte_start, b.owner.byte_end))
                        .or_default()
                        .insert(id.clone());
                    a.symbols.entry(id.clone()).or_insert_with(|| {
                        (
                            pin.directory.clone(),
                            Symbol {
                                key: id.clone(),
                                file: owner_file.clone(),
                                package: format!("source:{}", b.owner.file),
                                name: s(symbol, "name").into(),
                                kind: s(symbol, "kind").into(),
                                parent: String::new(),
                                start: b.owner.byte_start,
                                end: b.owner.byte_end,
                                body_start: b.owner.byte_start,
                                line: symbol["line"].as_u64().unwrap_or(1) as usize,
                                end_line: symbol["end_line"].as_u64().unwrap_or(1) as usize,
                                revision: text::sha256(
                                    &content.as_bytes()[b.owner.byte_start..b.owner.byte_end],
                                ),
                                signature: crate::digest(&symbol["signature"]),
                                implementation: text::sha256(
                                    &content.as_bytes()[b.owner.byte_start..b.owner.byte_end],
                                ),
                                correspondence: id,
                            },
                        )
                    });
                }
            }
            let owners: Vec<_> = owner_index
                .get(&(owner_file, b.owner.byte_start, b.owner.byte_end))
                .into_iter()
                .flatten()
                .filter_map(|id| a.symbols.get(id).map(|owner| (id, owner)))
                .filter(|(_, (world, _))| {
                    pin.modules.iter().any(|m| {
                        world
                            == &if m == "." {
                                pin.directory.clone()
                            } else {
                                format!("{}/{m}", pin.directory)
                            }
                    })
                })
                .map(|(id, owner)| (id.clone(), owner.clone()))
                .collect();
            if owners.len() != 1 {
                links.issues.push(issue("service_owner_unresolved", detail));
                continue;
            }
            if b.direction == Direction::Outbound
                && (b.site.file != b.owner.file
                    || b.site.byte_start < b.owner.byte_start
                    || b.site.byte_end > b.owner.byte_end)
            {
                links
                    .issues
                    .push(issue("service_call_outside_owner", detail));
                continue;
            }
            let (owner_id, (world, owner)) = &owners[0];
            let id = crate::digest(&json!(["service-boundary-v1", pin.repository_id, b.id]));
            let provenance = json!({"binding":b.id,"service":b.service,"direction":b.direction,"transport":b.transport,
                "authority":"pinned_configuration_assertion","runtime_observed":false,"citations":evidence,
                "manifest":{"file":MANIFEST,"sha256":source.sha,"repository_id":pin.repository_id,"commit":pin.commit,"snapshot":pin.snapshot,"policy_sha256":pin.policy_sha256}});
            let file = format!("{}/{}", pin.directory, b.site.file);
            let site = input.sources[&file].content.text().unwrap();
            let revision = crate::digest(&json!([b, source.sha]));
            let kind = match (&b.transport, b.direction) {
                (Transport::Http { .. }, Direction::Outbound) => "http_call",
                (Transport::Http { .. }, _) => "http_endpoint",
                (Transport::Grpc { .. }, Direction::Outbound) => "grpc_call",
                (Transport::Grpc { .. }, _) => "grpc_method",
                (Transport::Event { .. }, Direction::Outbound) => "event_publisher",
                (Transport::Event { .. }, _) => "event_consumer",
            };
            a.symbols.insert(
                id.clone(),
                (
                    world.clone(),
                    Symbol {
                        key: id.clone(),
                        file,
                        package: owner.package.clone(),
                        name: format!("{}::{}", b.service.name, b.id),
                        kind: kind.into(),
                        parent: owner.name.clone(),
                        start: b.site.byte_start,
                        end: b.site.byte_end,
                        body_start: b.site.byte_start,
                        line: site[..b.site.byte_start]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1,
                        end_line: site[..b.site.byte_end]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1,
                        revision,
                        signature: crate::digest(&json!(b.transport)),
                        implementation: text::sha256(
                            &site.as_bytes()[b.site.byte_start..b.site.byte_end],
                        ),
                        correspondence: id.clone(),
                    },
                ),
            );
            let (from, to, relation) = if b.direction == Direction::Outbound {
                (owner_id.clone(), id.clone(), "invokes_boundary")
            } else {
                (id.clone(), owner_id.clone(), "handles_boundary")
            };
            links.edges.push(Edge {
                from,
                to,
                relation,
                evidence: provenance.clone(),
            });
            links.nodes.insert(id.clone(), provenance.clone());
            if b.direction == Direction::Inbound && (b.dynamic || !b.transport.known()) {
                links.issues.push(issue(
                    "service_dynamic_configuration",
                    json!({"boundary":id}),
                ));
            }
            endpoints.push(Endpoint {
                id,
                binding: b,
                provenance,
            });
        }
    }
    for pin in &input.pins {
        if !links.manifests.is_empty()
            && !links
                .manifests
                .contains_key(&format!("{}/{}", pin.directory, MANIFEST))
        {
            links.issues.push(issue(
                "service_inventory_unavailable",
                json!({"directory":pin.directory}),
            ));
        }
    }
    'callers: for caller in endpoints
        .iter()
        .filter(|e| e.binding.direction == Direction::Outbound)
    {
        if caller.binding.dynamic || !caller.binding.transport.known() {
            links.issues.push(issue(
                "service_dynamic_configuration",
                json!({"boundary":caller.id}),
            ));
            continue;
        }
        let candidates: Vec<_> = endpoints
            .iter()
            .filter(|e| compatible(&caller.binding, &e.binding))
            .collect();
        let mut groups: BTreeMap<String, Vec<&Endpoint>> = BTreeMap::new();
        for candidate in candidates {
            // Independent event subscriber groups fan out. Replicas/duplicate
            // handlers within one service+group remain ambiguous.
            let group = match &candidate.binding.transport {
                // Consumer groups belong to the broker/topic, not the service.
                // Different services claiming the same group compete; neither
                // is a guaranteed recipient.
                Transport::Event { group, .. } => crate::digest(&json!(group)),
                _ => String::new(),
            };
            groups.entry(group).or_default().push(candidate);
        }
        if groups.is_empty() {
            links.issues.push(issue(
                "service_target_unavailable",
                json!({"boundary":caller.id}),
            ));
        }
        for candidates in groups.values() {
            if candidates.len() != 1 {
                links.issues.push(issue("service_target_ambiguous",json!({"boundary":caller.id,"candidates":candidates.iter().map(|c|&c.id).collect::<Vec<_>>()})));
                continue;
            }
            let target = candidates[0];
            if target.binding.dynamic || !target.binding.transport.known() {
                links.issues.push(issue(
                    "service_dynamic_configuration",
                    json!({"boundary":caller.id,"candidate":target.id}),
                ));
                continue;
            }
            if links.edges.len() >= MAX_RELATIONSHIPS {
                links.issues.push(issue(
                    "service_relationship_limit",
                    json!({"limit":MAX_RELATIONSHIPS}),
                ));
                break 'callers;
            }
            let relation = match caller.binding.transport.protocol() {
                "http" => "http_calls",
                "grpc" => "grpc_calls",
                _ => "event_delivers",
            };
            links.edges.push(Edge{from:caller.id.clone(),to:target.id.clone(),relation,
                evidence:json!({"relation":relation,"evidence":"configured_static","caller":caller.id,"callee":target.id,"source":caller.provenance,"target":target.provenance})});
        }
    }
    for endpoint in endpoints
        .iter()
        .filter(|e| e.binding.direction == Direction::Outbound)
    {
        let targets:Vec<_>=links.edges.iter().filter(|e|e.from==endpoint.id)
            .filter_map(|edge|endpoints.iter().find(|e|e.id==edge.to))
            .map(|target|json!({"boundary":target.id,"binding":target.binding.id,"service":target.binding.service,
                "owner":target.provenance["citations"][0],"site":target.provenance["citations"][1]})).collect();
        let partial = links
            .issues
            .iter()
            .any(|i| i["detail"]["boundary"] == endpoint.id)
            || links
                .issues
                .iter()
                .any(|i| i["reason"] == "service_relationship_limit");
        links.nodes.get_mut(&endpoint.id).unwrap()["resolution"] = json!({
            "status":if targets.is_empty(){"unresolved"}else if partial{"partial"}else{"configured_targets"},"targets":targets});
    }
    super::environment::attach(input, a, &mut links, environment_scopes)?;
    a.issues.extend(links.issues.iter().cloned());
    a.services = links;
    Ok(())
}

#[cfg(test)]
#[path = "service_graph_tests.rs"]
mod tests;
