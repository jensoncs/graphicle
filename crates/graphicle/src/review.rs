//! Go-first change impact over immutable, authorized snapshot sets. Resolution
//! is compiler-owned; graph traversal and context accounting never turn an
//! incomplete build into an unqualified no-impact result.
use crate::{
    Error, Result, local,
    store::{self, Store, s},
};
use graphicle::{affected, graph::Graph};
use graphicle_memory::text;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[path = "review_reuse.rs"]
mod reuse;
#[path = "service_graph.rs"]
mod services;
#[path = "review_flows.rs"]
mod flows;
#[path = "review_basic.rs"]
mod basic;

const FORMAT: &str = "graphicle.review.v1";
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
fn depth() -> usize {
    8
}
fn budget() -> usize {
    16000
}
fn modules() -> Vec<String> {
    vec![".".into()]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub repository_id: String,
    pub commit: String,
    pub snapshot: String,
    pub policy_sha256: String,
    /// Relative checkout name, stable between base and head. Replacements and
    /// workspace uses are resolved by Go within this admitted snapshot layout.
    pub directory: String,
    #[serde(default = "modules")]
    pub modules: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub goos: String,
    pub goarch: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub tests: bool,
    #[serde(default)]
    pub workspace: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub repository_id: String,
    pub base: Vec<Pin>,
    pub head: Vec<Pin>,
    pub profile: Profile,
    #[serde(default = "depth")]
    pub max_depth: usize,
    #[serde(default = "budget")]
    pub max_tokens: usize,
    /// Receipt issued by this store to this principal. Arbitrary client-supplied
    /// shown spans cannot suppress current evidence.
    #[serde(default)]
    pub previous_packet_id: Option<String>,
    /// Request-local historical assertions and conservative static domain cues.
    #[serde(default)]
    pub business_context: Option<crate::business_context::Options>,
    /// Opt in to source bodies referenced from the previous validated receipt.
    #[serde(default)]
    pub reuse_text: bool,
    /// Previously reviewed head entry IDs; results certify input equivalence,
    /// never that a human/model actually completed a review.
    #[serde(default)]
    pub reviewed_entries: Vec<String>,
}
#[derive(Clone)]
struct Source {
    repository: String,
    path: String,
    sha: String,
    content: crate::source_object::Content,
}
struct Input {
    pins: Vec<Pin>,
    sources: BTreeMap<String, Source>,
    issues: Vec<Value>,
    bindings: Vec<(Pin, bool)>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Symbol {
    key: String,
    file: String,
    package: String,
    name: String,
    kind: String,
    parent: String,
    start: usize,
    end: usize,
    body_start: usize,
    line: usize,
    end_line: usize,
    revision: String,
    signature: String,
    implementation: String,
    correspondence: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Call {
    caller: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    target_file: String,
    #[serde(default)]
    target_package: String,
    target_offset: usize,
    spelling: String,
    file: String,
    start: usize,
    end: usize,
    line: usize,
    relation: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    evidence: String,
    #[serde(default)]
    discarded_error: bool,
}
#[derive(Deserialize)]
struct World {
    root: String,
    symbols: Vec<Symbol>,
    calls: Vec<Call>,
    packages: Vec<Value>,
    issues: Vec<String>,
    #[serde(default)]
    inputs: Vec<Value>,
}
#[derive(Deserialize)]
struct Facts {
    analyzer: String,
    toolchain: String,
    worlds: Vec<World>,
}
// Deterministic type-check failures can be reused for the exact admitted tree.
// They remain failures. Never retain incomplete discovery, parse/read failures,
// missing input inventories, or helper failures as compiler facts.
fn cacheable_facts(facts: &Facts) -> bool {
    !facts.worlds.is_empty() && facts.worlds.iter().all(|world| {
        if world.issues.is_empty() { return true; }
        let packages: BTreeMap<_, _> = world.packages.iter().map(|p| (s(p,"path"), p)).collect();
        !packages.is_empty() && world.inputs.iter().any(|i| i["role"] == "selected") && world.issues.iter().all(|reason| {
            if let Some(package) = reason.strip_prefix("incomplete_package:") {
                packages.contains_key(package)
            } else if let Some(package) = reason.strip_prefix("type_check_failed:") {
                world.issues.iter().any(|r| r == &format!("incomplete_package:{package}"))
            } else { false }
        }) && world.inputs.iter().filter(|i| i["role"] == "selected").all(|i| {
            packages.get(s(i,"package")).is_some_and(|p| p["files"].as_array().is_some_and(|files| files.iter().any(|file| file == &i["file"])))
        })
    })
}

struct Analysis {
    symbols: BTreeMap<String, (String, Symbol)>,
    calls: Vec<(String, Call)>,
    packages: Vec<Value>,
    issues: Vec<Value>,
    analyzer: String,
    toolchain: String,
    inputs: Vec<Value>,
    raw: Option<Value>,
    services: services::Links,
}

fn issue(reason: &str, detail: Value) -> Value {
    json!({"reason":reason,"detail":detail})
}
fn key(world: &str, symbol: &str) -> String {
    crate::digest(&json!([world, symbol]))
}
fn validate(r: &Request) -> Result<()> {
    if r.base.is_empty()
        || r.head.is_empty()
        || r.base.len() > 64
        || r.head.len() > 64
        || r.max_depth > 32
        || !(512..=64000).contains(&r.max_tokens)
    {
        return Err(Error::bad(
            "Review needs 1–64 pinned repositories per side, depth 0–32 and 512–64000 tokens",
        ));
    }
    for pins in [&r.base, &r.head] {
        let mut directories = BTreeSet::new();
        let mut repos = BTreeSet::new();
        for pin in pins {
            store::hash(&pin.snapshot)?;
            store::hash(&pin.policy_sha256)?;
            if ![40, 64].contains(&pin.commit.len())
                || !pin
                    .commit
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::bad("Use full lowercase Git commit IDs"));
            }
            if pin.directory.is_empty()
                || pin.directory.len() > 100
                || !pin
                    .directory
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || !directories.insert(&pin.directory)
                || !repos.insert(&pin.repository_id)
            {
                return Err(Error::bad(
                    "Snapshot checkout names and repositories must be unique",
                ));
            }
            if pin.modules.is_empty() || pin.modules.len() > 64 {
                return Err(Error::bad("Declare 1–64 module roots per repository"));
            }
            let mut seen = BTreeSet::new();
            for module in &pin.modules {
                if module != "." {
                    store::path(module)?;
                }
                if !seen.insert(module) {
                    return Err(Error::bad("Duplicate module root"));
                }
            }
        }
        if !pins.iter().any(|p| p.repository_id == r.repository_id) {
            return Err(Error::bad("Anchor repository must be pinned on both sides"));
        }
    }
    let directories: BTreeMap<_, _> = r
        .base
        .iter()
        .map(|p| (&p.repository_id, &p.directory))
        .collect();
    for p in &r.head {
        if directories
            .get(&p.repository_id)
            .is_some_and(|d| *d != &p.directory)
        {
            return Err(Error::bad(
                "Keep checkout names stable between base and head",
            ));
        }
    }
    validate_profile(&r.profile)?;
    if let Some(id) = &r.previous_packet_id {
        store::hash(id)?;
    }
    if r.reviewed_entries.len() > 1000
        || (!r.reviewed_entries.is_empty() && r.previous_packet_id.is_none())
    {
        return Err(Error::bad(
            "Review coverage reuse requires a previous packet and at most 1000 entry IDs",
        ));
    }
    for id in &r.reviewed_entries {
        store::hash(id)?;
    }
    Ok(())
}
pub(crate) fn validate_profile(profile: &Profile) -> Result<()> {
    for option in [&profile.goos, &profile.goarch] {
        if option.is_empty()
            || option.len() > 32
            || !option.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(Error::bad("Invalid Go platform"));
        }
    }
    if profile.tags.len() > 64
        || profile.tags.iter().any(|t| {
            t.is_empty()
                || t.len() > 100
                || !t
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.".contains(&b))
        })
    {
        return Err(Error::bad("Invalid build tags"));
    }
    if !profile.workspace.is_empty() {
        store::path(&profile.workspace)?;
    }
    Ok(())
}
fn load(
    store: &Store,
    workspace: &str,
    token: &str,
    pins: &[Pin],
    profile: &Profile,
) -> Result<Input> {
    store.authorized_read(workspace,token,"read",|db,principal|{
        let mut input=Input{pins:pins.to_vec(),sources:BTreeMap::new(),issues:vec![],bindings:vec![]};let mut bytes=0usize;
        for pin in pins {
            if store::repo(db,principal,&pin.repository_id).is_err(){input.issues.push(issue("unavailable_snapshot",json!(pin.directory)));continue}
            let row=store::one(db,"SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?",params![pin.repository_id,pin.snapshot])?;
            let Some(row)=row else {input.issues.push(issue("unavailable_snapshot",json!(pin.directory)));continue};
            let manifest:Value=serde_json::from_str(s(&row,"manifest"))?;let metadata:Value=serde_json::from_str(s(&row,"metadata"))?;
            let binding=store::one(db,"SELECT scope_complete FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?",params![pin.repository_id,pin.snapshot,pin.commit,pin.policy_sha256])?;
            let legacy=metadata["commit"]==pin.commit && metadata["policy_sha256"]==pin.policy_sha256;
            if (binding.is_none() && !legacy)||crate::digest(&manifest)!=pin.snapshot {input.issues.push(issue("snapshot_binding_mismatch",json!(pin.directory)));continue}
            let complete = binding.as_ref().map(|v|v["scope_complete"]==1).unwrap_or(legacy && metadata["scope_complete"]==true);
            input.bindings.push((pin.clone(), complete));
            if let Some(preparation) = metadata.get("dependency_preparation") {
                let prepared_profile = serde_json::from_value::<Profile>(preparation["profile"].clone());
                if prepared_profile.ok().and_then(|p| serde_json::to_value(p).ok()) != Some(serde_json::to_value(profile)?) {
                    input.issues.push(issue("dependency_preparation_profile_mismatch", json!(pin.directory)));
                }
            }
            if !complete {input.issues.push(issue("source_admission_incomplete",json!(pin.directory)));}
            let files=manifest.as_object().ok_or_else(||Error::bad("Invalid stored source manifest"))?;
            for (path,entry) in files {
                store::path(path)?;
                if local::excluded_source_path(path) {input.issues.push(issue("excluded_source",json!(pin.directory)));continue}
                let size=entry["bytes"].as_u64().ok_or_else(||Error::bad("Invalid source size"))?;
                if size>MAX_SOURCE_BYTES as u64||bytes+size as usize>MAX_SOURCE_BYTES||input.sources.len()>=MAX_FILES{return Err(Error::bad("Review source bound exceeded (64 MiB / 20000 files per side)"));}
                let source=crate::source_object::load(db,&pin.repository_id,s(entry,"sha256"))?;
                let Some(content)=source else {input.issues.push(issue("missing_source_object",json!(pin.directory)));continue};
                if content.bytes().len()!=size as usize{return Err(Error::conflict("Pinned source failed integrity verification"));}
                bytes+=content.bytes().len();input.sources.insert(format!("{}/{}",pin.directory,path),Source{repository:pin.repository_id.clone(),path:path.clone(),sha:s(entry,"sha256").into(),content});
            }
        }
        Ok(input)
    })
}
fn analyze(input: &Input, profile: &Profile, cached: Option<Value>) -> Result<Analysis> {
    let mut out = analyze_go(input, profile, cached)?;
    basic::extend(input, &mut out)?;
    if out.symbols.len() > 200_000 {
        return Err(Error::bad("Review graph bound exceeded (200000 symbols)"));
    }
    Ok(out)
}

fn analyze_go(input: &Input, profile: &Profile, cached: Option<Value>) -> Result<Analysis> {
    let roots: Vec<_> = input
        .pins
        .iter()
        .flat_map(|p| {
            p.modules
                .iter()
                .filter(move |m| {
                    let root = if *m == "." { format!("{}/",p.directory) } else { format!("{}/{m}/",p.directory) };
                    input.sources.keys().any(|path| path.starts_with(&root)
                        && (path.ends_with(".go") || path.ends_with("/go.mod"))
                        && basic::language(input,path)!=Some("helm"))
                })
                .map(move |m| json!({"directory":p.directory,"module":m}))
        })
        .collect();
    let request = json!({"roots":roots,"goos":profile.goos,"goarch":profile.goarch,"tags":profile.tags,"tests":profile.tests,"workspace":profile.workspace});
    let mut out = Analysis {
        symbols: BTreeMap::new(),
        calls: vec![],
        packages: vec![],
        issues: input.issues.clone(),
        analyzer: String::new(),
        toolchain: String::new(),
        inputs: vec![],
        raw: None,
        services: services::Links::default(),
    };
    if !basic::has_go(input) {
        out.analyzer = "source-only".into();
        out.toolchain = "none".into();
        return Ok(out);
    }
    let raw = if let Some(facts) = cached {
        Ok(facts)
    } else {
        let tree = tempfile::tempdir()?;
        for pin in &input.pins {
            std::fs::create_dir(tree.path().join(&pin.directory))?;
        }
        for (path, source) in &input.sources {
            let target = tree.path().join(path);
            std::fs::create_dir_all(target.parent().unwrap())?;
            local::write_private(&target, source.content.bytes())?;
        }
        crate::phase::phase("review_native_facts", || {
            graphicle::extractors::go_types::review_encoded(tree.path(), &request)
        })
    };
    let raw = match raw {
        Ok(v) => v,
        Err(_) => {
            out.issues
                .push(issue("compiler_analysis_unavailable", Value::Null));
            return Ok(out);
        }
    };
    let facts: Facts = crate::phase::phase("review_decode_facts", || {
        graphicle::extractors::go_types::decode_review_facts(&raw)
    })
    .map_err(|_| Error::bad("Invalid bounded compiler facts"))?;
    if cacheable_facts(&facts) {
        out.raw = Some(raw);
    }
    out.analyzer = facts.analyzer;
    out.toolchain = facts.toolchain;
    for world in facts.worlds {
        for mut input in world.inputs {
            input["consumer_build"] = json!(world.root);
            out.inputs.push(input);
        }
        for reason in world.issues {
            out.issues.push(issue(&reason, json!(world.root)));
        }
        for symbol in world.symbols {
            let source = input
                .sources
                .get(&symbol.file)
                .ok_or_else(|| Error::bad("Compiler symbol outside pinned source"))?;
            if symbol.start >= symbol.end
                || symbol.body_start < symbol.start
                || symbol.body_start > symbol.end
                || source
                    .content
                    .text()
                    .and_then(|text| text.get(symbol.start..symbol.end))
                    .is_none_or(|v| text::sha256(v.as_bytes()) != symbol.revision)
            {
                return Err(Error::bad(
                    "Compiler declaration failed exact-span verification",
                ));
            }
            let id = key(&world.root, &symbol.key);
            if out
                .symbols
                .insert(id, (world.root.clone(), symbol))
                .is_some()
            {
                out.issues
                    .push(issue("duplicate_compiler_declaration", json!(world.root)));
            }
        }
        for mut package in world.packages {
            let files = package["files"]
                .as_array()
                .ok_or_else(|| Error::bad("Compiler package has no file inventory"))?;
            let selected: Vec<_> = files
                .iter()
                .map(|p| {
                    let path = p
                        .as_str()
                        .ok_or_else(|| Error::bad("Invalid package file"))?;
                    let source = input
                        .sources
                        .get(path)
                        .ok_or_else(|| Error::bad("Package file is not pinned"))?;
                    Ok(json!({"file":path,"repository_id":source.repository,"sha256":source.sha}))
                })
                .collect::<Result<_>>()?;
            package["selected_sources"] = json!(selected);
            package["consumer_build"] = json!(world.root);
            package["instance"] = json!(crate::digest(
                &json!({"package":package,"pins":input.pins,"profile":profile,"analyzer":out.analyzer,"toolchain":out.toolchain})
            ));
            out.packages.push(package);
        }
        for call in world.calls {
            if !call.reason.is_empty() {
                out.issues.push(issue("unresolved_edge",json!({"consumer_build":world.root,"file":call.file,"line":call.line,"spelling":call.spelling,"cause":call.reason})));
            }
            out.calls.push((world.root.clone(), call));
        }
    }
    // Package value declarations are now first-class alongside functions and
    // types. Keep a hard cardinality cap as well as the bounded fact transport.
    if out.symbols.len() > 200_000 {
        return Err(Error::bad(format!(
            "Review graph bound exceeded: {} symbols (limit 200000)", out.symbols.len()
        )));
    }
    // A byte-reuse hash is not semantic validity. This separate fingerprint
    // covers compiler-owned transitive package dependencies and native inputs.
    let config = crate::digest(
        &json!({"profile":profile,"analyzer":out.analyzer,"toolchain":out.toolchain,
        "roots":input.pins.iter().map(|p|json!([p.repository_id,p.directory,p.modules,p.policy_sha256])).collect::<Vec<_>>(),
        "configuration":input.sources.iter().filter(|(path,_)|matches!(Path::new(path).file_name().and_then(|n|n.to_str()),Some("go.mod"|"go.sum"|"go.work"|"go.work.sum"))).map(|(path,s)|json!([path,s.repository,s.sha])).collect::<Vec<_>>() }),
    );
    let nodes: BTreeMap<_, _> = out
        .packages
        .iter()
        .map(|p| {
            let mut value = p.clone();
            value.as_object_mut().unwrap().remove("instance");
            value["embedded"] = json!(
                out.inputs
                    .iter()
                    .filter(|i| i["consumer_build"] == p["consumer_build"]
                        && i["package"] == p["path"]
                        && i["role"] == "embed")
                    .map(|i| json!([i["file"], input.sources.get(s(i, "file")).map(|s| &s.sha)]))
                    .collect::<Vec<_>>()
            );
            (
                (s(p, "consumer_build").to_string(), s(p, "path").to_string()),
                crate::digest(&value),
            )
        })
        .collect();
    for package in &mut out.packages {
        let world = s(package, "consumer_build").to_string();
        let dependencies: Vec<_> = package["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| {
                nodes
                    .get(&(world.clone(), d.as_str()?.to_string()))
                    .map(|hash| json!([d, hash]))
            })
            .collect();
        package["content_instance"] = json!(crate::digest(
            &json!({"configuration":config,"package":nodes[&(world,s(package,"path").to_string())],"dependencies":dependencies})
        ));
    }
    Ok(out)
}

fn outside_declarations(analysis: &Analysis, file: &str, source: &str) -> String {
    let mut spans: Vec<_> = analysis
        .symbols
        .values()
        .filter(|(_, s)| s.file == file)
        .map(|(_, s)| (s.start, s.end))
        .collect();
    spans.sort();
    let mut cursor = 0;
    let mut outside = String::new();
    for (start, end) in spans {
        if start > cursor {
            outside.push_str(&source[cursor..start]);
        }
        cursor = cursor.max(end);
    }
    outside.push_str(&source[cursor..]);
    outside.trim().to_string()
}
// Repeated call sites share a traversal link, but remain in native facts for
// exact citations. Bound the structure we actually allocate, not raw references
// (including unresolved calls) that never become graph edges.
fn call_links(analysis: &Analysis, limit: usize) -> Result<Vec<(&str, &str, usize)>> {
    if analysis.services.edges.len() > limit {
        return Err(Error::bad(format!("Review graph bound exceeded: {} service links (limit {limit})", analysis.services.edges.len())));
    }
    // Declaration identities already exist. Re-hashing them for every endpoint
    // (and again for evidence) costs millions of digests on dependency graphs.
    let identities: BTreeMap<_, _> = analysis
        .symbols
        .iter()
        .map(|(id, (world, symbol))| ((world.as_str(), symbol.key.as_str()), id.as_str()))
        .collect();
    let mut links = BTreeMap::new();
    for (call_index, (world, call)) in analysis.calls.iter().enumerate() {
        if !call.reason.is_empty() {
            continue;
        }
        let (Some(&caller), Some(&callee)) = (
            identities.get(&(world.as_str(), call.caller.as_str())),
            identities.get(&(world.as_str(), call.target.as_str())),
        ) else {
            continue;
        };
        let old = links.entry((caller, callee, call.relation.as_str())).or_insert(call_index);
        // Prefer a discarded-error call as the representative when the same
        // caller invokes a supplier more than once. No target is discarded.
        if call.discarded_error && !analysis.calls[*old].1.discarded_error {
            *old = call_index;
        }
        if links.len() + analysis.services.edges.len() > limit {
            return Err(Error::bad(format!(
                "Review graph bound exceeded: {} distinct links (limit {limit}); {} call sites",
                links.len() + analysis.services.edges.len(), analysis.calls.len()
            )));
        }
    }
    Ok(links.into_iter().map(|((from, to, _), index)| (from, to, index)).collect())
}

fn call_evidence(world: &str, call: &Call) -> Value {
    json!({"relation":call.relation,"source_file":call.file,"source_location":format!("L{}",call.line),"byte_start":call.start,"byte_end":call.end,"spelling":call.spelling,"caller":key(world,&call.caller),"callee":key(world,&call.target),"discarded_error":call.discarded_error,"evidence":if !call.evidence.is_empty(){call.evidence.as_str()}else if call.relation=="may_call"{"function_value_candidate"}else{"go/types"}})
}

fn graph(analysis: &Analysis, reverse: bool) -> Result<Graph> {
    let links = call_links(analysis, 500_000)?;
    let mut graph = Graph::with_capacity(true, true, analysis.symbols.len(), links.len() + analysis.services.edges.len());
    for (id, (_, symbol)) in &analysis.symbols {
        graph.add_node(json!(id), json!({"label":symbol.name}).as_object().unwrap().clone())?;
    }
    for (caller, callee, call_index) in links {
        let call = &analysis.calls[call_index].1;
        let (mut from, mut to) = (caller, callee);
        if reverse {
            std::mem::swap(&mut from, &mut to)
        };
        // Traversal needs only the relation. Keep a reference to native evidence
        // and materialize it for selected paths, not for every edge in both graphs.
        graph.add_edge(
            json!(from),
            json!(to),
            None,
            json!({"relation":call.relation,"call_index":call_index})
                .as_object()
                .unwrap()
                .clone(),
        )?;
    }
    for (index, edge) in analysis.services.edges.iter().enumerate() {
        let (from, to) = if reverse { (&edge.to, &edge.from) } else { (&edge.from, &edge.to) };
        graph.add_edge(json!(from),json!(to),None,
            json!({"relation":edge.relation,"service_index":index}).as_object().unwrap().clone())?;
    }
    Ok(graph)
}

struct Selection {
    reasons: BTreeMap<String, Vec<Value>>,
    issues: Vec<Value>,
}
// One shortest explanation per direction is sufficient to explain inclusion.
// Changed symbols keep the dependency path from another seed, which connects
// flows across changed declarations. Its reverse consumer explanation would
// duplicate the same connecting evidence on the other endpoint.
fn add_reason(reasons: &mut Vec<Value>, reason: Value) {
    if reason["reason"] == "change_seed" {
        reasons.retain(|r| r["reason"] != "potential_consumer");
    } else if reason["reason"] == "potential_consumer"
        && reasons.iter().any(|r| r["reason"] == "change_seed")
    {
        return;
    }
    if let Some(old) = reasons.iter_mut().find(|r| r["reason"] == reason["reason"]) {
        let length = |r: &Value| r["path"].as_array().map_or(0, Vec::len);
        let discarded = |r: &Value| r["steps"].as_array().is_some_and(|steps| steps.iter().any(|s| s["discarded_error"] == true));
        if length(&reason) < length(old)
            || (length(&reason) == length(old) && (discarded(&reason) && !discarded(old)
                || discarded(&reason) == discarded(old) && reason.to_string().cmp(&old.to_string()).is_lt()))
        {
            *old = reason;
        }
    } else {
        reasons.push(reason);
    }
}

fn select(analysis: &Analysis, seeds: &[String], depth: usize) -> Result<Selection> {
    let mut out = Selection {
        reasons: BTreeMap::new(),
        issues: vec![],
    };
    if seeds.is_empty() {
        return Ok(out);
    }
    // Seed immediate failure-contract context before transitive traversal can
    // exhaust its budget. Interface implementations remain potential targets.
    if depth > 0 {
        let seed_set: BTreeSet<_> = seeds.iter().map(String::as_str).collect();
        let mut direct = 0;
        for (caller, callee, index) in call_links(analysis, 500_000)? {
            let (world, call) = &analysis.calls[index];
            if !seed_set.contains(caller) || !call.discarded_error
                || !matches!(call.relation.as_str(), "calls" | "may_call") { continue; }
            if direct == 20_000 {
                out.issues.push(issue("call_contract_budget", json!({"limit":20_000})));
                break;
            }
            add_reason(out.reasons.entry(callee.to_owned()).or_default(), json!({"reason":"dependency","path":[caller,callee],"steps":[call_evidence(world,call)]}));
            direct += 1;
        }
    }
    let relations = vec![
        "calls".into(),
        "may_call".into(),
        "uses_type".into(),
        "uses_value".into(),
        "references_callable".into(),
        "invokes_boundary".into(),
        "handles_boundary".into(),
        "http_calls".into(),
        "grpc_calls".into(),
        "event_delivers".into(),
        "potential_import".into(),
        "configuration_scope".into(),
    ];
    let mut paths_emitted = 0usize;
    for reverse in [false, true] {
        let graph = graph(analysis, reverse)?;
        for (seed_index, seed) in seeds.iter().enumerate() {
            if seed_index >= 1000 || paths_emitted >= 20_000 {
                out.issues.push(issue("traversal_budget",json!({"remaining_seeds":seeds.len()-seed_index,"direction":if reverse{"outgoing"}else{"incoming"}})));
                // All changed symbols still compete for packet space.
                for seed in seeds {
                    add_reason(
                        out.reasons.entry(seed.clone()).or_default(),
                        json!({"reason":"change_seed","path":[seed]}),
                    );
                }
                break;
            }
            add_reason(
                out.reasons.entry(seed.clone()).or_default(),
                json!({"reason":"change_seed","path":[seed]}),
            );
            let hits = affected::affected_nodes(&graph, seed, &relations, depth as i64)?;
            let mut paths = BTreeMap::from([(seed.clone(), vec![seed.clone()])]);
            let mut evidence_paths: BTreeMap<String, Vec<Value>> =
                BTreeMap::from([(seed.clone(), vec![])]);
            for hit in hits {
                paths_emitted += 1;
                if paths_emitted > 20_000 {
                    out.issues
                        .push(issue("traversal_budget", json!({"seed":seed})));
                    break;
                }
                let index = graph.node_index(&json!(hit.node_id)).unwrap();
                let predecessor = graph
                    .out_edges(index)
                    .into_iter()
                    .filter_map(|(_, target, edge)| {
                        let id = graph.nodes()[target].id.as_str().unwrap().to_string();
                        paths
                            .get(&id)
                            .is_some_and(|p| p.len() == hit.depth as usize)
                            .then(|| (id, serde_json::to_value(&edge.attributes).unwrap()))
                    })
                    .min_by(|a, b| {
                        a.0.cmp(&b.0)
                            .then_with(|| a.1.to_string().cmp(&b.1.to_string()))
                    });
                let Some((predecessor, reference)) = predecessor else {
                    out.issues
                        .push(issue("path_unavailable", json!(hit.node_id)));
                    continue;
                };
                let evidence = if let Some(index) = reference["service_index"].as_u64() {
                    let edge = &analysis.services.edges[index as usize];
                    json!({"relation":edge.relation,"caller":edge.from,"callee":edge.to,"evidence":"configured_static","provenance":edge.evidence})
                } else {
                    let (world, call) = &analysis.calls[reference["call_index"].as_u64().unwrap() as usize];
                    call_evidence(world, call)
                };
                let mut path = paths[&predecessor].clone();
                path.push(hit.node_id.clone());
                paths.insert(hit.node_id.clone(), path.clone());
                let mut steps = evidence_paths[&predecessor].clone();
                steps.push(evidence);
                evidence_paths.insert(hit.node_id.clone(), steps.clone());
                add_reason(
                    out.reasons.entry(hit.node_id).or_default(),
                    json!({"reason":if reverse{"dependency"}else{"potential_consumer"},"path":path,"steps":steps}),
                );
            }
            for (id, path) in &paths {
                if path.len() != depth + 1 {
                    continue;
                };
                let index = graph.node_index(&json!(id)).unwrap();
                if graph.in_edges(index).into_iter().any(|(source, _, _)| {
                    !paths.contains_key(graph.nodes()[source].id.as_str().unwrap())
                }) {
                    out.issues.push(issue("depth_limit",json!({"seed":seed,"at":id,"direction":if reverse{"outgoing"}else{"incoming"}})));
                }
            }
        }
    }
    Ok(out)
}

fn entry(
    side: &str,
    id: &str,
    symbol: &Symbol,
    source: &Source,
    reasons: &[Value],
    binding: &Value,
) -> Value {
    let identity = crate::digest(&json!([side, source.repository, id]));
    let mut value = json!({"id":identity,"side":side,"symbol_id":id,"repository_id":source.repository,"file":source.path,"file_sha256":source.sha,
        "name":symbol.name,"kind":symbol.kind,"parent":symbol.parent,"package":symbol.package,"package_binding":binding,"byte_start":symbol.start,"byte_end":symbol.end,"line":symbol.line,"end_line":symbol.end_line,
        "signature_revision":symbol.signature,"implementation_revision":symbol.implementation,"text":&source.content.text().expect("verified compiler span")[symbol.start..symbol.end],"reasons":reasons,"test":source.path.ends_with("_test.go")});
    value["text_sha256"] = json!(text::sha256(s(&value, "text").as_bytes()));
    value["revision"] = json!(crate::digest(&value));
    value
}

// go list's Deps is the transitive package inventory for this exact build.
// A package-local unknown can affect its consumers, but cannot flow backwards
// into unrelated suppliers. Unknown build configuration still uses full scope.
fn package_scope(analysis: &Analysis, changed: &BTreeSet<(String, String)>) -> BTreeSet<String> {
    let affected: BTreeSet<_> = analysis
        .packages
        .iter()
        .filter(|p| {
            let world = s(p, "consumer_build");
            changed.contains(&(world.to_owned(), s(p, "path").to_owned()))
                || p["dependencies"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|dependency| {
                        dependency.as_str().is_some_and(|name| {
                            changed.contains(&(world.to_owned(), name.to_owned()))
                        })
                    })
        })
        .map(|p| (s(p, "consumer_build").to_owned(), s(p, "path").to_owned()))
        .collect();
    analysis
        .symbols
        .iter()
        .filter(|(_, (world, symbol))| affected.contains(&(world.clone(), symbol.package.clone())))
        .map(|(id, _)| id.clone())
        .collect()
}

fn compare(base: &Input, head: &Input, a: &Analysis, b: &Analysis, r: &Request) -> Result<Value> {
    let mut changes = vec![];
    let mut seeds_a = BTreeSet::new();
    let mut seeds_b = BTreeSet::new();
    let mut unmatched_a = BTreeSet::new();
    let mut unmatched_b = BTreeSet::new();
    for (id, (_, old)) in &a.symbols {
        if let Some((_, new)) = b.symbols.get(id) {
            if old.revision != new.revision {
                if old.kind == "basic_file" || new.kind == "basic_file" {
                    changes.push(json!({"kind":"modified","base":id,"head":id,"granularity":"file","signature_changed":null,"implementation_changed":null}));
                } else {
                    changes.push(json!({"kind":"modified","base":id,"head":id,"signature_changed":old.signature!=new.signature,"implementation_changed":old.implementation!=new.implementation}));
                }
                seeds_a.insert(id.clone());
                seeds_b.insert(id.clone());
            }
        } else {
            unmatched_a.insert(id.clone());
        }
    }
    for id in b.symbols.keys() {
        if !a.symbols.contains_key(id) {
            unmatched_b.insert(id.clone());
        }
    }
    let mut correspondence = vec![];
    for id in unmatched_a.clone() {
        let (world, old) = &a.symbols[&id];
        let repo = &base.sources[&old.file].repository;
        let candidates: Vec<_> = unmatched_b
            .iter()
            .filter(|next| {
                let (new_world, new) = &b.symbols[*next];
                world == new_world
                    && repo == &head.sources[&new.file].repository
                    && old.kind == new.kind
                    && old.implementation == new.implementation
                    && old.correspondence == new.correspondence
            })
            .cloned()
            .collect();
        if candidates.len() == 1 {
            let next = &candidates[0];
            let (_, new) = &b.symbols[next];
            let reverse = unmatched_a
                .iter()
                .filter(|other| {
                    let (w, o) = &a.symbols[*other];
                    w == world
                        && base.sources[&o.file].repository == *repo
                        && o.kind == new.kind
                        && o.implementation == new.implementation
                        && o.correspondence == new.correspondence
                })
                .count();
            if reverse == 1 {
                unmatched_a.remove(&id);
                unmatched_b.remove(next);
                changes.push(json!({"kind":"moved_or_renamed","base":id,"head":next,"evidence":if old.kind=="basic_file"{"unique_file_content"}else{"unique_implementation_and_signature"}}));
                seeds_a.insert(id);
                seeds_b.insert(next.clone());
                continue;
            }
        }
        if !candidates.is_empty() {
            correspondence.push(json!({"base":id,"candidates":candidates,"status":"ambiguous"}));
        }
    }
    for id in unmatched_a {
        changes.push(json!({"kind":"deleted","base":id}));
        seeds_a.insert(id);
    }
    for id in unmatched_b {
        changes.push(json!({"kind":"added","head":id}));
        seeds_b.insert(id);
    }
    let files: BTreeSet<_> = base
        .sources
        .keys()
        .chain(head.sources.keys())
        .cloned()
        .collect();
    let mut diffs = vec![];
    let mut file_changes = vec![];
    let mut fallback_a = BTreeSet::new();
    let mut fallback_b = BTreeSet::new();
    let mut scope_issues = vec![];
    let diff_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    for file in files {
        let old = base.sources.get(&file);
        let new = head.sources.get(&file);
        if old.map(|s| &s.sha) == new.map(|s| &s.sha) {
            continue;
        }
        let before = old.and_then(|s| s.content.text()).unwrap_or("");
        let after = new.and_then(|s| s.content.text()).unwrap_or("");
        let binary = old
            .into_iter()
            .chain(new)
            .any(|s| s.content.text().is_none());
        let line_offsets = |source: &str| {
            let mut offsets = vec![0];
            for line in source.split_inclusive('\n') {
                offsets.push(offsets.last().unwrap() + line.len());
            }
            offsets
        };
        let offsets_a = line_offsets(before);
        let offsets_b = line_offsets(after);
        let diff = similar::TextDiff::configure()
            .deadline(diff_deadline)
            .diff_lines(before, after);
        for (hunk, ops) in diff
            .grouped_ops(3)
            .into_iter()
            .enumerate()
            .filter(|_| !binary)
        {
            let first = ops.first().unwrap();
            let last = ops.last().unwrap();
            let range_a = offsets_a[first.old_range().start]..offsets_a[last.old_range().end];
            let range_b = offsets_b[first.new_range().start]..offsets_b[last.new_range().end];
            diffs.push(json!({"file":file,"hunk":hunk,"base_sha256":old.map(|s|&s.sha),"head_sha256":new.map(|s|&s.sha),"base_bytes":[range_a.start,range_a.end],"head_bytes":[range_b.start,range_b.end],"before":&before[range_a],"after":&after[range_b]}));
        }
        let provenance: Vec<_> = a
            .inputs
            .iter()
            .chain(&b.inputs)
            .filter(|p| p["file"] == file)
            .collect();
        let selected = provenance.iter().any(|p| p["role"] == "selected");
        let embedded = provenance.iter().any(|p| p["role"] == "embed");
        let excluded = !provenance.is_empty()
            && provenance
                .iter()
                .all(|p| matches!(s(p, "role"), "excluded_test" | "excluded_by_profile"));
        let declaration_only = selected
            && outside_declarations(a, &file, before) == outside_declarations(b, &file, after);
        let documentation = matches!(
            Path::new(&file).extension().and_then(|e| e.to_str()),
            Some("md" | "rst" | "txt")
        ) || Path::new(&file).file_name().is_some_and(|n| n == "LICENSE");
        let classification = if embedded {
            "embedded_input"
        } else if excluded {
            "excluded_by_profile"
        } else if declaration_only {
            "declaration_change"
        } else if documentation {
            "non_compiler_input"
        } else {
            "uncertain_scope"
        };
        file_changes.push(json!({"file":file,"classification":classification,"binary":binary,"base_sha256":old.map(|s|&s.sha),"head_sha256":new.map(|s|&s.sha)}));
        if embedded || (!excluded && !declaration_only && !documentation) {
            scope_issues.push(issue(
                "uncertain_file_scope",
                json!({"file":file,"classification":classification,"binary":binary,"base_sha256":old.map(|s|&s.sha),"head_sha256":new.map(|s|&s.sha)}),
            ));
            for (input, analysis, fallback, source) in [
                (base, a, &mut fallback_a, old),
                (head, b, &mut fallback_b, new),
            ] {
                let Some(source) = source else { continue };
                let changed_packages: BTreeSet<_> = analysis
                    .inputs
                    .iter()
                    .filter(|p| p["file"] == file && matches!(s(p, "role"), "selected" | "embed"))
                    .map(|p| {
                        (
                            s(p, "consumer_build").to_owned(),
                            s(p, "package").to_owned(),
                        )
                    })
                    .collect();
                if !changed_packages.is_empty() {
                    fallback.extend(package_scope(analysis, &changed_packages));
                    continue;
                }
                let worlds: BTreeSet<_> = analysis
                    .packages
                    .iter()
                    .filter(|p| {
                        p["selected_sources"].as_array().is_some_and(|files| {
                            files
                                .iter()
                                .any(|f| f["repository_id"] == source.repository)
                        })
                    })
                    .map(|p| s(p, "consumer_build"))
                    .collect();
                for (id, (world, symbol)) in &analysis.symbols {
                    if input.sources[&symbol.file].repository == source.repository
                        || worlds.contains(world.as_str())
                    {
                        fallback.insert(id.clone());
                    }
                }
            }
        }
    }
    // A native resolution input change can affect unchanged callers. Compare
    // package bindings without source hashes; source changes are seeded above.
    let bindings = |analysis: &Analysis| -> BTreeMap<(String, String), Value> {
        analysis.packages.iter().map(|p|((s(p,"consumer_build").to_owned(),s(p,"path").to_owned()),
            json!({"directory":p["directory"],"module":p["module"],"version":p["version"],"replacement":p["replacement"],"imports":p["imports"]}))).collect()
    };
    let bindings_a = bindings(a);
    let bindings_b = bindings(b);
    let changed_bindings: BTreeSet<_> = bindings_a
        .keys()
        .chain(bindings_b.keys())
        .filter(|key| bindings_a.get(*key) != bindings_b.get(*key))
        .cloned()
        .collect();
    let layout_changed = base
        .pins
        .iter()
        .map(|p| (&p.repository_id, &p.directory, &p.modules, &p.policy_sha256))
        .collect::<Vec<_>>()
        != head
            .pins
            .iter()
            .map(|p| (&p.repository_id, &p.directory, &p.modules, &p.policy_sha256))
            .collect::<Vec<_>>();
    let dependency_changed = !changed_bindings.is_empty() || layout_changed;
    if layout_changed {
        fallback_a.extend(a.symbols.keys().cloned());
        fallback_b.extend(b.symbols.keys().cloned());
    } else if dependency_changed {
        fallback_a.extend(package_scope(a, &changed_bindings));
        fallback_b.extend(package_scope(b, &changed_bindings));
    }
    let select_scope =
        |analysis, seeds: &BTreeSet<String>, fallback: &BTreeSet<String>| -> Result<Selection> {
            let mut selected = select(
                analysis,
                &seeds.iter().cloned().collect::<Vec<_>>(),
                r.max_depth,
            )?;
            // An unchanged initializer can call a changed function. Include
            // package consumers for that path as well as directly edited init.
            let initializing: BTreeSet<_> = selected
                .reasons
                .iter()
                .filter_map(|(id, reasons)| {
                    let (world, symbol) = &analysis.symbols[id];
                    ((symbol.kind == "variable" || symbol.name.ends_with(".init"))
                        && reasons.iter().any(|r| {
                            r["reason"] == "change_seed" || r["reason"] == "potential_consumer"
                        }))
                    .then(|| (world.clone(), symbol.package.clone()))
                })
                .collect();
            let initialized_scope = package_scope(analysis, &initializing);
            if !initializing.is_empty() {
                selected
                    .issues
                    .push(issue("package_initialization_scope", json!(initializing)));
            }
            // Package scope already includes all transitive potential consumers.
            // It is a conservative context candidate, not evidence of a call from
            // every function into every supplier reachable from that package.
            for id in fallback.iter().chain(&initialized_scope) {
                selected.reasons.entry(id.clone()).or_insert_with(|| {
                    vec![json!({"reason":"conservative_scope","path":[id],"uncertain_scope":true})]
                });
            }
            Ok(selected)
        };
    let selected_a = select_scope(a, &seeds_a, &fallback_a)?;
    let selected_b = select_scope(b, &seeds_b, &fallback_b)?;
    let mut issues = a.issues.clone();
    issues.extend(b.issues.clone());
    issues.extend(scope_issues);
    issues.extend(selected_a.issues);
    issues.extend(selected_b.issues);
    if !correspondence.is_empty() {
        issues.push(issue(
            "ambiguous_correspondence",
            json!(correspondence.len()),
        ));
    }
    let mut entries = vec![];
    for (side, input, analysis, selection) in [
        ("base", base, a, selected_a.reasons),
        ("head", head, b, selected_b.reasons),
    ] {
        let bindings:BTreeMap<_,_>=analysis.packages.iter().map(|p|((s(p,"consumer_build"),s(p,"path")),json!({"instance":p["instance"],"content_instance":p["content_instance"],"consumer_build":p["consumer_build"],"module":p["module"],"version":p["version"],"replacement":p["replacement"]}))).collect();
        for (id, reasons) in selection {
            let (world, symbol) = &analysis.symbols[&id];
            let mut value = entry(
                side,
                &id,
                symbol,
                &input.sources[&symbol.file],
                &reasons,
                bindings
                    .get(&(world.as_str(), symbol.package.as_str()))
                    .unwrap_or(&Value::Null),
            );
            if symbol.kind == "basic_file" {
                basic::annotate(&mut value, analysis, &symbol.file);
            }
            if let Some(boundary) = analysis.services.nodes.get(&id) {
                value["service_boundary"] = boundary.clone();
                value["revision"] = json!(crate::digest(&value));
            }
            entries.push(value);
        }
    }
    flows::annotate(&mut entries, base, head);
    entries.sort_by_key(|e| {
        (
            !e["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == "change_seed"),
            s(e, "side") != "head",
            s(e, "id").to_string(),
        )
    });
    for (side, input) in [("base", base), ("head", head)] {
        for (path, source) in &input.sources {
            let analysis = if side == "base" { a } else { b };
            if source.content.text().is_none()
                && !analysis
                    .inputs
                    .iter()
                    .any(|i| i["file"] == *path && i["role"] == "embed")
            {
                issues.push(issue(
                    "unmodeled_binary_input",
                    json!({"side":side,"file":path}),
                ));
            }
            if basic::language(input, path).is_none() && unsupported_source(path, source.content.text().unwrap_or("")) {
                issues.push(issue(
                    "unsupported_source_language_or_contract",
                    json!({"side":side,"file":path}),
                ));
            }
        }
    }
    let complete = issues.is_empty();
    Ok(
        json!({"format":FORMAT,"base":base.pins,"head":head.pins,"profile":r.profile,"analyzer":a.analyzer,"toolchain":{"base":a.toolchain,"head":b.toolchain},"changes":changes,"correspondence":correspondence,"dependency_binding_changed":dependency_changed,
        "diffs":diffs,"entries":entries,"file_changes":file_changes,"package_instances":{"base":a.packages,"head":b.packages},"service_graph":{"base":a.services.summary(),"head":b.services.summary()},"coverage":{"complete":complete,"scope":"supplied snapshot set; Go compiler facts and basic Terraform/Helm/Bash/Python/Rust source dependencies","behavioral_propagation_proven":false,"path_selection":"shortest_example_per_direction","issues":issues,"max_depth":r.max_depth},
        "impact":if !complete{"incomplete"}else if seeds_a.is_empty()&&seeds_b.is_empty()&&diffs.is_empty()&&!dependency_changed{"no_changes_in_scope"}else{"potential_impact"}}),
    )
}

fn unsupported_source(path: &str, content: &str) -> bool {
    let path_ref = Path::new(path);
    let extension = path_ref.extension().and_then(|e| e.to_str()).unwrap_or("");
    extension != "go"
        && (graphicle::analyze::file_category(path) == "code"
            || matches!(extension, "proto" | "s" | "S")
            || content.starts_with("#!")
            || matches!(
                path_ref.file_name().and_then(|n| n.to_str()),
                Some("Makefile" | "Dockerfile")
            ))
}

/// Optional candidates are costed only against the remaining allowance. A
/// failed bounded count is a lower bound, never an estimated exact cost.
#[derive(Default)]
struct PackingCost {
    exact: Option<usize>,
    exceeds: usize,
}
impl PackingCost {
    fn measured(entry: &Value) -> Self {
        Self { exact: Some(text::count(&entry.to_string()) + text::count(&entry["id"].to_string()) + 24), exceeds: 0 }
    }
    fn exact(&mut self, entry: &Value) -> usize {
        *self.exact.get_or_insert_with(|| text::count(&entry.to_string()) + text::count(&entry["id"].to_string()) + 24)
    }
    fn within(&mut self, entry: &Value, limit: usize) -> Option<usize> {
        if let Some(cost) = self.exact { return (cost <= limit).then_some(cost); }
        if limit <= self.exceeds { return None; }
        let overhead = text::count(&entry["id"].to_string()) + 24;
        if let Some(remaining) = limit.checked_sub(overhead)
            && let Some(cost) = text::count_within(&entry.to_string(), remaining)
        {
            self.exact = Some(cost + overhead);
            return self.exact;
        }
        self.exceeds = limit;
        None
    }
}

// Changed declarations already explain their inclusion. Additional dependency
// examples and report examples are optional evidence, not required source bytes.
// Compact only under pressure; retain counts/digests and every uncertainty flag.
fn compact_required_metadata(report: &mut Value, candidates: &mut [(&str, Value, PackingCost)]) {
    for field in ["changes", "file_changes"] {
        if let Some(examples) = report[field]["examples"].as_array_mut() {
            examples.clear(); // count and digest cover the full original inventory
        }
    }
    for (_, entry, cost) in candidates.iter_mut().filter(|(_, e, _)| is_head_change(e)) {
        let Some(reasons) = entry["reasons"].as_array() else { continue };
        let retained: Vec<_> = reasons.iter().filter(|r| r["reason"] == "change_seed" || r["uncertain_scope"] == true).cloned().collect();
        if retained.len() == reasons.len() { continue; }
        entry["reason_examples"] = json!({"count":reasons.len(),"omitted":reasons.len()-retained.len(),"digest":crate::digest(&json!(reasons))});
        entry["reasons"] = json!(retained);
        entry.as_object_mut().unwrap().remove("revision");
        entry["revision"] = json!(crate::digest(entry));
        *cost = PackingCost::measured(entry);
    }
}

fn pack(
    mut report: Value,
    max_tokens: usize,
    previous: Option<&Value>,
    scopes: Option<&reuse::Scopes>,
) -> Result<(Value, Value)> {
    let mut entries = std::mem::take(report["entries"].as_array_mut().unwrap());
    // Optional annotations are upgrades to code, never part of the cost that
    // decides whether a required current declaration fits.
    let mut business = BTreeMap::new();
    let mut flow_annotations = BTreeMap::new();
    for entry in &mut entries {
        let context = entry.as_object_mut().unwrap().remove("business_context");
        let flow = entry
            .as_object_mut()
            .unwrap()
            .remove("_execution_flow_reasons");
        if context.is_some() || flow.is_some() {
            entry.as_object_mut().unwrap().remove("revision");
            entry["revision"] = json!(crate::digest(entry));
        }
        if let Some(context) = context {
            business.insert(s(entry, "id").to_owned(), context);
        }
        if let Some(flow) = flow {
            flow_annotations.insert(s(entry, "id").to_owned(), flow);
        }
    }
    if !business.is_empty() {
        report["business_context"]["packing"] = json!({"policy":"required_code_then_history_then_cited_flows_v2",
            "candidate_entries":business.len(),"retained_entries":0,"omitted_entries":business.len(),"allowance_tokens":0,"used_tokens":0});
    }
    let mut diffs = std::mem::take(report["diffs"].as_array_mut().unwrap());
    let instances = report["package_instances"].take();
    report["package_instances"] = json!({"digest":crate::digest(&instances),"base_count":instances["base"].as_array().unwrap().len(),"head_count":instances["head"].as_array().unwrap().len()});
    for diff in &mut diffs {
        diff["id"] = json!(crate::digest(&json!(["diff", diff["file"], diff["hunk"]])));
        diff["revision"] = json!(crate::digest(diff));
    }
    let scope_requirements = scopes.map(|s| {
        s.counts(
            entries.iter().map(|e| ("entries", e))
                .chain(diffs.iter().map(|e| ("diffs", e))),
        )
    });
    let required_head = entries.iter().filter(|e| is_head_change(e)).count();
    let required_contracts = entries.iter().filter(|e| is_call_contract(e)).count();
    report["coverage"]["required_entries"] = json!(entries.len());
    report["coverage"]["required_diffs"] = json!(diffs.len());
    let total = entries.len() + diffs.len();
    let issues = std::mem::take(report["coverage"]["issues"].as_array_mut().unwrap());
    report["coverage"]["issue_count"] = json!(issues.len());
    report["coverage"]["issues"] = json!(issues.iter().take(20).collect::<Vec<_>>());
    let changes = report["changes"].take();
    let deleted: BTreeSet<_> = changes
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "deleted")
        .map(|c| s(c, "base"))
        .collect();
    report["changes"] = json!({"count":changes.as_array().unwrap().len(),"digest":crate::digest(&changes),"examples":changes.as_array().unwrap().iter().take(20).collect::<Vec<_>>()});
    if let Some(files) = report["file_changes"].as_array() {
        let files = files.clone();
        report["file_changes"] = json!({"count":files.len(),"digest":crate::digest(&json!(files)),"examples":files.iter().take(40).collect::<Vec<_>>()});
    }
    report["entries"] = json!([]);
    report["diffs"] = json!([]);
    report["context_order"] = json!([]);
    report["coverage"]["budget_omitted"] = json!(0);
    report["coverage"]["current_head"] =
        json!({"required":required_head,"selected":0,"usable":required_head==0});
    report["coverage"]["call_contracts"] = json!({"required":required_contracts,"selected":0,"complete":required_contracts==0,"scope":"immediate pinned targets of discarded-error calls; may_call targets are candidates"});
    // The canonical full selection is independent of the prior receipt. A delta
    // describes this same bounded view; it cannot accumulate an unbounded view
    // of cheap, previously shown entries. Round-robin gives base/deletions and
    // consumers a turn alongside current changed declarations.
    let mut buckets: [std::collections::VecDeque<Value>; 8] =
        std::array::from_fn(|_| std::collections::VecDeque::new());
    let mut contracts = vec![];
    for entry in entries {
        if is_call_contract(&entry) {
            contracts.push(entry);
            continue;
        }
        let reasons = entry["reasons"].as_array();
        let seed = reasons.is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"));
        let consumer = reasons.is_some_and(|rs| {
            rs.iter()
                .any(|r| r["reason"] == "potential_consumer" && r["uncertain_scope"] != true)
        });
        let fallback = reasons.is_some_and(|rs| rs.iter().all(|r| r["uncertain_scope"] == true));
        let head = entry["side"] == "head";
        let bucket = if head && seed {
            0
        } else if deleted.contains(s(&entry, "symbol_id")) {
            1
        } else if head && consumer {
            2
        } else if seed {
            3
        } else if fallback {
            if head { 6 } else { 7 }
        } else if head {
            4
        } else {
            5
        };
        buckets[bucket].push_back(entry);
    }
    let mut candidates = vec![];
    // Current declarations are the review's primary evidence. A large old
    // declaration must not consume their space merely because it got an early
    // round-robin turn. Reserve a soft hunk allowance before filling context.
    candidates.extend(buckets[0].drain(..).map(|e| ("entries", e)));
    candidates.extend(contracts.into_iter().map(|e| ("entries", e)));
    candidates.extend(diffs.into_iter().map(|d| ("diffs", d)));
    while buckets[1..6].iter().any(|b| !b.is_empty()) {
        for bucket in &mut buckets[1..6] {
            if let Some(entry) = bucket.pop_front() {
                candidates.push(("entries", entry));
            }
        }
    }
    while buckets[6..].iter().any(|b| !b.is_empty()) {
        for bucket in &mut buckets[6..] {
            if let Some(entry) = bucket.pop_front() {
                candidates.push(("entries", entry));
            }
        }
    }
    let mut candidates: Vec<_> = candidates
        .into_iter()
        .map(|(collection, entry)| {
            let cost = if collection == "diffs" || is_head_change(&entry) || is_call_contract(&entry) {
                PackingCost::measured(&entry)
            } else { PackingCost::default() };
            (collection, entry, cost)
        })
        .collect();
    report["execution_flows"] = json!({"paths":[],"coverage":{"complete":false,"observed_execution_available":false,"selection":"selected_invocation_segments","omitted":0,"max_paths":16}});
    let required_cost = |entries: &[(&str, Value, PackingCost)]| entries.iter().filter(|(_, e, _)| is_head_change(e) || is_call_contract(e)).map(|(_, _, cost)| cost.exact.unwrap()).sum::<usize>();
    let metadata_cost = text::count(&report.to_string()) + 384;
    // Reserve room for whole history blocks when requested, before spending
    // tokens on repeated change/path examples. This never removes source text.
    let history_reserve = if business.is_empty() { 0 } else { max_tokens / 8 };
    if metadata_cost + required_cost(&candidates) + history_reserve > max_tokens {
        compact_required_metadata(&mut report, &mut candidates);
    }
    let mut used = text::count(&report.to_string()) + 384;
    let requested_diff_reserve = candidates.iter().filter(|(c, _, _)| *c == "diffs").map(|(_, _, cost)| cost.exact.unwrap()).sum::<usize>().min(max_tokens / 4);
    let head_cost = candidates.iter().filter(|(_, e, _)| is_head_change(e)).map(|(_, _, cost)| cost.exact.unwrap()).sum::<usize>();
    let contract_cost = candidates.iter().filter(|(_, e, _)| is_call_contract(e)).map(|(_, _, cost)| cost.exact.unwrap()).sum::<usize>();
    let optional_capacity = max_tokens.saturating_sub(used).saturating_sub(head_cost).saturating_sub(contract_cost);
    // Required declarations and call contracts are reserved first. Preserve an
    // affordable complete hunk before spending optional capacity on history.
    // A fixed half-budget split needlessly excludes whole history blocks; no
    // reservation is useful when even the smallest hunk cannot fit.
    let minimum_hunk = candidates.iter().filter(|(c, _, _)| *c == "diffs")
        .map(|(_, _, cost)| cost.exact.unwrap())
        .filter(|cost| *cost <= optional_capacity && *cost <= requested_diff_reserve)
        .min().unwrap_or(0);
    let business_allowance = optional_capacity.saturating_sub(minimum_hunk).min(max_tokens / 4);
    let business_costs = fit_business_context(&mut candidates, &business, business_allowance, true);
    let business_used: usize = business_costs.values().sum();
    let diff_reserve = requested_diff_reserve.min(optional_capacity.saturating_sub(business_used));
    let minimum_head_tokens = used + head_cost + diff_reserve;
    let flow_reserve = if !flow_annotations.is_empty()
        || candidates.iter().any(|(_, entry, _)| {
            entry["reasons"].as_array().is_some_and(|reasons| {
                reasons
                    .iter()
                    .any(|reason| reason.get("execution_flow").is_some())
            })
        }) {
        max_tokens
            .saturating_sub(minimum_head_tokens)
            .saturating_sub(contract_cost)
            .saturating_sub(business_used)
            .min(max_tokens / 16)
    } else {
        0
    };
    report["execution_flows"]["coverage"]["reserved_tokens"] = json!(flow_reserve);
    if !business.is_empty() {
        report["business_context"]["packing"]["allowance_tokens"] = json!(business_allowance);
        report["business_context"]["packing"]["used_tokens"] = json!(business_used);
    }
    let mut selected = vec![];
    let mut diff_used = 0;
    let mut deferred = vec![];
    for (collection, entry, mut cost) in candidates {
        let limit = if is_head_change(&entry) {
            max_tokens.saturating_sub(diff_reserve)
        } else {
            max_tokens.saturating_sub(flow_reserve)
        };
        let allowance = limit.saturating_sub(used).min(if collection == "diffs" {
            diff_reserve.saturating_sub(diff_used)
        } else { usize::MAX });
        let Some(measured) = cost.within(&entry, allowance) else {
            deferred.push((collection, entry, cost));
            continue;
        };
        let cost = measured;
        used += cost;
        if collection == "diffs" {
            diff_used += cost;
        }
        selected.push((collection, entry));
    }
    // Reclaim unused reservations in priority order, without re-tokenizing.
    for (collection, entry, mut cost) in deferred {
        let limit = if is_head_change(&entry) {
            max_tokens
        } else {
            max_tokens.saturating_sub(flow_reserve)
        };
        if let Some(cost) = cost.within(&entry, limit.saturating_sub(used)) {
            used += cost;
            selected.push((collection, entry));
        }
    }
    let head_selected = selected
        .iter()
        .filter(|(c, e)| *c == "entries" && is_head_change(e))
        .count();
    report["coverage"]["current_head"] = json!({"required":required_head,"selected":head_selected,"complete":required_head==head_selected,"minimum_tokens_with_hunk_reserve":minimum_head_tokens,"usable":required_head==0 || head_selected>0});
    let contracts_selected = selected.iter().filter(|(_, e)| is_call_contract(e)).count();
    report["coverage"]["call_contracts"]["selected"] = json!(contracts_selected);
    report["coverage"]["call_contracts"]["complete"] = json!(contracts_selected == required_contracts);
    report["coverage"]["budget_omitted"] = json!(total - selected.len());
    if total != selected.len() {
        report["coverage"]["complete"] = json!(false);
        report["impact"] = json!("incomplete");
    }
    if let (Some(scopes), Some(required)) = (scopes, scope_requirements.as_ref()) {
        scopes.finish(required, &mut selected, &mut report);
    }
    let flow_used = flows::pack(
        &mut report,
        &mut selected,
        &flow_annotations,
        max_tokens.saturating_sub(used).min(flow_reserve),
    );
    // Reclaim reservations that could not fit a complete cited path. Upgrade
    // selected code only; never spend a history allowance on omitted entries.
    if !business.is_empty() {
        let retained_business: usize = selected
            .iter()
            .filter_map(|(_, e)| business_costs.get(s(e, "id")))
            .sum();
        let refill_allowance = max_tokens
            .saturating_sub(used)
            .saturating_sub(flow_used)
            .min((max_tokens / 4).saturating_sub(retained_business));
        let refilled = if refill_allowance > 0 {
            let mut upgrades: Vec<_> = selected
                .into_iter()
                .map(|(collection, entry)| {
                    let cost = PackingCost::measured(&entry);
                    (collection, entry, cost)
                })
                .collect();
            let refilled: usize = fit_business_context(&mut upgrades, &business, refill_allowance, false)
                .values()
                .sum();
            selected = upgrades
                .into_iter()
                .map(|(collection, entry, _)| (collection, entry))
                .collect();
            refilled
        } else {
            0
        };
        let retained = selected
            .iter()
            .filter(|(_, e)| e.get("business_context").is_some())
            .count();
        report["business_context"]["packing"]["retained_entries"] = json!(retained);
        report["business_context"]["packing"]["omitted_entries"] = json!(business.len() - retained);
        report["business_context"]["packing"]["refill_tokens"] = json!(refilled);
        report["business_context"]["packing"]["used_tokens"] = json!(retained_business + refilled);
    }
    report["context_order"] = json!(
        selected
            .iter()
            .map(|(_, e)| e["id"].clone())
            .collect::<Vec<_>>()
    );
    let old: BTreeMap<_, _> = previous
        .and_then(|p| p["entries"].as_array())
        .into_iter()
        .flatten()
        .map(|v| (s(v, "id"), v))
        .collect();
    let mut retained = BTreeSet::new();
    let mut unchanged = vec![];
    let mut receipts = vec![];
    for (collection, mut entry) in selected {
        let id = s(&entry, "id").to_string();
        let revision = entry["revision"].clone();
        let body = entry["text_sha256"].clone();
        receipts.push(json!({"id":id,"revision":revision,"collection":collection,"repository_id":entry["repository_id"],"text_sha256":body,"side":entry["side"],"symbol_id":entry["symbol_id"],"file":entry["file"],"file_sha256":entry["file_sha256"],"content_instance":entry["package_binding"]["content_instance"],"review_basis":entry["review_basis"]}));
        retained.insert(id.clone());
        if old
            .get(id.as_str())
            .is_some_and(|e| e["revision"] == revision)
        {
            unchanged.push(id);
            continue;
        }
        entry["update"] = json!(if old.contains_key(id.as_str()) {
            "updated"
        } else {
            "added"
        });
        if report["text_reuse"] == true
            && collection == "entries"
            && !body.is_null()
            && old
                .get(id.as_str())
                .is_some_and(|e| e["text_sha256"] == body)
        {
            entry.as_object_mut().unwrap().remove("text");
            // The packet names the prior receipt; this entry already carries
            // its ID and body hash. Do not retransmit those identifiers.
            entry["text_ref"] = json!(true);
        }
        report[collection].as_array_mut().unwrap().push(entry);
    }
    report["previous_packet_id"] = previous
        .map(|p| p["packet_id"].clone())
        .unwrap_or(Value::Null);
    report["unchanged"] = json!(unchanged);
    report["invalidated"] = json!(old.values().filter(|e|!retained.contains(s(e,"id"))).map(|e|json!({"id":e["id"],"revision":e["revision"],"collection":e["collection"],"reason":"changed_removed_or_omitted"})).collect::<Vec<_>>());
    report["packet_id"] = json!(crate::digest(&report));
    report["tokens"] = json!(text::count(&report.to_string()) + 8);
    if text::count(&report.to_string()) > max_tokens {
        return Err(Error::bad(
            "Token budget too small for canonical context and delta controls; increase it or request a full packet",
        ));
    }
    let receipt = json!({"packet_id":report["packet_id"],"entries":receipts,"base":report["base"],"head":report["head"],"profile":report["profile"],"coverage_complete":report["coverage"]["complete"],"review_context_sha256":report["review_reuse"]["context_sha256"]});
    Ok((report, receipt))
}

fn is_call_contract(entry: &Value) -> bool {
    entry["side"] == "head" && !is_head_change(entry) && entry["reasons"].as_array().is_some_and(|reasons| {
        reasons.iter().any(|r| r["reason"] == "dependency"
            && r["path"].as_array().is_some_and(|p| p.len() == 2)
            && r["steps"].as_array().is_some_and(|steps| steps.iter().any(|s| s["discarded_error"] == true && matches!(s["relation"].as_str(), Some("calls" | "may_call")))))
    })
}

fn is_head_change(entry: &Value) -> bool {
    entry["side"] == "head"
        && entry["reasons"]
            .as_array()
            .is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"))
}

fn fit_business_context(
    candidates: &mut [(&str, Value, PackingCost)],
    contexts: &BTreeMap<String, Value>,
    allowance: usize,
    required_only: bool,
) -> BTreeMap<String, usize> {
    let analyzed = |context: &Value| {
        ["function_history", "cochange", "similar_changes"]
            .iter()
            .any(|field| {
                context["evolution"][field]
                    .as_array()
                    .is_some_and(|v| !v.is_empty())
            })
    };
    let mut order: Vec<_> = (0..candidates.len())
        .filter(|i| {
            candidates[*i].1.get("business_context").is_none()
                && contexts.contains_key(s(&candidates[*i].1, "id"))
                && (!required_only || is_head_change(&candidates[*i].1) || is_call_contract(&candidates[*i].1))
        })
        .collect();
    // Within the same evidence/side priority, prefer affordable complete
    // blocks. This covers more changed declarations without clipping patches
    // or dropping the history sample's continuity and omission metadata.
    let costs: BTreeMap<_, _> = order.iter().map(|i| (*i, text::count(&contexts[s(&candidates[*i].1, "id")].to_string()))).collect();
    order.sort_by_key(|i| {
        let entry = &candidates[*i].1;
        let history = analyzed(&contexts[s(entry, "id")]);
        (
            !history,
            !is_head_change(entry),
            entry["side"] != "head",
            costs[i],
            *i,
        )
    });
    let mut used = 0usize;
    let mut allocations = BTreeMap::new();
    for index in order {
        let (_, entry, cost) = &mut candidates[index];
        let base_cost = cost.exact(entry);
        let mut enriched = entry.clone();
        enriched["business_context"] = contexts[s(entry, "id")].clone();
        enriched.as_object_mut().unwrap().remove("revision");
        enriched["revision"] = json!(crate::digest(&enriched));
        let enriched_cost =
            text::count(&enriched.to_string()) + text::count(&enriched["id"].to_string()) + 24;
        let extra = enriched_cost.saturating_sub(base_cost);
        if used + extra <= allowance {
            used += extra;
            allocations.insert(s(entry, "id").to_owned(), extra);
            *entry = enriched;
            *cost = PackingCost { exact: Some(enriched_cost), exceeds: 0 };
        }
    }
    allocations
}

/// A completed review is an external attestation. We only prove that its code,
/// native dependency closure and review context still match available inputs.
fn reusable_reviews(
    r: &Request,
    head: &Input,
    analysis: &Analysis,
    report: &Value,
    previous: Option<&Value>,
    context: &str,
) -> Vec<String> {
    let Some(previous) = previous else {
        return vec![];
    };
    if previous["review_context_sha256"] != context {
        return vec![];
    }
    let packages: BTreeMap<_, _> = analysis
        .packages
        .iter()
        .map(|p| {
            (
                (s(p, "consumer_build"), s(p, "path")),
                s(p, "content_instance"),
            )
        })
        .collect();
    let old: BTreeMap<_, _> = previous["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| (s(e, "id"), e))
        .collect();
    let current: BTreeMap<_, _> = report["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| (s(e, "id"), e))
        .collect();
    r.reviewed_entries
        .iter()
        .filter(|id| {
            let Some(entry) = old.get(id.as_str()) else {
                return false;
            };
            let Some(next) = current.get(id.as_str()) else {
                return false;
            };
            if entry["side"] != "head"
                || entry["review_basis"]["complete"] != true
                || next["review_basis"]["complete"] != true
                || entry["review_basis"]["scope"] != next["review_basis"]["scope"]
            {
                return false;
            }
            let Some((world, symbol)) = analysis.symbols.get(s(entry, "symbol_id")) else {
                return false;
            };
            let source = &head.sources[&symbol.file];
            source.repository == s(entry, "repository_id")
                && source.path == s(entry, "file")
                && source.sha == s(entry, "file_sha256")
                && packages
                    .get(&(world.as_str(), symbol.package.as_str()))
                    .is_some_and(|hash| !hash.is_empty() && *hash == s(entry, "content_instance"))
        })
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn check_receipt_access(
    db: &rusqlite::Connection,
    principal: &store::Principal,
    receipt: &Value,
) -> Result<()> {
    for side in ["base", "head"] {
        for pin in receipt[side]
            .as_array()
            .ok_or_else(|| Error::conflict("Invalid review receipt; request a full packet"))?
        {
            // Full packets can report unavailable pins. A delta cannot reuse
            // such a receipt safely; keep the access check and offer recovery.
            store::repo(db, principal, s(pin, "repository_id")).map_err(|_| {
                Error::conflict(
                    "Previous packet references an unavailable repository; request a full packet",
                )
            })?;
        }
    }
    Ok(())
}

pub fn run(store: &Store, workspace: &str, token: &str, args: &Value) -> Result<Value> {
    // Authorization precedes parsing scope details, preserving the core API's
    // refusal boundary even for legacy/unsupported request shapes.
    let anchor = crate::string(args, "repository_id", 100)?;
    store.authorized_read(workspace, token, "read", |db, p| {
        store::repo(db, p, anchor).map(|_| ())
    })?;
    let mut request_args = args.clone();
    if let Some(object) = request_args.as_object_mut() {
        object.remove("repo"); // Already resolved and authorized by the MCP adapter.
    }
    let r: Request = serde_json::from_value(request_args)
        .map_err(|e| Error::bad(format!("Expected pinned base/head review request: {e}")))?;
    validate(&r)?;
    let previous=store.authorized_read(workspace,token,"read",|db,p|{
        let Some(id)=&r.previous_packet_id else{return Ok(None)};
        let row=store::one(db,"SELECT receipt FROM repo_review_packets WHERE id=? AND repository_id=? AND principal_hash=?",params![id,anchor,p.hash])?.ok_or_else(||Error::conflict("Previous packet unavailable; request a full packet"))?;
        let receipt:Value=serde_json::from_str(s(&row,"receipt"))?;
        if receipt["profile"]!=serde_json::to_value(&r.profile)? || receipt["base"]!=serde_json::to_value(&r.base)?{return Err(Error::conflict("Review profile changed; request a full packet"));}
        check_receipt_access(db,p,&receipt)?;
        Ok(Some(receipt))
    })?;
    let base = crate::phase::phase("review_load_base", || {
        load(store, workspace, token, &r.base, &r.profile)
    })?;
    let head = crate::phase::phase("review_load_head", || {
        load(store, workspace, token, &r.head, &r.profile)
    })?;
    if let Some(options) = &r.business_context {
        crate::business_context::validate(options, &r.base, &r.head, |pin, path| {
            [&base, &head].into_iter().find_map(|input| {
                if !input.pins.contains(pin) {
                    return None;
                }
                input
                    .sources
                    .get(&format!("{}/{path}", pin.directory))
                    .filter(|source| source.repository == pin.repository_id)
                    .and_then(|source| {
                        source
                            .content
                            .text()
                            .map(|text| (source.sha.clone(), text.lines().count()))
                    })
            })
        })?;
    }
    let identity = (basic::has_go(&base) || basic::has_go(&head)).then(graphicle::extractors::go_types::review_identity).and_then(std::result::Result::ok)
        .map(|v| crate::digest(&v));
    let fact_key = |input: &Input| {
        identity.as_ref().map(|identity|crate::digest(&json!({
        "identity":identity,"profile":r.profile,"fact_policy":"pinned-type-diagnostics-basic-roots-v2",
        "roots":input.pins.iter().map(|p|json!([p.repository_id,p.directory,p.modules,p.snapshot,p.policy_sha256])).collect::<Vec<_>>(),
        "sources":input.sources.iter().map(|(path,s)|json!([path,s.repository,s.sha])).collect::<Vec<_>>() })))
    };
    let key_a = fact_key(&base);
    let key_b = fact_key(&head);
    let read_cache = |key: &Option<String>| -> Result<Option<Value>> {
        match key {
            Some(key) => crate::review_cache::load(store, workspace, token, anchor, key),
            None => Ok(None),
        }
    };
    let cached_a = read_cache(&key_a)?;
    let hit_a = cached_a.is_some();
    let mut a = crate::phase::phase("review_analyze_base", || {
        analyze(&base, &r.profile, cached_a)
    })?;
    let cached_b = if key_a.is_some() && key_a == key_b {
        a.raw.clone()
    } else {
        read_cache(&key_b)?
    };
    let hit_b = cached_b.is_some();
    let mut b = crate::phase::phase("review_analyze_head", || {
        analyze(&head, &r.profile, cached_b)
    })?;
    services::attach(&base, &mut a)?;
    services::attach(&head, &mut b)?;
    let mut report = crate::phase::phase("review_compare", || compare(&base, &head, &a, &b, &r))?;
    let coverage_ids = store.authorized_read(workspace, token, "read", |db, p| {
        let visible = |pins: &[Pin]| {
            pins.iter()
                .filter(|pin| store::repo(db, p, &pin.repository_id).is_ok())
                .cloned()
                .collect::<Vec<_>>()
        };
        crate::coverage::annotate(
            db,
            &mut report,
            &visible(&r.base),
            &visible(&r.head),
            &r.profile,
        )
    })?;
    if let Some(options) = &r.business_context {
        let (suggestions, omitted) = crate::business_context::suggestions(&report, options, &r.base, &r.head);
        for suggestion in suggestions {
            let side = s(&suggestion, "side");
            let (input, analysis) = if side == "base" {
                (&base, &a)
            } else {
                (&head, &b)
            };
            let peer = &suggestion["peer"];
            // Statistical evidence never chooses between ambiguous build views.
            // Include all exact native views, retaining their package bindings.
            for (id, (world, symbol)) in &analysis.symbols {
                let source = &input.sources[&symbol.file];
                if suggestion["repository_id"] != source.repository
                    || peer["file"] != source.path
                    || peer["file_sha256"] != source.sha
                    || peer["byte_start"] != symbol.start
                    || peer["byte_end"] != symbol.end
                {
                    continue;
                }
                let entries = report["entries"].as_array_mut().unwrap();
                if entries
                    .iter()
                    .any(|e| e["side"] == side && e["symbol_id"] == *id)
                {
                    continue;
                }
                let binding = analysis
                    .packages
                    .iter()
                    .find(|p| p["consumer_build"] == *world && p["path"] == symbol.package)
                    .map(|p|json!({"instance":p["instance"],"content_instance":p["content_instance"],"consumer_build":p["consumer_build"],"module":p["module"],"version":p["version"],"replacement":p["replacement"]}))
                    .unwrap_or(Value::Null);
                entries.push(entry(
                    side,
                    id,
                    symbol,
                    source,
                    std::slice::from_ref(&suggestion),
                    &binding,
                ));
            }
        }
        crate::phase::phase("review_business_context", || {
            crate::business_context::annotate(&mut report, options, &r.base, &r.head, r.max_tokens)
        });
        report["business_context"]["omitted_peer_suggestions"] = json!(omitted);
    }
    let context_sha = crate::digest(
        &json!({"business_context":r.business_context,"execution_coverage":coverage_ids,
            "service_graph":{"base":a.services.summary(),"head":b.services.summary()}}),
    );
    let scopes = crate::phase::phase("review_reuse_scopes", || {
        reuse::annotate(&mut report, &a, &b)
    });
    let reusable = crate::phase::phase("review_reuse_decisions", || {
        reusable_reviews(&r, &head, &b, &report, previous.as_ref(), &context_sha)
    });
    report["review_reuse"] = json!({"eligible":reusable,"requested":r.reviewed_entries.len(),"context_sha256":context_sha,"not_a_review_verdict":true,"policy":"resolved_component_with_complete_context_v1"});
    report["text_reuse"] = json!(r.reuse_text);
    let (mut report, receipt) = crate::phase::phase("review_pack", || {
        pack(report, r.max_tokens, previous.as_ref(), Some(&scopes))
    })?;
    // Execution diagnostics are outside packet identity, like the token count.
    report["analysis_cache"] =
        json!({"base_hit":hit_a,"head_hit":hit_b,"granularity":"exact_snapshot_set"});
    report["tokens"] = json!(text::count(&report.to_string()) + 8);
    if text::count(&report.to_string()) > r.max_tokens {
        return Err(Error::bad("Review diagnostics exceed token budget"));
    }
    store.authorized(workspace,token,"read",|db,p|{
        store::repo(db,p,anchor)?;
        if let Some(previous)=&previous {check_receipt_access(db,p,previous)?;}
        for repository in base.sources.values().chain(head.sources.values()).map(|s|&s.repository).collect::<BTreeSet<_>>(){store::repo(db,p,repository)?;}
        // Source forgetting/retention may run while the compiler is working.
        // Do not serve materialized bytes that have since been removed.
        let mut exists=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)")?;
        for source in base.sources.values().chain(head.sources.values()) {
            if !exists.query_row(params![source.repository,source.sha,source.content.bytes().len()],|row|row.get::<_,bool>(0))? {return Err(Error::conflict("Pinned source was removed during review; retry with an available snapshot"));}
        }
        drop(exists);
        for input in [&base, &head] {
            for (pin, complete) in &input.bindings {
                let current = store::one(db,"SELECT scope_complete FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?",params![pin.repository_id,pin.snapshot,pin.commit,pin.policy_sha256])?;
                if current.is_some_and(|v| (v["scope_complete"] == 1) != *complete) {
                    return Err(Error::conflict("Snapshot scope attestation changed during review; retry"));
                }
            }
            for pin in &input.pins {
                if input.sources.values().any(|source| source.repository == pin.repository_id) && store::one(db,"SELECT sha256 FROM repo_snapshots WHERE repository_id=? AND sha256=?",params![pin.repository_id,pin.snapshot])?.is_none() {
                    return Err(Error::conflict("Pinned snapshot was removed during review"));
                }
            }
        }
        for id in &coverage_ids {
            let row=store::one(db,"SELECT repository_id FROM repo_review_coverage WHERE id=?",[id])?.ok_or_else(||Error::conflict("Coverage removed during review; retry"))?;
            store::repo(db,p,s(&row,"repository_id"))?;
        }
        for (input,analysis,key) in [(&base,&a,&key_a),(&head,&b,&key_b)] {
            if input.bindings.len()==input.pins.len() && let (Some(key),Some(facts))=(key,&analysis.raw) {
                crate::review_cache::save(db,anchor,key,facts,&input.pins)?;
            }
        }
        db.execute("INSERT OR IGNORE INTO repo_review_packets(id,repository_id,principal_hash,receipt,created_at) VALUES(?,?,?,?,?)",params![s(&report,"packet_id"),anchor,p.hash,receipt.to_string(),crate::now()])?;
        db.execute("DELETE FROM repo_review_packets WHERE repository_id=? AND principal_hash=? AND id NOT IN (SELECT id FROM repo_review_packets WHERE repository_id=? AND principal_hash=? ORDER BY rowid DESC LIMIT 20)",params![anchor,p.hash,anchor,p.hash])?;
        Ok(report)
    })
}

pub fn read_request(path: &Path) -> Result<Value> {
    use std::io::Read;
    let mut bytes = vec![];
    std::fs::File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(Error::bad("Review request exceeds 1 MiB"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Called in the existing sync transaction, including identical-tree replays.
/// These are collector attestations, not Git signatures.
pub(crate) fn bind_snapshot(
    db: &rusqlite::Connection,
    repository: &str,
    snapshot: &str,
    metadata: &Value,
) -> Result<()> {
    let commit = s(metadata, "commit");
    let policy = s(metadata, "policy_sha256");
    if ![40, 64].contains(&commit.len())
        || !commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || store::hash(policy).is_err()
    {
        return Ok(());
    }
    db.execute("INSERT INTO repo_snapshot_bindings(repository_id,snapshot,commit_id,policy_sha256,scope_complete) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,snapshot,commit_id,policy_sha256) DO UPDATE SET scope_complete=excluded.scope_complete",params![repository,snapshot,commit,policy,metadata["scope_complete"]==true])?;
    Ok(())
}

/// Wire schema shared by the MCP adapter; API and CLI use the same validator.
pub(crate) fn pin_schema() -> Value {
    json!({"type":"object","properties":{"repository_id":{"type":"string"},"commit":{"type":"string"},"snapshot":{"type":"string"},"policy_sha256":{"type":"string"},"directory":{"type":"string"},"modules":{"type":"array","items":{"type":"string"}}},"required":["repository_id","commit","snapshot","policy_sha256","directory"],"additionalProperties":false})
}
pub fn properties() -> Value {
    let pin = pin_schema();
    json!({"base":{"type":"array","items":pin,"minItems":1,"maxItems":64},"head":{"type":"array","items":pin,"minItems":1,"maxItems":64},"profile":{"type":"object","properties":{"goos":{"type":"string"},"goarch":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"tests":{"type":"boolean"},"workspace":{"type":"string"}},"required":["goos","goarch"],"additionalProperties":false},"max_depth":{"type":"integer","minimum":0,"maximum":32},"max_tokens":{"type":"integer","minimum":512,"maximum":64000},"previous_packet_id":{"type":"string"},"reuse_text":{"type":"boolean"},"reviewed_entries":{"type":"array","maxItems":1000,"items":{"type":"string"}},"business_context":crate::business_context::schema(),"repo":{"type":"string"}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_bounded_cost_can_be_retried_with_a_larger_allowance() {
        let entry = json!({"id":"one","text":"fn unicode() { /* 中文 */ }\n".repeat(200)});
        let expected = PackingCost::measured(&entry).exact.unwrap();
        let mut lazy = PackingCost::default();
        assert_eq!(lazy.within(&entry, 32), None);
        assert_eq!(lazy.exact, None);
        assert_eq!(lazy.within(&entry, 20), None);
        assert_eq!(lazy.within(&entry, expected), Some(expected));
        assert_eq!(lazy.within(&entry, expected - 1), None);
    }

    #[test]
    fn history_reservation_prefers_complete_affordable_required_blocks() {
        let entry = |id: &str, required: bool| json!({"id":id,"side":"head","text":"fn f() {}","reasons":[{"reason":if required {"change_seed"} else {"dependency"}}]});
        let context = |subject: String| json!({"evolution":{"function_history":[{"subject":subject,"patch_excerpt":"entire patch","patch_truncated":false}],"sample":{"complete":false,"omissions":{"unavailable_or_withheld_unit":1}}}});
        let short = context("Earlier fix".into());
        let contexts = BTreeMap::from([("large".into(),context("long history ".repeat(500))), ("first".into(),short.clone()), ("second".into(),short.clone()), ("optional".into(),short.clone())]);
        let mut candidates: Vec<_> = [("large",true),("optional",false),("first",true),("second",true)].into_iter().map(|(id,required)| ("entries",entry(id,required),PackingCost::default())).collect();
        let costs = fit_business_context(&mut candidates, &contexts, 500, true);
        assert_eq!(costs.len(),2);
        assert!(costs.contains_key("first") && costs.contains_key("second"));
        for (_, entry, _) in &candidates[2..] { assert_eq!(entry["business_context"], short); }
        assert!(candidates[1].1.get("business_context").is_none());
        // Optional context is considered only after code selection.
        assert_eq!(fit_business_context(&mut candidates[1..2], &contexts, 500, false).len(), 1);
    }

    #[test]
    fn abundant_history_preserves_an_affordable_complete_hunk() {
        let entries: Vec<_> = (0..8).map(|i| json!({"id":format!("head{i}"),"side":"head","revision":"r","text":"work(); ".repeat(180),"reasons":[{"reason":"change_seed"}]})).collect();
        let plain = json!({"entries":entries,"diffs":[{"file":"f.go","before":"old(); ".repeat(60),"after":"new(); ".repeat(60)}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]}});
        let mut checked = 0;
        for budget in [6000,6500,7000,7500] {
            let (baseline, _) = pack(plain.clone(),budget,None,None).unwrap();
            if baseline["coverage"]["current_head"]["complete"] != true || baseline["diffs"].as_array().unwrap().is_empty() { continue; }
            checked += 1;
            let mut enriched = plain.clone();
            enriched["business_context"] = json!({"complete":false});
            for entry in enriched["entries"].as_array_mut().unwrap() {
                entry["business_context"] = json!({"evolution":{"function_history":[{"subject":"Earlier behavior ".repeat(100),"patch_excerpt":"complete patch","patch_truncated":false}]}});
            }
            let (packet, _) = pack(enriched,budget,None,None).unwrap();
            assert_eq!(packet["coverage"]["current_head"]["complete"],true);
            assert_eq!(packet["diffs"],baseline["diffs"]);
        }
        assert!(checked > 0);
    }

    #[test]
    fn call_contract_source_precedes_optional_context_and_keeps_delta_identity() {
        let caller = json!({"id":"caller","symbol_id":"caller","revision":"r","repository_id":"repo","side":"head","text":"items, _ := source.Load(); publish(items)","reasons":[{"reason":"change_seed"}]});
        let supplier = json!({"id":"supplier","symbol_id":"supplier","revision":"r","repository_id":"repo","side":"head","text":"func Load() ([]string, error) { if unavailable { return nil, err }; return items, nil }","reasons":[{"reason":"dependency","path":["caller","supplier"],"steps":[{"relation":"may_call","evidence":"interface_implementation","discarded_error":true}]}]});
        let mut entries = vec![caller, supplier];
        entries.extend((0..30).map(|i| json!({"id":format!("optional{i}"),"revision":"r","repository_id":"repo","side":"head","text":"optional consumer context; ".repeat(200),"reasons":[{"reason":"potential_consumer"}]})));
        let report = json!({"entries":entries,"diffs":[{"file":"main.go","before":"old(); ".repeat(500),"after":"new(); ".repeat(500)}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]}});
        for budget in [2048, 4096] {
            let (full, receipt) = pack(report.clone(), budget, None, None).unwrap();
            let (delta, next) = pack(report.clone(), budget, Some(&receipt), None).unwrap();
            assert_eq!(full["coverage"]["current_head"]["complete"], true);
            assert_eq!(full["coverage"]["call_contracts"]["selected"], 1);
            assert!(full["entries"].as_array().unwrap().iter().any(|e| e["id"] == "supplier" && e["text"].as_str().unwrap().contains("return nil, err")));
            assert_eq!(receipt["entries"], next["entries"]);
            assert_eq!(full["context_order"], delta["context_order"]);
            assert_eq!(full["coverage"]["complete"], false);
            for packet in [&full, &delta] { assert!(text::count(&packet.to_string()) <= budget); }
        }
        let mut huge = report;
        huge["entries"][1]["text"] = json!("a large supplier body; ".repeat(4000));
        let (packet, _) = pack(huge, 2048, None, None).unwrap();
        assert_eq!(packet["coverage"]["current_head"]["complete"], true);
        assert_eq!(packet["coverage"]["call_contracts"]["complete"], false);
    }

    #[test]
    fn graph_bound_counts_unique_links_and_preserves_call_site_evidence() {
        let mut analysis = Analysis { symbols:BTreeMap::new(),calls:vec![],packages:vec![],issues:vec![],analyzer:String::new(),toolchain:String::new(),inputs:vec![],raw:None,services:services::Links::default() };
        for name in ["caller", "supplier", "other"] {
            let symbol: Symbol = serde_json::from_value(json!({"key":name,"file":"app/main.go","package":"app","name":name,"kind":"function","parent":"","start":0,"end":1,"body_start":0,"line":1,"end_line":1,"revision":"r","signature":"s","implementation":"i","correspondence":"c"})).unwrap();
            analysis.symbols.insert(key("world", name), ("world".into(), symbol));
        }
        for n in 0..5 {
            let call: Call = serde_json::from_value(json!({"caller":"caller","target":"supplier","target_offset":0,"spelling":"s.Load","file":"app/main.go","start":n,"end":n+1,"line":n+1,"relation":"calls","discarded_error":n==3})).unwrap();
            analysis.calls.push(("world".into(), call));
        }
        let links = call_links(&analysis, 1).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].2, 3);
        assert_eq!(analysis.calls.len(), 5);
        assert_eq!(call_evidence(&analysis.calls[3].0, &analysis.calls[3].1)["byte_start"], 3);
        let mut distinct = analysis.calls[0].clone();
        distinct.1.target = "other".into();
        analysis.calls.push(distinct);
        assert!(call_links(&analysis, 1).is_err());
        assert_eq!(call_links(&analysis, 2).unwrap().len(), 2);
        analysis.calls.clear();
        analysis.services.edges.push(services::Edge { from: "caller".into(), to: "supplier".into(), relation: "http_calls", evidence: json!({}) });
        assert!(call_links(&analysis, 0).is_err());
    }

    #[test]
    fn only_inventory_backed_type_failures_are_cacheable() {
        let sample = json!({"analyzer":"fixture","toolchain":"go","worlds":[{"root":"app","symbols":[],"calls":[],"packages":[{"path":"app","files":["app/main.go"]}],"inputs":[{"file":"app/main.go","package":"app","role":"selected"}],"issues":["incomplete_package:app","type_check_failed:app"]}]});
        let check = |value| cacheable_facts(&serde_json::from_value(value).unwrap());
        assert!(check(sample.clone()));
        for reason in ["incomplete_discovery:app","invalid_package_metadata","type_check_failed:other","unreadable_source"] {
            let mut bad = sample.clone(); bad["worlds"][0]["issues"][0] = json!(reason);
            assert!(!check(bad));
        }
        let mut absent = sample.clone(); absent["worlds"][0]["inputs"] = json!([]); assert!(!check(absent));
        let mut missing = sample; missing["worlds"][0]["packages"][0]["files"] = json!([]); assert!(!check(missing));
    }

    #[test]
    fn affordable_complete_flow_uses_the_fractional_reservation() {
        let nodes: Vec<_> = (0..25).map(|i| crate::digest(&json!(["node",i]))).collect();
        let steps: Vec<_> = nodes.windows(2).map(|n| json!({"caller":n[0],"callee":n[1],"relation":"calls"})).collect();
        let entry = json!({"id":crate::digest(&json!("entry")),"side":"head","revision":"r","text":"func Changed() {}","reasons":[{"reason":"change_seed"},{"reason":"dependency","path":nodes,"steps":steps,"execution_flow":{"kind":"potential_static"}}]});
        let report = json!({"entries":[entry],"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[],"max_depth":32}});
        let (packet,_) = pack(report,64000,None,None).unwrap();
        assert_eq!(packet["execution_flows"]["paths"][0]["steps"].as_array().unwrap().len(),24);
        assert!(packet["execution_flows"]["coverage"]["used_tokens"].as_u64().unwrap()>2048);
        assert!(text::count(&packet.to_string())<=64000);
    }

    #[test]
    fn optional_examples_yield_to_required_code_and_whole_history() {
        let history = json!({"evolution":{"function_history":[{"subject":"A complete earlier constraint ".repeat(70),"commit":"a".repeat(40)}]}});
        let entries: Vec<_> = (0..8).map(|i| json!({"id":format!("head{i}"),"revision":"r","side":"head","text":"perform_work(); ".repeat(40),"reasons":[{"reason":"change_seed","path":[format!("head{i}")]},{"reason":"dependency","path":["caller","callee"],"steps":[{"spelling":"additional explanation ".repeat(100)}]},{"reason":"potential_consumer","uncertain_scope":true}],"business_context":history})).collect();
        let report = json!({"entries":entries,"diffs":[],"changes":(0..20).map(|i|json!({"kind":"modified","head":i,"explanation":"optional detail ".repeat(300)})).collect::<Vec<_>>(),"file_changes":[{"file":"changed.go","classification":"declaration_change"}],"package_instances":{"base":[],"head":[]},"business_context":{"complete":false},"coverage":{"complete":false,"issues":[{"reason":"unresolved_call"}]}});
        let (full, receipt) = pack(report.clone(),8000,None,None).unwrap();
        assert_eq!(full["coverage"]["current_head"]["selected"],8);
        assert_eq!(full["changes"]["count"],20);
        assert_eq!(full["changes"]["examples"],json!([]));
        assert!(full["entries"].as_array().unwrap().iter().any(|e| e.get("business_context")==Some(&history)));
        for entry in full["entries"].as_array().unwrap() {
            assert_eq!(entry["reason_examples"]["omitted"],1);
            assert!(entry["reasons"].as_array().unwrap().iter().any(|r| r["uncertain_scope"]==true));
        }
        let (delta, next) = pack(report,8000,Some(&receipt),None).unwrap();
        assert_eq!(receipt["entries"],next["entries"]);
        assert_eq!(full["context_order"],delta["context_order"]);
        for packet in [full,delta] { assert!(text::count(&packet.to_string())<=8000); assert_eq!(packet["coverage"]["complete"],false); }
    }

    #[test]
    fn scarce_optional_capacity_retains_whole_history_and_affordable_hunks() {
        let history = json!({"evolution":{"function_history":[{"subject":"prior behavioral constraint ".repeat(200),"commit":"b".repeat(40)}]}});
        for budget in [8000, 9000, 10000] {
            let diffs: Vec<_> = (0..8).map(|i| json!({"id":format!("diff-{i}"),"file":"f.go","before":"old text ".repeat(100),"after":"new text ".repeat(100)})).collect();
            let report = json!({"entries":[{"id":"changed","revision":"r","side":"head","repository_id":"repo","text":"return quote(); ".repeat(1400),"reasons":[{"reason":"change_seed"}],"business_context":history}],"diffs":diffs,"changes":[],"package_instances":{"base":[],"head":[]},"business_context":{"complete":false},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
            let (full, receipt) = pack(report.clone(), budget, None, None).unwrap();
            assert_eq!(full["coverage"]["current_head"]["selected"], 1);
            assert_eq!(full["entries"][0]["business_context"], history, "budget={budget}: {:?}", full["business_context"]);
            assert!(!full["diffs"].as_array().unwrap().is_empty());
            let (delta, delta_receipt) = pack(report, budget, Some(&receipt), None).unwrap();
            assert_eq!(receipt["entries"], delta_receipt["entries"]);
            assert_eq!(full["context_order"], delta["context_order"]);
            assert!(text::count(&full.to_string()) <= budget);
            assert!(text::count(&delta.to_string()) <= budget);
        }
    }

    #[test]
    fn diff_reserve_cannot_evict_required_current_declarations() {
        let entries: Vec<_> = (0..12).map(|i| json!({"id":format!("head{i}"),"revision":"r","repository_id":"repo","side":"head","text":"perform_work(); ".repeat(80),"reasons":[{"reason":"change_seed"}]})).collect();
        let mut report = json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (code_only, _) = pack(report.clone(), 64000, None, None).unwrap();
        let budget = code_only["coverage"]["current_head"]["minimum_tokens_with_hunk_reserve"].as_u64().unwrap() as usize + 512;
        assert!(budget < 64000);
        report["diffs"] = json!((0..32).map(|i| json!({"file":"main.go","hunk":i,"before":"old_call(); ".repeat(20),"after":"new_call(); ".repeat(20)})).collect::<Vec<_>>());
        let (full, receipt) = pack(report.clone(), budget, None, None).unwrap();
        let (delta, _) = pack(report, budget, Some(&receipt), None).unwrap();
        for packet in [&full, &delta] {
            assert_eq!(packet["coverage"]["current_head"]["selected"], 12, "required code fits; diffs may be omitted: {}", packet["coverage"]);
            assert!(text::count(&packet.to_string()) <= budget);
            assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
            assert_eq!(packet["coverage"]["complete"], false);
        }
    }

    #[test]
    fn large_review_packets_remain_bounded() {
        let entries: Vec<_> = (0..20_000).map(|i| json!({"id":format!("{i:064x}"),"revision":"r","repository_id":"repo","text":"func F() int { return 1 }\n".repeat(8)})).collect();
        let report = json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let start = std::time::Instant::now();
        let (packet, _) = super::pack(report, 16_000, None, None).unwrap();
        eprintln!("packed 20,000 candidates in {:?}", start.elapsed());
        assert!(text::count(&packet.to_string()) <= 16_000);
        assert_eq!(packet["coverage"]["required_entries"], 20_000);
        assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
        assert_eq!(packet["impact"], "incomplete");
    }

    #[test]
    fn inclusion_explanations_are_bounded_without_losing_the_changed_reason() {
        let mut reasons = vec![];
        for n in (1..=100).rev() {
            super::add_reason(
                &mut reasons,
                json!({"reason":"dependency","path":vec!["node";n]}),
            );
        }
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0]["path"].as_array().unwrap().len(), 1);
        super::add_reason(
            &mut reasons,
            json!({"reason":"change_seed","path":["changed"]}),
        );
        super::add_reason(
            &mut reasons,
            json!({"reason":"potential_consumer","path":["seed","changed"]}),
        );
        assert_eq!(reasons.len(), 2);
        assert!(reasons.iter().any(|r| r["reason"] == "change_seed"));
        super::add_reason(&mut reasons, json!({"reason":"change_seed","path":["changed"]}));
        assert_eq!(reasons.len(), 2);
    }

    #[test]
    fn hunks_and_current_code_survive_a_full_dependency_budget() {
        let mut entries: Vec<_> = (0..100).map(|i| json!({"id":format!("{i:064x}"),"revision":"r","repository_id":"repo","side":"head","text":"unchanged dependency code; ".repeat(100),"reasons":[{"reason":"conservative_scope","uncertain_scope":true}]})).collect();
        entries.insert(0,json!({"id":"changed","revision":"r","repository_id":"repo","side":"head","text":"func Changed() { UseNewContract() }","reasons":[{"reason":"change_seed"}]}));
        let report = json!({"entries":entries,"diffs":[{"file":"main.go","hunk":0,"before":"return 1","after":"return 2"}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (packet, _) = super::pack(report, 4096, None, None).unwrap();
        assert_eq!(packet["coverage"]["current_head"]["selected"], 1);
        assert_eq!(packet["diffs"].as_array().unwrap().len(), 1);
        assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
        assert!(text::count(&packet.to_string()) <= 4096);
    }

    #[test]
    fn optional_entries_cannot_starve_selected_execution_flow_references() {
        let mut entries: Vec<_> = (0..100).map(|i| json!({"id":format!("optional{i}"),"revision":"r","repository_id":"repo","side":"head","text":"optional dependency; ".repeat(8),"reasons":[{"reason":"conservative_scope","uncertain_scope":true}]})).collect();
        entries.insert(0,json!({"id":"changed","revision":"r","repository_id":"repo","side":"head","text":"func F() { G() }","reasons":[{"reason":"change_seed"},{"reason":"dependency","path":["f","g"],"steps":[{"caller":"f","callee":"g","relation":"calls"}],"execution_flow":{"kind":"potential_static"}}]}));
        let report=json!({"entries":entries,"diffs":[{"file":"f.go","before":"old()","after":"new()"}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[],"max_depth":8},"impact":"potential_impact"});
        for budget in [4096,8000] {
            let (packet,receipt)=pack(report.clone(),budget,None,None).unwrap();
            assert_eq!(packet["coverage"]["current_head"]["selected"],1);
            assert_eq!(packet["diffs"].as_array().unwrap().len(),1);
            assert_eq!(packet["execution_flows"]["paths"].as_array().unwrap().len(),1);
            assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap()>0);
            let (delta,_)=pack(report.clone(),budget,Some(&receipt),None).unwrap();
            assert_eq!(delta["execution_flows"],packet["execution_flows"]);
            for p in [&packet,&delta] { assert!(text::count(&p.to_string())<=budget); }
        }
    }

    #[test]
    fn cited_flow_enrichment_cannot_starve_changed_symbol_history() {
        let make_report = |count: usize| {
            let entries: Vec<_> = (0..count).map(|i| {
                let reason = json!({"reason":"dependency","path":[format!("f{i}"),"g"],"steps":[{"caller":format!("f{i}"),"callee":"g","relation":"calls","source_file":"app/main.go","byte_start":12,"byte_end":16}]});
                let mut cited = reason.clone();
                cited["execution_flow"] = json!({"kind":"potential_static","observed":false,"citations_complete":true});
                cited["steps"][0]["citation"] = json!({"repository_id":"repo","commit":"b".repeat(40),"snapshot":"c".repeat(64),"policy_sha256":"d".repeat(64),"file":"main.go","sha256":"e".repeat(64),"byte_start":12,"byte_end":16});
                json!({"id":format!("head{i}"),"revision":"a".repeat(64),"side":"head","repository_id":"repo","text":"return quote(); ".repeat(6),"reasons":[{"reason":"change_seed"},reason],"_execution_flow_reasons":[{"reason":"change_seed"},cited],"business_context":{"evolution":{"function_history":[{"subject":"Fix tenant isolation","commit":"f".repeat(40)}]}}})
            }).collect();
            json!({"entries":entries,"diffs":[{"file":"main.go","before":"return 1","after":"return 2"}],"changes":[],"package_instances":{"base":[],"head":[]},"business_context":{"complete":false},"coverage":{"complete":true,"issues":[],"max_depth":8},"impact":"potential_impact"})
        };
        let (first, receipt) = pack(make_report(21), 8000, None, None).unwrap();
        let (full, full_receipt) = pack(make_report(23), 8000, None, None).unwrap();
        let (delta, delta_receipt) = pack(make_report(23), 8000, Some(&receipt), None).unwrap();
        for (packet, count) in [(&first, 21), (&full, 23)] {
            assert_eq!(packet["coverage"]["current_head"]["selected"], count, "{packet}");
            assert_eq!(packet["diffs"].as_array().unwrap().len(), 1);
            assert!(packet["entries"].as_array().unwrap().iter().any(|e| is_head_change(e) && e["business_context"]["evolution"]["function_history"].as_array().is_some_and(|h| !h.is_empty())), "{packet}");
            assert!(!packet.to_string().contains("_execution_flow_reasons"));
        }
        assert_eq!(full_receipt["entries"], delta_receipt["entries"]);
        for key in ["context_order", "execution_flows", "business_context"] {
            assert_eq!(full[key], delta[key]);
        }
        for packet in [&first, &full, &delta] { assert!(text::count(&packet.to_string()) <= 8000); }
    }

    #[test]
    fn unused_flow_and_hunk_reservations_keep_whole_history() {
        let reason = json!({"reason":"dependency","path":["f","g"],"steps":[{"caller":"f","callee":"g","relation":"calls"}]});
        let mut cited = reason.clone();
        cited["execution_flow"] = json!({"kind":"potential_static","observed":false});
        cited["steps"][0]["citation"] = json!({"file":"nested/".repeat(2000),"sha256":"a".repeat(64)});
        // A whole history block fits after the overlarge hunk is omitted, but
        // not while reserving a quarter of the packet for that hunk. The flow
        // citation cannot fit either; its unused reservation must be released.
        let history = json!({"evolution":{"function_history":[{"subject":"history evidence ".repeat(800),"commit":"b".repeat(40)}]}});
        let report = json!({"entries":[{"id":"changed","revision":"r","side":"head","repository_id":"repo","text":"return quote(); ".repeat(1400),"reasons":[{"reason":"change_seed"},reason],"_execution_flow_reasons":[{"reason":"change_seed"},cited],"business_context":history}],"diffs":[{"file":"f.go","before":"old text ".repeat(3000),"after":"new text ".repeat(3000)}],"changes":[],"package_instances":{"base":[],"head":[]},"business_context":{"complete":false},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (packet, _) = pack(report, 8000, None, None).unwrap();
        assert_eq!(packet["coverage"]["current_head"]["selected"], 1);
        assert_eq!(packet["entries"][0]["business_context"], history, "{packet}");
        assert!(packet["business_context"]["packing"]["used_tokens"].as_u64().unwrap() > 0, "{:?}", packet["business_context"]["packing"]);
        assert_eq!(packet["execution_flows"]["paths"], json!([]));
        assert_eq!(packet["execution_flows"]["coverage"]["used_tokens"], 0);
        assert_eq!(packet["execution_flows"]["coverage"]["omitted"], 1);
        assert!(text::count(&packet.to_string()) <= 8000);
    }

    #[test]
    fn old_bodies_cannot_starve_changed_head_declarations() {
        let mut entries:Vec<_>=(0..21).map(|i|json!({"id":format!("head{i}"),"revision":"r","side":"head","repository_id":"repo","text":"func F() { changed() }","reasons":[{"reason":"change_seed"}]})).collect();
        entries.insert(1,json!({"id":"old","revision":"r","side":"base","repository_id":"repo","text":"old body; ".repeat(2000),"reasons":[{"reason":"change_seed"}]}));
        let report = json!({"entries":entries,"diffs":[{"file":"f.go","hunk":0,"before":"old()","after":"new()"}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (packet, _) = pack(report, 4096, None, None).unwrap();
        assert_eq!(packet["coverage"]["current_head"]["selected"], 21);
        assert_eq!(packet["coverage"]["current_head"]["complete"], true);
        assert_eq!(packet["diffs"].as_array().unwrap().len(), 1);
        assert!(text::count(&packet.to_string()) <= 4096);
    }

    #[test]
    fn optional_history_cannot_displace_required_head_code_or_hunks() {
        let make_entry=|i|json!({"id":format!("{i:064x}"),"revision":"a".repeat(64),"side":"head","repository_id":"repo","name":format!("F{i}"),"text":"return quote(); ".repeat(10),"reasons":[{"reason":"change_seed"}]});
        let plain=json!({"entries":(0..23).map(make_entry).collect::<Vec<_>>(),"diffs":[{"file":"app.go","before":"return 1","after":"return 2"}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (baseline,_)=pack(plain.clone(),8000,None,None).unwrap();
        assert_eq!(baseline["coverage"]["current_head"]["selected"],23);
        let mut enriched=plain;
        enriched["business_context"]=json!({"complete":false});
        for (i,entry) in enriched["entries"].as_array_mut().unwrap().iter_mut().enumerate() {
            entry["business_context"]=if i==1 {
                json!({"evolution":{"function_history":[{"subject":"Fix tenant isolation","commit":"b".repeat(40)}]}})
            } else {json!({"historical_assertions":[{"subject":"optional detail ".repeat(4000)}]})};
        }
        let (full,previous)=pack(enriched.clone(),8000,None,None).unwrap();
        assert_eq!(full["coverage"]["current_head"]["selected"],23);
        assert_eq!(full["diffs"].as_array().unwrap().len(),1);
        assert!(full["entries"].as_array().unwrap().iter().any(|e|e["business_context"]["evolution"]["function_history"].as_array().is_some_and(|h|!h.is_empty())));
        assert_eq!(full["business_context"]["packing"]["omitted_entries"],22);
        let (delta,_)=pack(enriched,8000,Some(&previous),None).unwrap();
        assert_eq!(delta["context_order"],full["context_order"]);
        assert_eq!(delta["unchanged"].as_array().unwrap().len(),24);
        for packet in [&full,&delta] {assert!(text::count(&packet.to_string())<=8000);}
    }

    #[test]
    fn budgeted_deltas_reserve_unchanged_and_invalidated_records() {
        let entry = |i: usize, rev: &str| json!({"id":format!("{i:064x}"),"revision":rev,"repository_id":"repo","text":"return calculate(); ".repeat(40)});
        let report = |entries: Vec<Value>| json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (_, previous) = super::pack(
            report((0..24).map(|i| entry(i, "old")).collect()),
            16000,
            None,
            None,
        )
        .unwrap();
        for budget in [4096, 6000, 8000] {
            let next = report(
                (0..12)
                    .map(|i| entry(i, "old"))
                    .chain((12..60).map(|i| entry(i, "new")))
                    .collect(),
            );
            let (full, _) = super::pack(next.clone(), budget, None, None).unwrap();
            let (packet, receipt) = super::pack(next, budget, Some(&previous), None).unwrap();
            assert!(text::count(&packet.to_string()) <= budget);
            assert_eq!(packet["context_order"], full["context_order"]);
            for old in previous["entries"].as_array().unwrap() {
                assert!(
                    receipt["entries"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|e| e["id"] == old["id"])
                        || packet["invalidated"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|e| e["id"] == old["id"])
                );
            }
            assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
        }
    }

    #[test]
    fn reverted_change_invalidates_the_previously_supplied_diff() {
        use serde_json::json;
        let report = json!({"entries":[],"diffs":[{"file":"app/main.go","before":"return 1","after":"return 2"}],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"issues":[]}});
        let (first, receipt) = super::pack(report.clone(), 4000, None, None).unwrap();
        let (repeated, _) = super::pack(report.clone(), 4000, Some(&receipt), None).unwrap();
        assert!(repeated["diffs"].as_array().unwrap().is_empty());
        assert_eq!(repeated["unchanged"], json!([first["diffs"][0]["id"]]));
        let mut reverted = report;
        reverted["diffs"] = json!([]);
        let (reverted, _) = super::pack(reverted, 4000, Some(&receipt), None).unwrap();
        assert_eq!(reverted["invalidated"][0]["id"], first["diffs"][0]["id"]);
        assert_eq!(reverted["invalidated"][0]["collection"], "diffs");
    }

    #[test]
    fn unsupported_inputs_include_extensionless_scripts_and_build_definitions() {
        for path in [
            "run.sh",
            "run.js",
            "run.ts",
            "Makefile",
            "Dockerfile",
            "service.proto",
        ] {
            assert!(super::unsupported_source(path, ""), "{path}");
        }
        assert!(super::unsupported_source("run", "#!/bin/bash\necho test\n"));
        assert!(super::unsupported_source("Makefile", "all: test"));
        assert!(!super::unsupported_source("go.mod", "module example/app"));
        assert!(!super::unsupported_source("main.go", "package app"));
        assert!(!super::unsupported_source("README.md", "Documentation"));
    }
}
