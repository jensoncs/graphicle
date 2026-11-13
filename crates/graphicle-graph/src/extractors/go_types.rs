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
    sync::{Mutex, OnceLock, TryLockError},
    time::{Duration, Instant},
};

include!(concat!(env!("OUT_DIR"), "/go_export_sources.rs"));

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

/// Where a helper built from these sources with this toolchain is kept, so a
/// later process skips `go build` (about 4 s). Keyed by [`review_identity`]:
/// every embedded helper source, `GOVERSION`, `GOROOT` and the host platform.
fn helper_cache_file() -> Option<PathBuf> {
    use sha2::Digest;
    let identity = review_identity().ok()?;
    let key = format!(
        "{:x}",
        sha2::Sha256::digest(identity.to_string().as_bytes())
    );
    let store = crate::global_graph::GlobalStore::default_location().ok()?;
    Some(store.directory.join("go-helper").join(&key[..32]))
}

/// The cached helper at `path`, only when this user alone could have written
/// it and its folder: the bytes are executed.
fn read_cached_helper(path: &Path) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = nix::unistd::Uid::current().as_raw();
        for p in [path, path.parent()?] {
            let m = std::fs::symlink_metadata(p).ok()?;
            if m.file_type().is_symlink() || m.uid() != uid || m.mode() & 0o022 != 0 {
                return None;
            }
        }
    }
    std::fs::read(path).ok()
}

/// Keep `bytes` at `path`, atomically, in a folder only this user can write.
/// A failure only costs the next process a build.
fn store_cached_helper(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let Some(dir) = path.parent() else { return };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    if builder.create(dir).is_err() {
        return;
    }
    // A named temporary file is created readable and writable by its owner only.
    let Ok(mut file) = tempfile::NamedTempFile::new_in(dir) else {
        return;
    };
    if file.write_all(bytes).is_ok() {
        let _ = file.persist(path);
    }
}

fn helper() -> Option<(tempfile::TempDir, PathBuf)> {
    // Rust statics are not dropped at exit. Cache bytes, not a TempDir whose
    // destructor would never run; executable scratch files live only for a call.
    static HELPER: OnceLock<Vec<u8>> = OnceLock::new();
    static INITIALIZING: Mutex<()> = Mutex::new(());
    // A compile that failed with budget left is remembered for FAILED_RETRY, so a
    // slow-failing toolchain does not cost every review its budget; a compile the
    // deadline cut short stays retryable at once in a retained server.
    static FAILED_AT: Mutex<Option<Instant>> = Mutex::new(None);
    const FAILED_RETRY: Duration = Duration::from_secs(600);
    // Waiting for another initializer consumes this request's budget too.
    if HELPER.get().is_none() {
        let guard = loop {
            crate::process::check_deadline().ok()?;
            match INITIALIZING.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        if HELPER.get().is_none() {
            let failed = *FAILED_AT.lock().unwrap_or_else(|error| error.into_inner());
            if failed.is_some_and(|at| at.elapsed() < FAILED_RETRY) {
                return None;
            }
            let compile = || {
                let cached = helper_cache_file();
                if let Some(bytes) = cached.as_deref().and_then(read_cached_helper) {
                    return Some(bytes);
                }
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
                let semantic = dir.path().join("semantic.go");
                std::fs::write(&semantic, include_str!("go_semantic.go")).ok()?;
                let package_cache = dir.path().join("cache.go");
                std::fs::write(&package_cache, include_str!("go_review_cache.go")).ok()?;
                for (name, source) in GO_EXPORT_SOURCES {
                    let target = dir.path().join(name);
                    std::fs::create_dir_all(target.parent()?).ok()?;
                    std::fs::write(target, source).ok()?;
                }
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
                        .args(["build", "-mod=vendor", "-o"])
                        .arg(&binary)
                        .arg(&source)
                        .arg(&review)
                        .arg(&semantic)
                        .arg(&package_cache)
                        .current_dir(dir.path())
                        .env("GO111MODULE", "on")
                        .env("GOMODCACHE", dir.path().join("modules"))
                        // Helper compilation must also work in service/benchmark
                        // environments without HOME or a writable user cache.
                        .env("GOCACHE", dir.path().join("build"))
                        .env("GOOS", host["GOHOSTOS"].as_str()?)
                        .env("GOARCH", host["GOHOSTARCH"].as_str()?),
                    Duration::from_secs(60),
                )
                .ok()?;
                let bytes = result
                    .success
                    .then(|| std::fs::read(binary).ok())
                    .flatten()?;
                if let Some(path) = &cached {
                    store_cached_helper(path, &bytes);
                }
                Some(bytes)
            };
            // Keep a compiled helper even when the deadline expired meanwhile: the
            // deadline check below still ends this request, and the next one reuses it.
            match compile() {
                Some(bytes) => {
                    let _ = HELPER.set(bytes);
                }
                None => {
                    if crate::process::check_deadline().is_ok() {
                        let mut at = FAILED_AT.lock().unwrap_or_else(|error| error.into_inner());
                        *at = Some(Instant::now());
                    }
                    return None;
                }
            }
        }
        drop(guard);
    }
    crate::process::check_deadline().ok()?;
    let bytes = HELPER.get()?;
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

/// Analyze supplied, admitted Go source bytes without executing source code.
pub fn semantic(request: &Value) -> crate::Result<Value> {
    let bytes = serde_json::to_vec(&json!({"semantic":request}))?;
    if bytes.len() > 3 * 1024 * 1024 {
        return Err("Semantic request limit exceeded".into());
    }
    let (directory, binary) = helper().ok_or("Go compiler helper unavailable")?;
    let output = crate::process::capture_bounded(
        offline_command(binary)
            .current_dir(directory.path())
            .env_remove("GOOS")
            .env_remove("GOARCH"),
        Duration::from_secs(30),
        Some(bytes),
        8 * 1024 * 1024,
    )?;
    if !output.success {
        return Err("Go semantic analysis failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Compiler facts for an isolated, explicitly pinned Go snapshot set. The
/// helper shares go/types resolution with extraction, but records build inputs,
/// exact declarations and every call site instead of graph-name guesses.
pub fn review(root: &Path, request: &Value) -> crate::Result<Value> {
    decode_review_facts(
        &review_encoded(root, request).map_err(|e| crate::Error::from(e.to_string()))?,
    )
}

/// Bounded compressed facts, also suitable for immutable cache storage.
pub fn review_encoded(root: &Path, request: &Value) -> Result<Value, ReviewFailure> {
    let timings = std::env::var("GRAPHICLE_PHASE_TIMINGS").is_ok_and(|v| !v.is_empty() && v != "0");
    let started = std::time::Instant::now();
    let (_helper_dir, binary) = helper().ok_or_else(|| ReviewFailure::new("helper_unavailable"))?;
    if timings {
        eprintln!("go_phase=helper ns={}", started.elapsed().as_nanos());
    }
    let cache = tempfile::tempdir().map_err(|_| ReviewFailure::new("scratch_unavailable"))?;
    let output = crate::process::capture_bounded(
        offline_command(&binary)
            .current_dir(root)
            .env("GOMODCACHE", cache.path().join("modules"))
            .env("GOCACHE", cache.path().join("build")),
        Duration::from_secs(REVIEW_TIMEOUT_SECONDS),
        Some(
            serde_json::to_vec(&json!({"review":request}))
                .map_err(|_| ReviewFailure::new("invalid_request"))?,
        ),
        REVIEW_OUTPUT_BYTES,
    )
    .map_err(ReviewFailure::process)?;
    if timings {
        // Never relay arbitrary helper/toolchain stderr, which may contain
        // source text or paths. Only fixed phases, counters and identity hashes.
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            if let Some((name, ns)) = native_review_timing(line) {
                eprintln!("go_phase={name} ns={ns}");
            } else if let Some(stats) = native_review_cache(line) {
                eprintln!("phase=review_package_cache_native ms=0 stats={stats}");
            }
        }
    }
    if !output.success {
        let mut failure = ReviewFailure::new("helper_exit");
        failure.detail["exit_code"] = json!(output.code);
        return Err(failure);
    }
    serde_json::from_slice(&output.stdout).map_err(|_| ReviewFailure::new("invalid_output"))
}

fn native_review_timing(line: &str) -> Option<(&str, u64)> {
    let (name, ns) = line.strip_prefix("go_phase=")?.split_once(" ns=")?;
    if !matches!(
        name,
        "other"
            | "preflight"
            | "discovery"
            | "metadata"
            | "import"
            | "parse"
            | "type_check"
            | "collect"
            | "link"
            | "encode"
            | "fingerprint"
            | "export_load"
            | "export_save"
    ) {
        return None;
    }
    Some((name, ns.parse().ok()?))
}

fn native_review_cache(line: &str) -> Option<Value> {
    if line.len() > 8192 {
        return None;
    }
    let value: Value = serde_json::from_str(line.strip_prefix("go_cache=")?).ok()?;
    let object = value.as_object()?;
    if object.len() != 12 {
        return None;
    }
    for field in ["key", "identity", "configuration_sha256"] {
        let text = object.get(field)?.as_str()?;
        if !(text.is_empty()
            || field == "key" && text == "unavailable"
            || text.len() == 64
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return None;
        }
    }
    for field in [
        "hits",
        "misses",
        "rejected",
        "pool_entries",
        "pool_bytes",
        "created_entries",
        "created_bytes",
        "used_entries",
    ] {
        object.get(field)?.as_u64()?;
    }
    for (name, count) in object.get("counts")?.as_object()? {
        if !matches!(
            name.as_str(),
            "supplied_entries"
                | "supplied_bytes"
                | "disabled_identity"
                | "configuration_unreadable"
                | "input_invalid_key"
                | "input_entry_limit"
                | "input_count_limit"
                | "input_byte_limit"
                | "loaded_entries"
                | "loaded_bytes"
                | "miss_not_present"
                | "miss_import_failed"
                | "miss_invalid_export"
                | "hit_standard_library"
                | "hit_dependency"
                | "save_attempted"
                | "save_export_failed"
                | "save_raw_limit"
                | "save_compression_failed"
                | "save_entry_limit"
                | "evicted_unused_entries"
                | "save_count_limit"
                | "save_byte_limit"
                | "saved_entries"
                | "transport_metadata_omitted"
                | "transport_omitted_entries"
                | "transport_omitted_bytes"
                | "transport_entries"
                | "transport_bytes"
        ) {
            return None;
        }
        count.as_u64()?;
    }
    Some(value)
}

#[cfg(test)]
mod timing_tests {
    #[test]
    fn cache_diagnostics_never_forward_paths_text_or_unknown_fields() {
        let good = serde_json::json!({"key":"a".repeat(64),"identity":"b".repeat(64),"configuration_sha256":"c".repeat(64),
            "counts":{"miss_not_present":2},"hits":0,"misses":2,"rejected":0,"pool_entries":2,"pool_bytes":20,
            "created_entries":2,"created_bytes":20,"used_entries":2});
        let parse = |v: &serde_json::Value| super::native_review_cache(&format!("go_cache={v}"));
        assert_eq!(parse(&good), Some(good.clone()));
        for (field, value) in [
            ("key", serde_json::json!("/private/source.go")),
            ("identity", serde_json::json!("bad\nmessage")),
            ("hits", serde_json::json!(-1)),
            ("pool_bytes", serde_json::json!("123")),
            ("counts", serde_json::json!({"/private/source.go":1})),
            ("counts", serde_json::json!({"miss_not_present":"source"})),
            ("extra", serde_json::json!("must not leak")),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(parse(&bad).is_none(), "{bad}");
        }
        assert!(super::native_review_cache(&format!("go_cache={} ", " ".repeat(8192))).is_none());
    }

    #[test]
    fn forwards_only_known_numeric_native_timings() {
        assert_eq!(
            super::native_review_timing("go_phase=type_check ns=123"),
            Some(("type_check", 123))
        );
        for line in [
            "go_phase=/private/source.go ns=123",
            "go_phase=collect ns=123 source",
            "go_phase=collect ns=-1",
            "source error",
            "go_phase=collect ns=18446744073709551616",
        ] {
            assert_eq!(super::native_review_timing(line), None);
        }
    }
}

const REVIEW_TIMEOUT_SECONDS: u64 = 120;
const REVIEW_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Bounded machine diagnostics, without forwarding subprocess source/stderr.
#[derive(Debug)]
pub struct ReviewFailure {
    pub detail: Value,
}
impl ReviewFailure {
    fn new(cause: &str) -> Self {
        Self {
            detail: json!({"stage":"go_compiler_analysis","cause":cause}),
        }
    }
    fn process(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::TimedOut => Self {
                detail: json!({"stage":"go_compiler_analysis","cause":"timeout","limit_seconds":REVIEW_TIMEOUT_SECONDS}),
            },
            std::io::ErrorKind::FileTooLarge => Self {
                detail: json!({"stage":"go_compiler_analysis","cause":"output_bound","limit_bytes_per_stream":REVIEW_OUTPUT_BYTES}),
            },
            _ => Self::new("process_io"),
        }
    }
}
impl std::fmt::Display for ReviewFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Go review analysis unavailable: {}", self.detail)
    }
}
impl std::error::Error for ReviewFailure {}

/// Decode directly to typed facts without a second JSON object tree.
/// Both transport and expanded bytes are bounded; corrupt streams fail closed.
pub fn decode_review_facts<T: serde::de::DeserializeOwned>(value: &Value) -> crate::Result<T> {
    decode_review_with_limit(value, 512 * 1024 * 1024)
}

fn decode_review_with_limit<T: serde::de::DeserializeOwned>(
    value: &Value,
    max: u64,
) -> crate::Result<T> {
    use base64::Engine;
    use std::io::Read;
    if value.get("encoding").is_none() {
        return Ok(serde_json::from_value(value.clone())?);
    }
    if value["encoding"] != "gzip-base64-v1" {
        return Err("Unsupported Go fact encoding".into());
    }
    let encoded = value["facts"].as_str().ok_or("Missing Go facts")?;
    if encoded.len() > 64 * 1024 * 1024 {
        return Err("Go fact transport bound exceeded".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| crate::Error::invalid("Invalid Go fact base64"))?;
    let mut decoder = flate2::bufread::GzDecoder::new(bytes.as_slice());
    let mut bounded = (&mut decoder).take(max + 1);
    let facts = serde_json::from_reader(std::io::BufReader::new(&mut bounded))?;
    if bounded.limit() == 0 {
        return Err("Go fact expansion bound exceeded".into());
    }
    if !decoder.get_ref().is_empty() {
        return Err("Trailing Go fact data".into());
    }
    Ok(facts)
}

/// Identity for immutable compiler-fact reuse, queried outside the checkout.
pub fn review_identity() -> crate::Result<Value> {
    let directory = tempfile::tempdir()?;
    let output = crate::process::capture(
        offline_command("go").current_dir(directory.path()).args([
            "env",
            "-json",
            "GOVERSION",
            "GOROOT",
            "GOHOSTOS",
            "GOHOSTARCH",
        ]),
        Duration::from_secs(5),
    )?;
    if !output.success {
        return Err("Go toolchain identity unavailable".into());
    }
    Ok(
        json!({"toolchain":serde_json::from_slice::<Value>(&output.stdout)?,
        "helper":include_str!("go_types.go"),"review":include_str!("go_review.go"),"semantic":include_str!("go_semantic.go"),
        "package_cache":include_str!("go_review_cache.go"),"export_library":GO_EXPORT_SOURCES,"policy":"offline-cgo-disabled-v1"}),
    )
}

#[derive(Deserialize)]
struct Site {
    #[serde(default)]
    candidate: bool,
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

#[derive(Deserialize)]
struct Group {
    root: String,
    files: Vec<String>,
    sites: Vec<Site>,
}

/// The helper prints one line per module group as it finishes. Returns the
/// finished groups' sites in root order (the order of a complete run) and the
/// files those groups cover. A line cut short by a kill or crash is not a group.
fn finished_groups(
    stdout: &[u8],
    root: &Path,
) -> Result<(Vec<Site>, BTreeSet<String>), &'static str> {
    let whole = &stdout[..stdout
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1)];
    let mut groups = whole
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .map(serde_json::from_slice::<Group>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "go_analysis_invalid_output")?;
    groups.sort_by(|a, b| a.root.cmp(&b.root));
    let covered = groups
        .iter()
        .flat_map(|g| &g.files)
        .map(|f| source_key(f, root))
        .collect();
    Ok((groups.into_iter().flat_map(|g| g.sites).collect(), covered))
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
    // The compiler also reads the toolchain, module cache and local replace
    // targets outside the scan. Until those inputs are recorded, every update
    // must retry resolution, including recovery from an unavailable helper.
    crate::inputs::uncovered("Go compiler inputs outside the scan");
    let files: BTreeSet<_> = paths
        .iter()
        .map(|p| source_key(&p.to_string_lossy(), root))
        .collect();
    // Remove all legacy guesses, including same-file and receiver passes.
    edges.retain(|e| {
        !matches!(s(e, "relation"), "calls" | "may_call")
            || !files.contains(&source_key(s(e, "source_file"), root))
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
                    let mut call = json!({"source_file":key.0,"source_location":format!("L{}",key.1),"column":key.2,"language":"go","reason":"type_information_unavailable"});
                    // The callee as written (`server.Run`, `Map[int]`), so an
                    // answer can tell which symbol an unresolved call could
                    // name; a function literal or a call result names none.
                    if let Some(function) = node.child_by_field_name("function").filter(|f| {
                        matches!(
                            f.kind(),
                            "identifier"
                                | "selector_expression"
                                | "generic_type"
                                | "index_expression"
                        )
                    }) && let Ok(text) = function.utf8_text(&parsed.source)
                    {
                        let text: String = text.split_whitespace().collect();
                        call["callee"] = json!(text.chars().take(200).collect::<String>());
                    }
                    unresolved.insert(key.clone(), call);
                }
                let mut cursor = node.walk();
                pending.extend(node.named_children(&mut cursor));
            }
        }
    }
    let finished = (|| -> Result<_, &str> {
        let (_helper_dir, binary) = helper().ok_or("go_helper_unavailable")?;
        let cache = tempfile::tempdir().map_err(|_| "go_scratch_unavailable")?;
        let workspace = root.join("go.work");
        let workspace = if workspace.is_file() {
            workspace.into_os_string()
        } else {
            "off".into()
        };
        // Extraction may use already-cached dependencies. Keep the inherited
        // module cache (or Go's default); offline_command and -mod=readonly
        // prevent downloads and module-file edits. Pinned review stays isolated.
        let run = crate::process::capture_lines(
            offline_command(&binary)
                .current_dir(root)
                .env("GOWORK", workspace)
                .env("GO111MODULE", "auto")
                .env("GOCACHE", cache.path().join("build")),
            // A large repository runs for minutes. Each module group reports
            // when it finishes, so the helper is stopped only after five
            // minutes with no group finishing; the request deadline still
            // applies. A 6,900-file repository printed ~31 KiB per file.
            Duration::from_secs(300),
            Some(serde_json::to_vec(&json!({"Paths": paths})).map_err(|_| "go_request_invalid")?),
            (paths.len() * 64 * 1024).max(64 * 1024 * 1024),
        )
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                "go_analysis_timeout"
            } else {
                "go_analysis_io_failure"
            }
        })?;
        let (sites, covered) = finished_groups(&run.output.stdout, root)?;
        let unfinished = if run.stalled {
            Some("go_analysis_timeout")
        } else {
            (!run.output.success).then_some("go_analysis_failed")
        };
        Ok((sites, covered, unfinished))
    })();
    // Finished groups keep their results; only calls the helper never
    // reported on carry the failure.
    let (sites, covered, unfinished) = match finished {
        Ok(finished) => finished,
        Err(reason) => (vec![], BTreeSet::new(), Some(reason)),
    };
    if let Some(reason) = unfinished {
        for ((file, ..), value) in unresolved.iter_mut() {
            if !covered.contains(file) {
                value["reason"] = json!(reason);
            }
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
    for site in sites {
        let key = (source_key(&site.file, root), site.line, site.column);
        if !files.contains(&key.0) {
            continue;
        }
        let from = declaration(&site.file, site.caller_line, &site.caller);
        let to = declaration(&site.target_file, site.target_line, &site.target);
        if site.reason.is_empty()
            && let (Some(from), Some(to)) = (from, to)
        {
            if have.insert((
                from.clone(),
                to.clone(),
                site.candidate,
                site.line,
                site.column,
            )) {
                edges.push(json!({"source":from,"target":to,"relation":if site.candidate{"may_call"}else{"calls"},"confidence":if site.candidate{"INFERRED"}else{"EXTRACTED"},"weight":1.0,"context":"call","source_file":site.file,"source_location":format!("L{}",site.line),"resolution":"go_types","evidence":if site.candidate{"function_value_candidate"}else{"type"}}));
            }
            if !site.candidate {
                unresolved.remove(&key);
            }
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

#[cfg(test)]
mod fact_encoding_tests {
    use super::*;
    use base64::Engine;
    use std::io::Write;
    #[test]
    fn finished_groups_keep_root_order_and_drop_a_cut_line() {
        let root = Path::new("/repo");
        let site = |file: &str| {
            format!(r#"{{"file":"{file}","line":1,"column":1,"caller":"G","caller_line":1}}"#)
        };
        let stdout = format!(
            "{{\"root\":\"/repo/b\",\"files\":[\"/repo/b/x.go\"],\"sites\":[{}]}}\n{{\"root\":\"/repo/a\",\"files\":[\"/repo/a/x.go\",\"/repo/a/y.go\"],\"sites\":[{}]}}\n{{\"root\":\"/repo/c\",\"fil",
            site("/repo/b/x.go"),
            site("/repo/a/x.go"),
        );
        let (sites, covered) = finished_groups(stdout.as_bytes(), root).unwrap();
        let files: Vec<_> = sites.iter().map(|s| s.file.as_str()).collect();
        assert_eq!(files, ["/repo/a/x.go", "/repo/b/x.go"]);
        assert_eq!(
            covered,
            ["a/x.go", "a/y.go", "b/x.go"]
                .map(|f| source_key(&format!("/repo/{f}"), root))
                .into()
        );
        assert!(finished_groups(b"[]\n", root).is_err());
        assert!(finished_groups(b"", root).unwrap().0.is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn a_cached_helper_is_used_only_when_this_user_alone_could_write_it() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path, m: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap()
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("go-helper").join("key");
        assert!(read_cached_helper(&path).is_none());
        store_cached_helper(&path, b"binary");
        assert_eq!(read_cached_helper(&path).as_deref(), Some(&b"binary"[..]));
        mode(&path, 0o666);
        assert!(
            read_cached_helper(&path).is_none(),
            "a file others can write"
        );
        mode(&path, 0o700);
        mode(path.parent().unwrap(), 0o777);
        assert!(
            read_cached_helper(&path).is_none(),
            "a folder others can write"
        );
        mode(path.parent().unwrap(), 0o700);
        assert!(read_cached_helper(&path).is_some());
    }
    #[test]
    fn compiler_failures_name_the_bound_without_forwarding_process_output() {
        let timeout = ReviewFailure::process(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "private source",
        ));
        assert_eq!(timeout.detail["cause"], "timeout");
        assert_eq!(timeout.detail["limit_seconds"], 120);
        let output = ReviewFailure::process(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "private source",
        ));
        assert_eq!(output.detail["cause"], "output_bound");
        assert_eq!(output.detail["limit_bytes_per_stream"], 64 * 1024 * 1024);
        assert!(!output.to_string().contains("private source"));
        assert!(!timeout.to_string().contains("private source"));
    }
    fn encoded(raw: &[u8]) -> Value {
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(raw).unwrap();
        json!({"encoding":"gzip-base64-v1","facts":base64::engine::general_purpose::STANDARD.encode(gzip.finish().unwrap())})
    }
    #[test]
    fn fact_transport_checks_encoding_integrity_and_expansion() {
        let data = json!({"worlds":[],"literal":"unicode π"});
        let wire = encoded(data.to_string().as_bytes());
        assert_eq!(decode_review_facts::<Value>(&wire).unwrap(), data);
        assert_eq!(decode_review_facts::<Value>(&data).unwrap(), data);
        assert!(decode_review_with_limit::<Value>(&wire, 8).is_err());
        assert!(decode_review_facts::<Value>(&json!({"encoding":"other","facts":""})).is_err());
        assert!(
            decode_review_facts::<Value>(&json!({"encoding":"gzip-base64-v1","facts":"!"}))
                .is_err()
        );
        let mut truncated = base64::engine::general_purpose::STANDARD
            .decode(wire["facts"].as_str().unwrap())
            .unwrap();
        truncated.truncate(truncated.len() - 2);
        assert!(decode_review_facts::<Value>(&json!({"encoding":"gzip-base64-v1","facts":base64::engine::general_purpose::STANDARD.encode(truncated)})).is_err());
        assert!(decode_review_facts::<Value>(&encoded(b"{}{}")).is_err());
    }
}
