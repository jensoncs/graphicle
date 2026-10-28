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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
}
#[derive(Clone)]
struct Source {
    repository: String,
    path: String,
    sha: String,
    content: String,
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
}
#[derive(Deserialize)]
struct World {
    root: String,
    symbols: Vec<Symbol>,
    calls: Vec<Call>,
    packages: Vec<Value>,
    issues: Vec<String>,
}
#[derive(Deserialize)]
struct Facts {
    analyzer: String,
    toolchain: String,
    worlds: Vec<World>,
}
struct Analysis {
    symbols: BTreeMap<String, (String, Symbol)>,
    calls: Vec<(String, Call)>,
    packages: Vec<Value>,
    issues: Vec<Value>,
    analyzer: String,
    toolchain: String,
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
fn load(store: &Store, workspace: &str, token: &str, pins: &[Pin]) -> Result<Input> {
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
            if !complete {input.issues.push(issue("source_admission_incomplete",json!(pin.directory)));}
            let files=manifest.as_object().ok_or_else(||Error::bad("Invalid stored source manifest"))?;
            for (path,entry) in files {
                store::path(path)?;
                if path.split('/').any(|p|local::sensitive(p)||p.starts_with(".env")||p=="secrets"||local::SKIPPED_NAMES.contains(&p))||local::skipped_file(path) {input.issues.push(issue("excluded_source",json!(pin.directory)));continue}
                let size=entry["bytes"].as_u64().ok_or_else(||Error::bad("Invalid source size"))?;
                if size>MAX_SOURCE_BYTES as u64||bytes+size as usize>MAX_SOURCE_BYTES||input.sources.len()>=MAX_FILES{return Err(Error::bad("Review source bound exceeded (64 MiB / 20000 files per side)"));}
                let source=store::one(db,"SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?",params![pin.repository_id,s(entry,"sha256")])?;
                let Some(source)=source else {input.issues.push(issue("missing_source_object",json!(pin.directory)));continue};
                let content=s(&source,"content");if content.len()!=size as usize||text::sha256(content.as_bytes())!=s(entry,"sha256"){return Err(Error::conflict("Pinned source failed integrity verification"));}
                bytes+=content.len();input.sources.insert(format!("{}/{}",pin.directory,path),Source{repository:pin.repository_id.clone(),path:path.clone(),sha:s(entry,"sha256").into(),content:content.into()});
            }
        }
        Ok(input)
    })
}
fn analyze(input: &Input, profile: &Profile) -> Result<Analysis> {
    let tree = tempfile::tempdir()?;
    for pin in &input.pins {
        std::fs::create_dir(tree.path().join(&pin.directory))?;
    }
    for (path, source) in &input.sources {
        let target = tree.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap())?;
        local::write_private(&target, source.content.as_bytes())?;
    }
    let roots: Vec<_> = input
        .pins
        .iter()
        .flat_map(|p| {
            p.modules
                .iter()
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
    };
    let facts = match graphicle::extractors::go_types::review(tree.path(), &request) {
        Ok(v) => serde_json::from_value::<Facts>(v)?,
        Err(_) => {
            out.issues
                .push(issue("compiler_analysis_unavailable", Value::Null));
            return Ok(out);
        }
    };
    out.analyzer = facts.analyzer;
    out.toolchain = facts.toolchain;
    for world in facts.worlds {
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
                    .get(symbol.start..symbol.end)
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
    if out.symbols.len() > 100_000 || out.calls.len() > 500_000 {
        return Err(Error::bad("Review graph bound exceeded"));
    }
    Ok(out)
}

fn change_range(before: &str, after: &str) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = before.as_bytes()[prefix..]
        .iter()
        .rev()
        .zip(after.as_bytes()[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - suffix)
        || !after.is_char_boundary(after.len() - suffix)
    {
        suffix -= 1;
    }
    (prefix..before.len() - suffix, prefix..after.len() - suffix)
}
fn touches(s: &Symbol, range: &std::ops::Range<usize>) -> bool {
    if range.is_empty() {
        s.start <= range.start && range.start <= s.end
    } else {
        s.start < range.end && range.start < s.end
    }
}
fn graph(analysis: &Analysis, reverse: bool) -> Result<Graph> {
    let mut graph = Graph::new(true, true);
    for (id, (_, symbol)) in &analysis.symbols {
        graph.add_node(
            json!(id),
            json!({"label":symbol.name}).as_object().unwrap().clone(),
        )?;
    }
    for (world, call) in &analysis.calls {
        let mut from = key(world, &call.caller);
        let mut to = key(world, &call.target);
        if !call.reason.is_empty()
            || !analysis.symbols.contains_key(&from)
            || !analysis.symbols.contains_key(&to)
        {
            continue;
        };
        if reverse {
            std::mem::swap(&mut from, &mut to)
        };
        graph.add_edge(json!(from),json!(to),None,json!({"relation":call.relation,"source_file":call.file,"source_location":format!("L{}",call.line),"byte_start":call.start,"byte_end":call.end,"spelling":call.spelling,"caller":key(world,&call.caller),"callee":key(world,&call.target),"evidence":"go/types"}).as_object().unwrap().clone())?;
    }
    Ok(graph)
}

struct Selection {
    reasons: BTreeMap<String, Vec<Value>>,
    issues: Vec<Value>,
}
fn select(analysis: &Analysis, seeds: &BTreeSet<String>, depth: usize) -> Result<Selection> {
    let mut out = Selection {
        reasons: BTreeMap::new(),
        issues: vec![],
    };
    let relations = vec!["calls".into(), "uses_type".into()];
    let mut paths_emitted = 0usize;
    for reverse in [false, true] {
        let graph = graph(analysis, reverse)?;
        for (seed_index, seed) in seeds.iter().enumerate() {
            if seed_index >= 1000 || paths_emitted >= 20_000 {
                out.issues.push(issue("traversal_budget",json!({"remaining_seeds":seeds.len()-seed_index,"direction":if reverse{"outgoing"}else{"incoming"}})));
                // All changed symbols still compete for packet space.
                for seed in seeds {
                    out.reasons
                        .entry(seed.clone())
                        .or_default()
                        .push(json!({"reason":"change_seed","path":[seed]}));
                }
                break;
            }
            out.reasons
                .entry(seed.clone())
                .or_default()
                .push(json!({"reason":"change_seed","path":[seed]}));
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
                let Some((predecessor, evidence)) = predecessor else {
                    out.issues
                        .push(issue("path_unavailable", json!(hit.node_id)));
                    continue;
                };
                let mut path = paths[&predecessor].clone();
                path.push(hit.node_id.clone());
                paths.insert(hit.node_id.clone(), path.clone());
                let mut steps = evidence_paths[&predecessor].clone();
                steps.push(evidence);
                evidence_paths.insert(hit.node_id.clone(), steps.clone());
                out.reasons.entry(hit.node_id).or_default().push(json!({"reason":if reverse{"dependency"}else{"potential_consumer"},"path":path,"steps":steps}));
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
        "signature_revision":symbol.signature,"implementation_revision":symbol.implementation,"text":&source.content[symbol.start..symbol.end],"reasons":reasons,"test":source.path.ends_with("_test.go")});
    value["revision"] = json!(crate::digest(&value));
    value
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
                changes.push(json!({"kind":"modified","base":id,"head":id,"signature_changed":old.signature!=new.signature,"implementation_changed":old.implementation!=new.implementation}));
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
                changes.push(json!({"kind":"moved_or_renamed","base":id,"head":next,"evidence":"unique_implementation_and_signature"}));
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
    for file in files {
        let old = base.sources.get(&file);
        let new = head.sources.get(&file);
        if old.map(|s| &s.sha) == new.map(|s| &s.sha) {
            continue;
        }
        let before = old.map(|s| s.content.as_str()).unwrap_or("");
        let after = new.map(|s| s.content.as_str()).unwrap_or("");
        let (range_a, range_b) = change_range(before, after);
        diffs.push(json!({"file":file,"base_sha256":old.map(|s|&s.sha),"head_sha256":new.map(|s|&s.sha),"base_bytes":[range_a.start,range_a.end],"head_bytes":[range_b.start,range_b.end],"before":&before[range_a.clone()],"after":&after[range_b.clone()]}));
        for (analysis, seeds, source, range) in [
            (a, &mut seeds_a, old, &range_a),
            (b, &mut seeds_b, new, &range_b),
        ] {
            let mut mapped = false;
            for (id, (_, symbol)) in &analysis.symbols {
                if symbol.file == file && touches(symbol, range) {
                    seeds.insert(id.clone());
                    mapped = true;
                }
            }
            if !mapped && let Some(source) = source {
                let worlds: BTreeSet<_> = analysis
                    .packages
                    .iter()
                    .filter(|p| {
                        p["files"]
                            .as_array()
                            .is_some_and(|files| files.iter().any(|f| f == &file))
                    })
                    .map(|p| s(p, "consumer_build"))
                    .collect();
                for (id, (world, symbol)) in &analysis.symbols {
                    let input = if std::ptr::eq(analysis, a) {
                        base
                    } else {
                        head
                    };
                    if input.sources[&symbol.file].repository == source.repository
                        || worlds.contains(world.as_str())
                    {
                        seeds.insert(id.clone());
                    }
                }
            }
        }
    }
    // A native resolution input change can affect unchanged callers. Compare
    // package bindings without source hashes; source changes are seeded above.
    let bindings = |analysis: &Analysis| -> Value {
        json!(analysis.packages.iter().map(|p|json!({"consumer_build":p["consumer_build"],"path":p["path"],"directory":p["directory"],"module":p["module"],"version":p["version"],"replacement":p["replacement"],"imports":p["imports"]})).collect::<Vec<_>>())
    };
    let dependency_changed = bindings(a) != bindings(b)
        || base
            .pins
            .iter()
            .map(|p| (&p.repository_id, &p.directory, &p.modules, &p.policy_sha256))
            .collect::<Vec<_>>()
            != head
                .pins
                .iter()
                .map(|p| (&p.repository_id, &p.directory, &p.modules, &p.policy_sha256))
                .collect::<Vec<_>>();
    if dependency_changed {
        seeds_a.extend(a.symbols.keys().cloned());
        seeds_b.extend(b.symbols.keys().cloned());
    }
    let selected_a = select(a, &seeds_a, r.max_depth)?;
    let selected_b = select(b, &seeds_b, r.max_depth)?;
    let mut issues = a.issues.clone();
    issues.extend(b.issues.clone());
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
        let bindings:BTreeMap<_,_>=analysis.packages.iter().map(|p|((s(p,"consumer_build"),s(p,"path")),json!({"instance":p["instance"],"consumer_build":p["consumer_build"],"module":p["module"],"version":p["version"],"replacement":p["replacement"]}))).collect();
        for (id, reasons) in selection {
            let (world, symbol) = &analysis.symbols[&id];
            entries.push(entry(
                side,
                &id,
                symbol,
                &input.sources[&symbol.file],
                &reasons,
                bindings
                    .get(&(world.as_str(), symbol.package.as_str()))
                    .unwrap_or(&Value::Null),
            ));
        }
    }
    entries.sort_by_key(|e| {
        (
            !e["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == "change_seed"),
            s(e, "side").to_string(),
            s(e, "id").to_string(),
        )
    });
    for (side, input) in [("base", base), ("head", head)] {
        for (path, source) in &input.sources {
            if unsupported_source(path, &source.content) {
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
        "diffs":diffs,"entries":entries,"package_instances":{"base":a.packages,"head":b.packages},"coverage":{"complete":complete,"scope":"supplied Go snapshot set; static calls and type uses","behavioral_propagation_proven":false,"issues":issues,"max_depth":r.max_depth},
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

fn pack(mut report: Value, max_tokens: usize, previous: Option<&Value>) -> Result<(Value, Value)> {
    let entries = report["entries"].as_array().unwrap().clone();
    report["entries"] = json!([]);
    // Keep full package metadata in the analysis identity while bounding its
    // wire representation. A package count is never a claim of completeness.
    let instances = report["package_instances"].clone();
    report["package_instances"] = json!({"digest":crate::digest(&instances),"base_count":instances["base"].as_array().unwrap().len(),"head_count":instances["head"].as_array().unwrap().len()});
    let mut diffs = report["diffs"].as_array().unwrap().clone();
    for diff in &mut diffs {
        diff["id"] = json!(crate::digest(&json!(["diff", diff["file"]])));
        diff["revision"] = json!(crate::digest(diff));
    }
    report["coverage"]["required_diffs"] = json!(diffs.len());
    report["diffs"] = json!([]);
    let issues = report["coverage"]["issues"].as_array().unwrap().clone();
    report["coverage"]["issue_count"] = json!(issues.len());
    report["coverage"]["issues"] = json!(issues.iter().take(20).collect::<Vec<_>>());
    let total_changes = report["changes"].as_array().unwrap().len();
    let changes_digest = crate::digest(&report["changes"]);
    report["changes"] = json!({"count":total_changes,"digest":changes_digest,"examples":report["changes"].as_array().unwrap().iter().take(20).collect::<Vec<_>>()});
    report["coverage"]["budget_omitted"] = json!(0);
    report["coverage"]["required_entries"] = json!(entries.len());
    let old: BTreeMap<String, Value> = previous
        .and_then(|p| p["entries"].as_array())
        .into_iter()
        .flatten()
        .map(|v| (s(v, "id").into(), v.clone()))
        .collect();
    let mut receipts = vec![];
    let mut unchanged = vec![];
    let mut candidates = vec![];
    // Classify the delta before allocating any excerpt budget. All control
    // records must fit even if every changed excerpt is omitted.
    for (collection, mut entry) in diffs
        .into_iter()
        .map(|v| ("diffs", v))
        .chain(entries.into_iter().map(|v| ("entries", v)))
    {
        let id = s(&entry, "id").to_string();
        let revision = s(&entry, "revision").to_string();
        let receipt = json!({"id":id,"revision":revision,"collection":collection,"repository_id":entry["repository_id"]});
        if old.get(&id).is_some_and(|e| e["revision"] == revision) {
            unchanged.push(id);
            receipts.push(receipt);
        } else {
            entry["update"] = json!(if old.contains_key(&id) {
                "updated"
            } else {
                "added"
            });
            candidates.push((collection, entry, receipt));
        }
    }
    let retained: BTreeSet<_> = unchanged.iter().map(String::as_str).collect();
    let mut invalidated: BTreeMap<String, Value> = old.values()
        .filter(|v| !retained.contains(s(v, "id")))
        .map(|v| (s(v, "id").to_string(), json!({"id":v["id"],"revision":v["revision"],"collection":v["collection"],"reason":"changed_removed_or_omitted"})))
        .collect();
    report["previous_packet_id"] = previous
        .map(|p| p["packet_id"].clone())
        .unwrap_or(Value::Null);
    report["invalidated"] = json!(invalidated.values().collect::<Vec<_>>());
    report["unchanged"] = json!(unchanged);
    // Tokenize each candidate once, rather than cloning/tokenizing the growing
    // packet for every symbol. Reserve room for JSON boundaries and final fields.
    let mut used = text::count(&report.to_string()) + 256;
    let mut omitted = 0;
    for (collection, entry, receipt) in candidates {
        let id = s(&entry, "id");
        let removed = invalidated
            .get(id)
            .map_or(0, |v| text::count(&v.to_string()).saturating_sub(8));
        let cost = text::count(&entry.to_string()) + 16;
        let trial = used.saturating_sub(removed).saturating_add(cost);
        if trial <= max_tokens {
            used = trial;
            invalidated.remove(id);
            report[collection].as_array_mut().unwrap().push(entry);
            receipts.push(receipt);
        } else {
            omitted += 1;
        }
    }
    report["invalidated"] = json!(invalidated.values().collect::<Vec<_>>());
    report["coverage"]["budget_omitted"] = json!(omitted);
    if omitted > 0 {
        report["coverage"]["complete"] = json!(false);
        report["impact"] = json!("incomplete");
    }
    // Deltas have to carry all invalidations. Refuse rather than silently drop
    // one if the client chooses a budget smaller than this control metadata.
    report["packet_id"] = json!(crate::digest(&report));
    report["tokens"] = json!(text::count(&report.to_string()) + 8);
    if text::count(&report.to_string()) > max_tokens {
        return Err(Error::bad(
            "Token budget too small for review metadata and invalidations; increase it or request a full packet",
        ));
    }
    let receipt = json!({"packet_id":report["packet_id"],"entries":receipts,"base":report["base"],"head":report["head"],"profile":report["profile"]});
    Ok((report, receipt))
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
        for side in ["base","head"] { for pin in receipt[side].as_array().ok_or_else(||Error::conflict("Invalid review receipt"))? { store::repo(db,p,s(pin,"repository_id"))?; } }
        Ok(Some(receipt))
    })?;
    let base = load(store, workspace, token, &r.base)?;
    let head = load(store, workspace, token, &r.head)?;
    let a = analyze(&base, &r.profile)?;
    let b = analyze(&head, &r.profile)?;
    let mut report = compare(&base, &head, &a, &b, &r)?;
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
    let (report, receipt) = pack(report, r.max_tokens, previous.as_ref())?;
    store.authorized(workspace,token,"read",|db,p|{
        store::repo(db,p,anchor)?;
        for repository in base.sources.values().chain(head.sources.values()).map(|s|&s.repository).collect::<BTreeSet<_>>(){store::repo(db,p,repository)?;}
        if let Some(previous)=&previous {for side in ["base","head"] {for pin in previous[side].as_array().unwrap() {store::repo(db,p,s(pin,"repository_id"))?;}}}
        // Source forgetting/retention may run while the compiler is working.
        // Do not serve materialized bytes that have since been removed.
        let mut exists=db.prepare_cached("SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)")?;
        for source in base.sources.values().chain(head.sources.values()) {
            if !exists.query_row(params![source.repository,source.sha,source.content.len()],|row|row.get::<_,bool>(0))? {return Err(Error::conflict("Pinned source was removed during review; retry with an available snapshot"));}
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
pub fn properties() -> Value {
    let pin = json!({"type":"object","properties":{"repository_id":{"type":"string"},"commit":{"type":"string"},"snapshot":{"type":"string"},"policy_sha256":{"type":"string"},"directory":{"type":"string"},"modules":{"type":"array","items":{"type":"string"}}},"required":["repository_id","commit","snapshot","policy_sha256","directory"],"additionalProperties":false});
    json!({"base":{"type":"array","items":pin,"minItems":1,"maxItems":64},"head":{"type":"array","items":pin,"minItems":1,"maxItems":64},"profile":{"type":"object","properties":{"goos":{"type":"string"},"goarch":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"tests":{"type":"boolean"},"workspace":{"type":"string"}},"required":["goos","goarch"],"additionalProperties":false},"max_depth":{"type":"integer","minimum":0,"maximum":32},"max_tokens":{"type":"integer","minimum":512,"maximum":64000},"previous_packet_id":{"type":"string"},"repo":{"type":"string"}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_review_packets_remain_bounded() {
        let entries: Vec<_> = (0..20_000).map(|i| json!({"id":format!("{i:064x}"),"revision":"r","repository_id":"repo","text":"func F() int { return 1 }\n".repeat(8)})).collect();
        let report = json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let start = std::time::Instant::now();
        let (packet, _) = super::pack(report, 16_000, None).unwrap();
        eprintln!("packed 20,000 candidates in {:?}", start.elapsed());
        assert!(text::count(&packet.to_string()) <= 16_000);
        assert_eq!(packet["coverage"]["required_entries"], 20_000);
        assert!(packet["coverage"]["budget_omitted"].as_u64().unwrap() > 0);
        assert_eq!(packet["impact"], "incomplete");
    }

    #[test]
    fn budgeted_deltas_reserve_unchanged_and_invalidated_records() {
        let entry = |i: usize, rev: &str| json!({"id":format!("{i:064x}"),"revision":rev,"repository_id":"repo","text":"return calculate(); ".repeat(40)});
        let report = |entries: Vec<Value>| json!({"entries":entries,"diffs":[],"changes":[],"package_instances":{"base":[],"head":[]},"coverage":{"complete":true,"issues":[]},"impact":"potential_impact"});
        let (_, previous) = super::pack(
            report((0..24).map(|i| entry(i, "old")).collect()),
            16000,
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
            let (packet, receipt) = super::pack(next, budget, Some(&previous)).unwrap();
            assert!(text::count(&packet.to_string()) <= budget);
            assert_eq!(packet["unchanged"].as_array().unwrap().len(), 12);
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
        let (first, receipt) = super::pack(report.clone(), 4000, None).unwrap();
        let (repeated, _) = super::pack(report.clone(), 4000, Some(&receipt)).unwrap();
        assert!(repeated["diffs"].as_array().unwrap().is_empty());
        assert_eq!(repeated["unchanged"], json!([first["diffs"][0]["id"]]));
        let mut reverted = report;
        reverted["diffs"] = json!([]);
        let (reverted, _) = super::pack(reverted, 4000, Some(&receipt)).unwrap();
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
