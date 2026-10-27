//! Go calls require compiler identities, never a same-named declaration.
use super::{
    parse_cache,
    resolution::{s, source_key},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

fn helper() -> Option<&'static Path> {
    static HELPER: OnceLock<Option<(tempfile::TempDir, PathBuf)>> = OnceLock::new();
    HELPER
        .get_or_init(|| {
            let dir = tempfile::tempdir().ok()?;
            let source = dir.path().join("main.go");
            let binary = dir.path().join(if cfg!(windows) {
                "go-types.exe"
            } else {
                "go-types"
            });
            std::fs::write(&source, include_str!("go_types.go")).ok()?;
            let host = crate::process::capture(
                Command::new("go")
                    .args(["env", "-json", "GOHOSTOS", "GOHOSTARCH"])
                    .current_dir(dir.path())
                    .env("GOTOOLCHAIN", "local"),
                Duration::from_secs(5),
            )
            .ok()?;
            if !host.success {
                return None;
            }
            let host: Value = serde_json::from_slice(&host.stdout).ok()?;
            let result = crate::process::capture(
                Command::new("go")
                    .args(["build", "-o"])
                    .arg(&binary)
                    .arg(&source)
                    .current_dir(dir.path())
                    .env("GOWORK", "off")
                    .env("GO111MODULE", "off")
                    .env("GOTOOLCHAIN", "local")
                    .env("GOFLAGS", "")
                    .env("GOOS", host["GOHOSTOS"].as_str()?)
                    .env("GOARCH", host["GOHOSTARCH"].as_str()?),
                Duration::from_secs(60),
            )
            .ok()?;
            result.success.then_some((dir, binary))
        })
        .as_ref()
        .map(|(_, path)| path.as_path())
}

#[derive(Deserialize)]
struct Site {
    file: String,
    line: usize,
    column: usize,
    caller: String,
    caller_line: usize,
    #[serde(default)]
    target_file: String,
    #[serde(default)]
    target_line: usize,
    #[serde(default)]
    target: String,
    #[serde(default)]
    reason: String,
}

pub fn resolve(
    paths: &[PathBuf],
    nodes: &[Value],
    context: &[Value],
    edges: &mut Vec<Value>,
    root: &Path,
) -> Vec<Value> {
    let paths: Vec<_> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "go"))
        .collect();
    if paths.is_empty() {
        return vec![];
    }
    let files: BTreeSet<_> = paths
        .iter()
        .map(|p| source_key(&p.to_string_lossy(), root))
        .collect();
    // Remove all legacy guesses, including same-file and receiver passes.
    edges.retain(|e| {
        e["relation"] != "calls" || !files.contains(&source_key(s(e, "source_file"), root))
    });
    let mut unresolved = BTreeMap::new();
    for path in &paths {
        if let Some(parsed) = parse_cache::parse(path, "go", tree_sitter_go::LANGUAGE.into()) {
            let mut pending = vec![parsed.tree.root_node()];
            while let Some(node) = pending.pop() {
                if node.kind() == "call_expression" {
                    let row = node.start_position();
                    let key = (
                        source_key(&path.to_string_lossy(), root),
                        row.row + 1,
                        row.column + 1,
                    );
                    unresolved.insert(key.clone(), json!({"source_file":key.0,"source_location":format!("L{}",key.1),"column":key.2,"language":"go","reason":"type_information_unavailable"}));
                }
                let mut cursor = node.walk();
                pending.extend(node.named_children(&mut cursor));
            }
        }
    }
    let sites = helper().and_then(|binary| {
        let output = crate::process::capture_with_input(
            Command::new(binary)
                .current_dir(root)
                .env("GOTOOLCHAIN", "local"),
            Duration::from_secs(60),
            Some(serde_json::to_vec(&json!({"Paths": paths})).ok()?),
        )
        .ok()?;
        output
            .success
            .then(|| serde_json::from_slice::<Vec<Site>>(&output.stdout).ok())
            .flatten()
    });
    // File + declaration line + name must select exactly one graph node.
    let mut declarations: BTreeMap<(String, usize, String), BTreeSet<String>> = BTreeMap::new();
    for n in nodes.iter().chain(context) {
        let Some(line) = s(n, "source_location")
            .strip_prefix('L')
            .and_then(|l| l.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(name) = s(n, "label").trim_start_matches('.').strip_suffix("()") else {
            continue;
        };
        declarations
            .entry((source_key(s(n, "source_file"), root), line, name.into()))
            .or_default()
            .insert(s(n, "id").into());
    }
    let declaration = |file: &str, line, name: &str| {
        let ids = declarations.get(&(source_key(file, root), line, name.into()))?;
        (ids.len() == 1).then(|| ids.first().unwrap().clone())
    };
    let mut have = BTreeSet::new();
    for site in sites.into_iter().flatten() {
        let key = (source_key(&site.file, root), site.line, site.column);
        if !files.contains(&key.0) {
            continue;
        }
        let from = declaration(&site.file, site.caller_line, &site.caller);
        let to = declaration(&site.target_file, site.target_line, &site.target);
        if site.reason.is_empty()
            && let (Some(from), Some(to)) = (from, to)
        {
            if have.insert((from.clone(), to.clone())) {
                edges.push(json!({"source":from,"target":to,"relation":"calls","confidence":"EXTRACTED","weight":1.0,"context":"call","source_file":site.file,"source_location":format!("L{}",site.line),"resolution":"go_types","evidence":"type"}));
            }
            unresolved.remove(&key);
        } else if let Some(value) = unresolved.get_mut(&key) {
            value["reason"] = json!(if site.reason.is_empty() {
                "target_not_in_graph"
            } else {
                &site.reason
            });
        }
    }
    unresolved.into_values().collect()
}
