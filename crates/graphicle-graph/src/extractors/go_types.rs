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

// One environment policy for helper compilation, extraction and pinned review.
// Explicit private-module overrides must not bypass GOPROXY=off.
fn offline_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env("GOTOOLCHAIN", "local")
        .env("GOENV", "off")
        .env("GOFLAGS", "")
        .env("GOWORK", "off")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .env("GOVCS", "*:off")
        .env("GOPRIVATE", "")
        .env("GONOPROXY", "")
        .env("GONOSUMDB", "")
        .env("GOINSECURE", "")
        .env("GOEXPERIMENT", "")
        .env("CGO_ENABLED", "0");
    command
}

fn helper() -> Option<(tempfile::TempDir, PathBuf)> {
    // Rust statics are not dropped at exit. Cache bytes, not a TempDir whose
    // destructor would never run; executable scratch files live only for a call.
    static HELPER: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let bytes = HELPER
        .get_or_init(|| {
            let dir = tempfile::tempdir().ok()?;
            let source = dir.path().join("main.go");
            let binary = dir.path().join(if cfg!(windows) {
                "go-types.exe"
            } else {
                "go-types"
            });
            std::fs::write(&source, include_str!("go_types.go")).ok()?;
            let review = dir.path().join("review.go");
            std::fs::write(&review, include_str!("go_review.go")).ok()?;
            let host = crate::process::capture(
                offline_command("go")
                    .args(["env", "-json", "GOHOSTOS", "GOHOSTARCH"])
                    .current_dir(dir.path()),
                Duration::from_secs(5),
            )
            .ok()?;
            if !host.success {
                return None;
            }
            let host: Value = serde_json::from_slice(&host.stdout).ok()?;
            let result = crate::process::capture(
                offline_command("go")
                    .args(["build", "-o"])
                    .arg(&binary)
                    .arg(&source)
                    .arg(&review)
                    .current_dir(dir.path())
                    .env("GO111MODULE", "off")
                    .env("GOOS", host["GOHOSTOS"].as_str()?)
                    .env("GOARCH", host["GOHOSTARCH"].as_str()?),
                Duration::from_secs(60),
            )
            .ok()?;
            result.success.then(|| std::fs::read(binary).ok()).flatten()
        })
        .as_ref()?;
    let dir = tempfile::tempdir().ok()?;
    let binary = dir.path().join(if cfg!(windows) {
        "go-types.exe"
    } else {
        "go-types"
    });
    std::fs::write(&binary, bytes).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).ok()?;
    }
    Some((dir, binary))
}

/// Compiler facts for an isolated, explicitly pinned Go snapshot set. The
/// helper shares go/types resolution with extraction, but records build inputs,
/// exact declarations and every call site instead of graph-name guesses.
pub fn review(root: &Path, request: &Value) -> crate::Result<Value> {
    let (_helper_dir, binary) = helper().ok_or("Go compiler helper unavailable")?;
    let cache = tempfile::tempdir()?;
    let output = crate::process::capture_bounded(
        offline_command(&binary)
            .current_dir(root)
            .env("GOMODCACHE", cache.path().join("modules"))
            .env("GOCACHE", cache.path().join("build")),
        Duration::from_secs(120),
        Some(serde_json::to_vec(&json!({"review":request}))?),
        64 * 1024 * 1024,
    )?;
    if !output.success || output.stdout.len() > 64 * 1024 * 1024 {
        return Err("Go review analysis failed or exceeded its output bound".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
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
    let sites = (|| -> Result<Vec<Site>, &str> {
        let (_helper_dir, binary) = helper().ok_or("go_helper_unavailable")?;
        let cache = tempfile::tempdir().map_err(|_| "go_scratch_unavailable")?;
        let workspace = root.join("go.work");
        let workspace = if workspace.is_file() {
            workspace.into_os_string()
        } else {
            "off".into()
        };
        let output = crate::process::capture_bounded(
            offline_command(&binary)
                .current_dir(root)
                .env("GOWORK", workspace)
                .env("GO111MODULE", "auto")
                .env("GOMODCACHE", cache.path().join("modules"))
                .env("GOCACHE", cache.path().join("build")),
            Duration::from_secs(60),
            Some(serde_json::to_vec(&json!({"Paths": paths})).map_err(|_| "go_request_invalid")?),
            64 * 1024 * 1024,
        )
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                "go_analysis_timeout"
            } else {
                "go_analysis_io_failure"
            }
        })?;
        if !output.success {
            return Err("go_analysis_failed");
        }
        serde_json::from_slice::<Vec<Site>>(&output.stdout)
            .map_err(|_| "go_analysis_invalid_output")
    })();
    if let Err(reason) = &sites {
        for value in unresolved.values_mut() {
            value["reason"] = json!(reason);
        }
    }
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
