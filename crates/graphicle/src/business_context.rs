//! Optional, evidence-bearing business context. Classification proposes questions;
//! it never upgrades a historical assertion to an owner-confirmed constraint.
use crate::{Error, Result, local, review::Pin, store};
use graphicle_memory::text;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) mod evolution;

const FORMAT: &str = "graphicle.business-history.v1";
const MAX_FILES: usize = 32;
const MAX_RANGES: usize = 128;
const MAX_ASSERTIONS: usize = 3;
pub const MAX_HISTORY_BYTES: usize = 512 * 1024;
/// Two default bundles, including the request envelope, fit the aggregate limit.
pub const DEFAULT_HISTORY_BYTES: usize = 240 * 1024;

fn check_history_budget(bytes: usize) -> Result<()> {
    if !(16 * 1024..=MAX_HISTORY_BYTES).contains(&bytes) {
        return Err(Error::bad("History byte budget must be 16384–524288"));
    }
    Ok(())
}

/// Scan decoded fields, never one synthetic JSON line. An oversized or otherwise
/// unscannable individual field still fails closed in the existing detector.
fn ensure_safe_value(value: &Value) -> Result<()> {
    match value {
        Value::String(s) => ensure_safe(s),
        Value::Array(values) => {
            for v in values {
                ensure_safe_value(v)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (k, v) in values {
                ensure_safe(k)?;
                ensure_safe_value(v)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub history: Vec<History>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub format: String,
    pub pin: Pin,
    pub shallow: bool,
    pub files: Vec<FileHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evolution: Option<evolution::Evolution>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileHistory {
    pub path: String,
    pub file_sha256: String,
    pub line_count: usize,
    pub omitted_ranges: usize,
    pub attributions: Vec<Attribution>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribution {
    pub commit: String,
    pub line_start: usize,
    pub line_end: usize,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

fn ensure_safe(value: &str) -> Result<()> {
    if graphicle::security::redact_secrets(value) != value {
        return Err(Error::bad(
            "History credential scan rejected content or could not complete; omit it before indexing",
        ));
    }
    Ok(())
}

fn commit_id(id: &str) -> bool {
    [40, 64].contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn admitted_path(path: &str) -> Result<()> {
    store::path(path)?;
    if path.split('/').any(|p| {
        local::sensitive(p)
            || p.starts_with(".env")
            || p == "secrets"
            || local::SKIPPED_NAMES.contains(&p)
    }) || local::skipped_file(path)
    {
        return Err(Error::bad("History path is excluded by source admission"));
    }
    Ok(())
}

/// Match history to authorized immutable inputs before running the analyzer.
/// The callback returns a loaded source's digest and line count, never live HEAD.
pub(crate) fn validate(
    options: &Options,
    base: &[Pin],
    head: &[Pin],
    source: impl Fn(&Pin, &str) -> Option<(String, usize)>,
) -> Result<()> {
    if options.history.len() > 128 || serde_json::to_vec(options)?.len() > MAX_HISTORY_BYTES {
        return Err(Error::bad("Business history exceeds 128 bundles / 512 KiB"));
    }
    ensure_safe_value(&serde_json::to_value(options)?)?;
    let mut seen = BTreeSet::new();
    let mut subjects = BTreeMap::new();
    let mut ranges = 0usize;
    let mut files = 0usize;
    for history in &options.history {
        if history.format != FORMAT
            || !base.iter().chain(head).any(|p| p == &history.pin)
            || history.files.len() > MAX_FILES
        {
            return Err(Error::bad(
                "Business history needs an exact review pin and supported format",
            ));
        }
        if let Some(evolution) = &history.evolution {
            evolution.validate(history, &source)?;
        }
        for file in &history.files {
            admitted_path(&file.path)?;
            store::hash(&file.file_sha256)?;
            if !seen.insert((
                crate::digest(&serde_json::to_value(&history.pin)?),
                &file.path,
            )) {
                return Err(Error::bad("Duplicate business history file for pin"));
            }
            let Some((sha, lines)) = source(&history.pin, &file.path) else {
                return Err(Error::conflict(
                    "Business history source unavailable in pinned snapshot",
                ));
            };
            if sha != file.file_sha256 || lines != file.line_count {
                return Err(Error::conflict(
                    "Business history source revision differs from pinned snapshot",
                ));
            }
            files += 1;
            ranges += file.attributions.len();
            if files > 128 || ranges > 4096 || file.attributions.len() > MAX_RANGES {
                return Err(Error::bad(
                    "Business history exceeds file or attribution limits",
                ));
            }
            let mut previous_end = 0;
            for attribution in &file.attributions {
                if !commit_id(&attribution.commit)
                    || attribution.line_start <= previous_end
                    || attribution.line_start > attribution.line_end
                    || attribution.line_end > lines
                    || attribution.subject.len() > 512
                    || attribution.subject.chars().any(char::is_control)
                {
                    return Err(Error::bad("Invalid business history attribution"));
                }
                if attribution
                    .author_id
                    .as_ref()
                    .is_some_and(|id| store::hash(id).is_err())
                    || attribution
                        .timestamp
                        .is_some_and(|t| !(0..=4_102_444_800).contains(&t))
                {
                    return Err(Error::bad("Invalid historical ownership attribution"));
                }
                previous_end = attribution.line_end;
                if let Some(subject) = subjects.insert(
                    (&history.pin.repository_id, &attribution.commit),
                    &attribution.subject,
                ) && subject != &attribution.subject
                {
                    return Err(Error::bad(
                        "Conflicting subjects for the same historical commit",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Split identifier words as well as prose, without substring matches such as
/// `Author` -> `auth` or `Discharge` -> `charge`.
fn words(input: &str) -> BTreeSet<String> {
    let mut spaced = String::new();
    let chars: Vec<_> = input.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase()
            && i > 0
            && (chars[i - 1].is_ascii_lowercase()
                || (chars[i - 1].is_ascii_uppercase()
                    && chars.get(i + 1).is_some_and(char::is_ascii_lowercase)))
        {
            spaced.push(' ');
        }
        spaced.push(ch.to_ascii_lowercase());
    }
    spaced
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn domains(input: &str, basis: &str, evidence: &str) -> Vec<Value> {
    let words = words(input);
    let rules: &[(&str, &[&str], &str)] = &[
        (
            "authorization",
            &[
                "auth",
                "authorize",
                "authorization",
                "permission",
                "permissions",
                "tenant",
                "acl",
            ],
            "Does this change preserve authorization and tenant isolation for the affected callers?",
        ),
        (
            "payments",
            &[
                "payment", "payments", "charge", "charges", "refund", "billing", "checkout",
            ],
            "Does this change preserve payment outcomes, including retry and duplicate-request behavior?",
        ),
        (
            "data_lifecycle",
            &["delete", "purge", "retention", "erase"],
            "Does this change preserve deletion scope, retention and recovery requirements?",
        ),
    ];
    rules.iter().filter_map(|(domain, cues, question)| {
        let matched: Vec<_> = cues.iter().filter(|cue| words.contains(**cue)).collect();
        (!matched.is_empty()).then(|| json!({"domain":domain,"status":"candidate","confidence":"low","basis":basis,"evidence":evidence,"matched_words":matched,"review_question":question}))
    }).collect()
}

fn assertion_kind(subject: &str) -> &'static str {
    let words = words(subject);
    if words.contains("revert") || words.contains("reverted") {
        "revert_candidate"
    } else if ["fix", "prevent", "avoid", "regression"]
        .iter()
        .any(|w| words.contains(*w))
    {
        "correctness_candidate"
    } else {
        "historical_intent"
    }
}

/// Enrich native declarations; reuse their selection explanations and
/// the review packer's revision, omission and invalidation machinery.
pub(crate) fn annotate(
    report: &mut Value,
    options: &Options,
    base: &[Pin],
    head: &[Pin],
    max_tokens: usize,
) {
    let mut index = BTreeMap::new();
    for (side, pins) in [("base", base), ("head", head)] {
        for history in &options.history {
            if pins.contains(&history.pin) {
                for file in &history.files {
                    index.insert(
                        (side, history.pin.repository_id.as_str(), file.path.as_str()),
                        (history, file),
                    );
                }
            }
        }
    }
    let changed: BTreeSet<_> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["reasons"]
                .as_array()
                .is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"))
        })
        .map(|e| {
            (
                store::s(e, "side").to_owned(),
                store::s(e, "repository_id").to_owned(),
                store::s(e, "file").to_owned(),
                e["byte_start"].as_u64().unwrap_or(0),
                e["byte_end"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    // Bound each candidate here; the review packer owns the shared optional
    // budget after reserving required code and diff space.
    let evolution_budget = max_tokens / 8;
    let mut omitted_evolution = 0;
    let mut annotated = 0;
    let mut omitted = 0;
    // Spend the optional evidence allowance on current changes first. Base
    // history and statistical peers must not crowd out current-head evidence.
    report["entries"].as_array_mut().unwrap().sort_by_key(|e| {
        let seed = e["reasons"]
            .as_array()
            .is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"));
        (!seed, e["side"] != "head")
    });
    for entry in report["entries"].as_array_mut().unwrap() {
        // Qualified package names would classify every function in a package.
        // Only the declaration's own terminal name is a lexical code cue.
        let name = store::s(entry, "name").rsplit('.').next().unwrap_or("");
        let mut candidates = domains(name, "identifier_words", store::s(entry, "symbol_id"));
        let mut assertions = vec![];
        let mut omitted_assertions = 0;
        let mut history_status = json!({"status":"not_supplied"});
        let mut evolution_evidence = None;
        if let Some((history, file)) = index.get(&(
            store::s(entry, "side"),
            store::s(entry, "repository_id"),
            store::s(entry, "file"),
        )) {
            let start = entry["line"].as_u64().unwrap() as usize;
            let end = entry["end_line"].as_u64().unwrap() as usize;
            if let Some(analysis) = &history.evolution {
                evolution_evidence = analysis.evidence(entry, history.shallow);
                if let Some(evidence) = &mut evolution_evidence {
                    evidence["ownership"] =
                        evolution::ownership(file, start, end, analysis.anchor_time);
                    for peer in evidence["cochange"].as_array_mut().unwrap() {
                        peer["changed_in_review"] = json!(changed.contains(&(
                            store::s(entry, "side").to_owned(),
                            store::s(entry, "repository_id").to_owned(),
                            store::s(peer, "file").to_owned(),
                            peer["byte_start"].as_u64().unwrap_or(0),
                            peer["byte_end"].as_u64().unwrap_or(0)
                        )));
                    }
                }
            }
            let mut commits = BTreeMap::<&str, (&Attribution, Vec<Value>)>::new();
            for a in file
                .attributions
                .iter()
                .filter(|a| a.line_start <= end && a.line_end >= start)
            {
                commits.entry(&a.commit).or_insert_with(|| (a, vec![])).1.push(json!({"line_start":a.line_start.max(start),"line_end":a.line_end.min(end)}));
            }
            let mut matches: Vec<_> = commits.into_values().collect();
            // Correctness/revert subjects compete first; ties retain source order.
            matches.sort_by_key(|(a, _)| {
                (
                    assertion_kind(&a.subject) == "historical_intent",
                    a.line_start,
                )
            });
            omitted_assertions = matches.len().saturating_sub(MAX_ASSERTIONS);
            for (a, ranges) in matches.into_iter().take(MAX_ASSERTIONS) {
                let kind = assertion_kind(&a.subject);
                let mut assertion = json!({"commit":a.commit,"subject":a.subject,"kind":kind,"authority":"caller_attested","validity":"unverified","association":"surviving_line_overlap","ranges":ranges,"pin":history.pin,"file_sha256":file.file_sha256,"file":file.path});
                let id = crate::digest(&assertion);
                assertion["id"] = json!(id);
                if kind != "revert_candidate" {
                    candidates.extend(domains(&a.subject, "historical_subject", &id));
                }
                assertions.push(assertion);
            }
            history_status = json!({"status":"partial","shallow":history.shallow,"collector_omitted_ranges":file.omitted_ranges});
        }
        if !candidates.is_empty() || !assertions.is_empty() {
            annotated += 1;
        }
        omitted += omitted_assertions;
        // Absence of historical/domain evidence is already described by the
        // report's limitations. Do not attach and hash an empty annotation on
        // every conservative dependency candidate.
        if candidates.is_empty() && assertions.is_empty() && evolution_evidence.is_none()
            && entry["reasons"].as_array().is_some_and(|reasons| !reasons.is_empty()
                && reasons.iter().all(|reason| reason["uncertain_scope"] == true))
        {
            continue;
        }
        entry["business_context"] = json!({"candidates":candidates,"historical_assertions":assertions,"history":history_status,"omitted_assertions":omitted_assertions,"criticality":"unknown","runtime_hotness":"unknown","owner_confirmed":false,"dependency_paths":"reasons"});
        if let Some(mut evidence) = evolution_evidence {
            for field in ["similar_changes", "function_history", "cochange"] {
                while text::count(&evidence.to_string()) + 180 > evolution_budget
                    && evidence[field].as_array().is_some_and(|a| !a.is_empty())
                {
                    evidence[field].as_array_mut().unwrap().pop();
                    let omitted = format!(
                        "omitted_{}",
                        if field == "function_history" {
                            "history"
                        } else if field == "similar_changes" {
                            "similar"
                        } else {
                            "cochange"
                        }
                    );
                    evidence[&omitted] = json!(evidence[&omitted].as_u64().unwrap_or(0) + 1);
                }
            }
            let cost = text::count(&evidence.to_string()) + 180;
            if cost > evolution_budget {
                omitted_evolution += 1;
                entry["business_context"]["evolution_omitted"] = json!(true);
                entry.as_object_mut().unwrap().remove("revision");
                entry["revision"] = json!(crate::digest(entry));
                continue;
            }
            let mut signals = vec![];
            if evidence["function_history"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["kind"] == "correctness_candidate")
            {
                signals.push(json!({"reason":"previous_correctness_fix","question":"Does this edit preserve the behavior that the earlier fix introduced?"}));
            }
            if evidence["cochange"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["changed_in_review"] == false)
            {
                signals.push(json!({"reason":"historically_coupled_function_unchanged","question":"Does the associated function or test need a companion change?"}));
            }
            if evidence["sample"]["target_units"].as_u64().unwrap_or(0) >= 3
                && evidence["hotspot"]["decision_complexity"]
                    .as_u64()
                    .unwrap_or(0)
                    >= 5
            {
                signals.push(json!({"reason":"frequent_complex_change","rule":"at least 3 sampled changes and 5 AST decisions","question":"Which branches and regression tests cover this edit?"}));
            }
            entry["business_context"]["review_priority"] = json!({"level":if signals.is_empty(){"unassessed"}else{"elevated"},"signals":signals,"confidence":"limited_historical_evidence","policy":"history-review-priority-v1","not_defect_probability":true,"not_business_criticality":true});
            entry["business_context"]["evolution"] = evidence;
        }
        entry.as_object_mut().unwrap().remove("revision");
        entry["revision"] = json!(crate::digest(entry));
    }
    report["business_context"] = json!({"analyzer":"static-business-context-v2","omitted_evolution_entries":omitted_evolution,"annotated_entries_before_packing":annotated,"omitted_assertions_before_packing":omitted,"history_bundles":options.history.len(),"complete":false,"limitations":["lexical_candidates_are_not_business_importance","historical_assertions_are_untrusted_and_may_be_superseded","blame_omits_deleted_lines_and_does_not_prove_constraint_validity","dependency_and_packet_coverage_limits_apply","no_runtime_or_owner_confirmation"]});
}

/// Statistical context candidates; the caller must match exact pinned native
/// declarations before materializing any excerpt. These are never call edges.
pub(crate) fn suggestions(
    report: &Value,
    options: &Options,
    base: &[Pin],
    head: &[Pin],
) -> (Vec<Value>, usize) {
    let mut result = vec![];
    let mut omitted = 0;
    let mut seen = BTreeSet::new();
    let mut seeds: Vec<_> = report["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            e["reasons"]
                .as_array()
                .is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"))
        })
        .collect();
    seeds.sort_by_key(|e| e["side"] != "head");
    for entry in seeds {
        let pins = if entry["side"] == "base" { base } else { head };
        for history in options
            .history
            .iter()
            .filter(|h| pins.contains(&h.pin) && entry["repository_id"] == h.pin.repository_id)
        {
            let Some(evidence) = history
                .evolution
                .as_ref()
                .and_then(|e| e.evidence(entry, history.shallow))
            else {
                continue;
            };
            for peer in evidence["cochange"].as_array().into_iter().flatten() {
                let key = crate::digest(&json!([
                    entry["side"],
                    history.pin.repository_id,
                    peer["file"],
                    peer["byte_start"],
                    peer["byte_end"]
                ]));
                if seen.insert(key) {
                    if result.len() < 6 {
                        result.push(json!({"side":entry["side"],"repository_id":history.pin.repository_id,"peer":peer,"reason":"historical_cochange","origin":entry["symbol_id"],"compiler_dependency":false,"authority":"caller_attested","validity":"unverified"}));
                    } else {
                        omitted += 1;
                    }
                }
            }
        }
    }
    (result, omitted)
}

/// Execute read-only Git plumbing, with a single deadline and bounded output.
/// No shells, textconv, replacement objects, lazy fetches or inherited Git paths.
fn git(root: &Path, args: &[&str], deadline: Instant, limit: usize) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("GIT_"))
    {
        command.env_remove(name);
    }
    let mut child = command
        .args([
            "--no-pager",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.allow=never",
        ])
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        // This whitelist also overrides repository-local protocol.*.allow,
        // including on Git versions predating GIT_NO_LAZY_FETCH.
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = vec![];
        let result = stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let result = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    // The output reader can finish at the byte bound while Git is still writing.
    // Kill on errors/bounds, and poll status under the same deadline on success.
    let bytes = match result {
        Ok(Ok(bytes)) if bytes.len() <= limit => Some(bytes),
        _ => None,
    };
    let mut success = false;
    if bytes.is_some() {
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) => {
                    success = status.success();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    if success {
        Ok(bytes.unwrap())
    } else {
        Err(Error::bad(
            "Local Git history unavailable, timed out or exceeded output limit",
        ))
    }
}

/// Metadata demand is bounded by the retained ranges, not an unrelated global
/// commit sample. Each Git response and individual record keep their old bounds.
fn commit_subjects(
    root: &Path,
    commits: &BTreeSet<String>,
    repository: &str,
    deadline: Instant,
) -> Result<BTreeMap<String, (String, String, i64)>> {
    if commits.len() > MAX_FILES * MAX_RANGES || commits.iter().any(|id| !commit_id(id)) {
        return Err(Error::bad("Invalid bounded history commit set"));
    }
    let ids: Vec<_> = commits.iter().cloned().collect();
    let mut records = BTreeMap::new();
    for chunk in ids.chunks(MAX_RANGES) {
        records.extend(commit_subject_batch(root, &chunk.iter().cloned().collect(), repository, deadline)?);
    }
    Ok(records)
}

/// One bounded Git invocation for immutable commit metadata. Per-range `show`
/// calls spent the shared collection deadline on process startup in large files.
fn commit_subject_batch(
    root: &Path,
    commits: &BTreeSet<String>,
    repository: &str,
    deadline: Instant,
) -> Result<BTreeMap<String, (String, String, i64)>> {
    if commits.len() > 128 || commits.iter().any(|id| !commit_id(id)) {
        return Err(Error::bad("Invalid bounded history commit set"));
    }
    if commits.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut args = vec!["show", "-s", "--no-walk=unsorted", "--no-show-signature", "--format=%H%x00%ct%x00%ae%x00%s%x00"];
    args.extend(commits.iter().map(String::as_str));
    args.push("--");
    let bytes = git(root, &args, deadline, commits.len() * (64 * 1024 + 66))?;
    let raw = std::str::from_utf8(&bytes).map_err(|_| Error::bad("Invalid commit metadata"))?;
    ensure_safe(raw)?;
    let fields: Vec<_> = raw.trim_end_matches('\n').split_terminator('\0').collect();
    if fields.len() != commits.len() * 4 {
        return Err(Error::bad("Incomplete batched commit metadata"));
    }
    let mut subjects = BTreeMap::new();
    for record in fields.chunks_exact(4) {
        let id = record[0].strip_prefix('\n').unwrap_or(record[0]);
        if !commits.contains(id) || subjects.contains_key(id) {
            return Err(Error::bad("Unexpected batched commit identity"));
        }
        if record[1..].iter().map(|field| field.len()).sum::<usize>() + 3 > 64 * 1024 {
            return Err(Error::bad("Commit metadata exceeds its byte bound"));
        }
        let timestamp = record[1].parse().map_err(|_| Error::bad("Invalid commit time"))?;
        let author = crate::digest(&json!([repository, record[2].to_lowercase()]));
        let mut cleaned = String::new();
        for ch in record[3].trim().chars().filter(|c| !c.is_control()) {
            if cleaned.len() + ch.len_utf8() > 512 { break; }
            cleaned.push(ch);
        }
        ensure_safe(&cleaned)?;
        subjects.insert(id.to_owned(), (cleaned, author, timestamp));
    }
    Ok(subjects)
}

/// Explicit local collection; the API never accepts checkout paths.
pub fn collect(root: &Path, pin: Pin, paths: &[String]) -> Result<History> {
    collect_with_budget(root, pin, paths, DEFAULT_HISTORY_BYTES)
}

pub fn collect_with_budget(
    root: &Path,
    pin: Pin,
    paths: &[String],
    max_bytes: usize,
) -> Result<History> {
    check_history_budget(max_bytes)?;
    if !commit_id(&pin.commit) || paths.is_empty() || paths.len() > MAX_FILES {
        return Err(Error::bad(
            "History needs a full commit and 1–32 explicit files",
        ));
    }
    store::hash(&pin.snapshot)?;
    store::hash(&pin.policy_sha256)?;
    let mut unique = BTreeSet::new();
    for path in paths {
        admitted_path(path)?;
        if !unique.insert(path) {
            return Err(Error::bad("Duplicate history file"));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    if git(root, &["rev-parse", "--show-prefix"], deadline, 4096)? != b"\n" {
        return Err(Error::bad(
            "History checkout path must be the repository root",
        ));
    }
    let commit = git(
        root,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", pin.commit),
        ],
        deadline,
        128,
    )?;
    if String::from_utf8_lossy(&commit).trim() != pin.commit {
        return Err(Error::bad("History pin must identify a commit object"));
    }
    let shallow = git(
        root,
        &["rev-parse", "--is-shallow-repository"],
        deadline,
        16,
    )? == b"true\n";
    let mut commits = BTreeSet::new();
    let mut files = vec![];
    for path in unique {
        let bytes = git(
            root,
            &["cat-file", "blob", &format!("{}:{path}", pin.commit)],
            deadline,
            1024 * 1024,
        )?;
        let source =
            std::str::from_utf8(&bytes).map_err(|_| Error::bad("History requires UTF-8 source"))?;
        ensure_safe(source)?;
        if source.contains('\0') {
            return Err(Error::bad("History requires text source"));
        }
        let blame = git(
            root,
            &[
                "blame",
                "--no-textconv",
                "--root",
                "--line-porcelain",
                &pin.commit,
                "--",
                path,
            ],
            deadline,
            16 * 1024 * 1024,
        )?;
        let blame =
            std::str::from_utf8(&blame).map_err(|_| Error::bad("Invalid Git blame output"))?;
        let mut ranges: Vec<Attribution> = vec![];
        for line in blame.lines().filter(|line| !line.starts_with('\t')) {
            let columns: Vec<_> = line.split(' ').collect();
            if columns.len() < 3 || !commit_id(columns[0]) {
                continue;
            }
            let number: usize = columns[2]
                .parse()
                .map_err(|_| Error::bad("Invalid Git blame line"))?;
            if let Some(previous) = ranges.last_mut()
                && previous.commit == columns[0]
                && previous.line_end + 1 == number
            {
                previous.line_end = number;
            } else {
                ranges.push(Attribution {
                    commit: columns[0].into(),
                    line_start: number,
                    line_end: number,
                    subject: String::new(),
                    author_id: None,
                    timestamp: None,
                });
            }
        }
        if ranges.last().map_or(0, |r| r.line_end) != source.lines().count() {
            return Err(Error::bad("Git blame does not cover pinned source"));
        }
        let omitted_ranges = ranges.len().saturating_sub(MAX_RANGES);
        ranges.truncate(MAX_RANGES);
        for range in &ranges {
            commits.insert(range.commit.clone());
            if commits.len() > MAX_FILES * MAX_RANGES {
                return Err(Error::bad("History exceeds the retained range bound"));
            }
        }
        files.push(FileHistory {
            path: path.clone(),
            file_sha256: text::sha256(&bytes),
            line_count: source.lines().count(),
            omitted_ranges,
            attributions: ranges,
        });
    }
    let subjects = commit_subjects(root, &commits, &pin.repository_id, deadline)?;
    for file in &mut files {
        for range in &mut file.attributions {
            let (subject, author, timestamp) = &subjects[&range.commit];
            range.subject = subject.clone();
            range.author_id = Some(author.clone());
            range.timestamp = Some(*timestamp);
        }
    }
    let mut history = History {
        format: FORMAT.into(),
        pin,
        shallow,
        files,
        evolution: None,
    };
    evolution::bound_blame(&mut history, max_bytes)?;
    Ok(history)
}

/// Collect the v1 blame envelope plus bounded, local function-change analysis.
pub fn collect_analysis(root: &Path, pin: Pin, paths: &[String]) -> Result<History> {
    collect_analysis_with_budget(root, pin, paths, DEFAULT_HISTORY_BYTES)
}

pub fn collect_analysis_with_budget(
    root: &Path,
    pin: Pin,
    paths: &[String],
    max_bytes: usize,
) -> Result<History> {
    evolution::collect(
        root,
        collect_with_budget(root, pin, paths, max_bytes)?,
        max_bytes,
    )
}

pub(crate) fn schema() -> Value {
    let attribution = json!({"type":"object","properties":{"commit":{"type":"string"},"line_start":{"type":"integer","minimum":1},"line_end":{"type":"integer","minimum":1},"subject":{"type":"string","maxLength":512},"author_id":{"type":"string"},"timestamp":{"type":"integer"}},"required":["commit","line_start","line_end","subject"],"additionalProperties":false});
    let file = json!({"type":"object","properties":{"path":{"type":"string"},"file_sha256":{"type":"string"},"line_count":{"type":"integer","minimum":0},"omitted_ranges":{"type":"integer","minimum":0},"attributions":{"type":"array","maxItems":128,"items":attribution}},"required":["path","file_sha256","line_count","omitted_ranges","attributions"],"additionalProperties":false});
    let history = json!({"type":"object","properties":{"format":{"const":FORMAT},"pin":crate::review::pin_schema(),"shallow":{"type":"boolean"},"evolution":evolution::schema(),"files":{"type":"array","maxItems":32,"items":file}},"required":["format","pin","shallow","files"],"additionalProperties":false});
    json!({"type":"object","properties":{"history":{"type":"array","maxItems":128,"items":history}},"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_peers_stay_inside_the_pinned_modules() {
        let root = tempfile::tempdir().unwrap();
        fixture_git(root.path(), &["init", "-q"]);
        for module in ["included", "excluded"] {
            std::fs::create_dir(root.path().join(module)).unwrap();
            std::fs::write(
                root.path().join(format!("{module}/main.go")),
                "package p\nfunc F() int { return 1 }\n",
            )
            .unwrap();
        }
        fixture_git(root.path(), &["add", "."]);
        fixture_git(root.path(), &["commit", "-qm", "Introduce both modules"]);
        let pin = Pin {
            repository_id: "repo".into(),
            directory: "app".into(),
            commit: fixture_git(root.path(), &["rev-parse", "HEAD"]),
            snapshot: "b".repeat(64),
            policy_sha256: "c".repeat(64),
            modules: vec!["included".into()],
            excluded_modules: vec![],
        };
        let history =
            collect_analysis(root.path(), pin.clone(), &["included/main.go".into()]).unwrap();
        let analysis = history.evolution.as_ref().unwrap();
        assert_eq!(analysis.functions.len(), 1);
        assert!(
            analysis
                .units
                .iter()
                .flat_map(|u| &u.changes)
                .all(|c| c.path == "included/main.go")
        );
        assert!(analysis.omissions["excluded_path"] > 0);
        let sha = history.files[0].file_sha256.clone();
        validate(
            &Options {
                history: vec![history],
            },
            &[],
            &[pin],
            |_, path| (path == "included/main.go").then(|| (sha.clone(), 2)),
        )
        .unwrap();
    }

    fn fixture_git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=History Fixture",
                "-c",
                "user.email=history@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    #[test]
    fn collector_reads_pinned_objects_and_surviving_lines_not_worktree() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        let old = "package access\nfunc Check(tenant, owner string) bool {\n return true\n}\n";
        std::fs::write(dir.path().join("access.go"), old).unwrap();
        fixture_git(dir.path(), &["add", "access.go"]);
        fixture_git(dir.path(), &["commit", "-qm", "Introduce access check"]);
        let original = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        let fixed = old.replace("return true", "return tenant == owner");
        std::fs::write(dir.path().join("access.go"), &fixed).unwrap();
        fixture_git(dir.path(), &["commit", "-qam", "Fix tenant isolation"]);
        let fix = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        let pin = Pin {
            repository_id: "test".into(),
            directory: "app".into(),
            commit: fix.clone(),
            snapshot: "a".repeat(64),
            policy_sha256: "b".repeat(64),
            modules: vec![".".into()],
            excluded_modules: vec![],
        };
        // An uncommitted regression must not change the collected source/hash.
        std::fs::write(dir.path().join("access.go"), old).unwrap();
        let bundle = collect(dir.path(), pin.clone(), &["access.go".into()]).unwrap();
        let file = &bundle.files[0];
        assert_eq!(file.file_sha256, text::sha256(fixed.as_bytes()));
        assert_eq!(file.attributions.len(), 3);
        assert_eq!(file.attributions[0].commit, original);
        assert_eq!(file.attributions[1].commit, fix);
        assert_eq!(file.attributions[1].line_start, 3);
        assert_eq!(file.attributions[1].subject, "Fix tenant isolation");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("access.go")).unwrap(),
            old
        );
        let options = Options {
            history: vec![bundle.clone()],
        };
        validate(&options, std::slice::from_ref(&pin), &[], |_, _| {
            Some((text::sha256(fixed.as_bytes()), fixed.lines().count()))
        })
        .unwrap();
        assert!(
            jsonschema::validator_for(&schema())
                .unwrap()
                .is_valid(&serde_json::to_value(&options).unwrap())
        );
        let mut conflicting = options;
        conflicting.history[0].files[0].attributions[2].subject = "A conflicting subject".into();
        assert!(
            validate(&conflicting, std::slice::from_ref(&pin), &[], |_, _| Some(
                (text::sha256(fixed.as_bytes()), fixed.lines().count())
            ))
            .is_err()
        );

        // A standard revert becomes last-writer evidence, not an active fix claim.
        fixture_git(dir.path(), &["reset", "--hard", "HEAD"]);
        fixture_git(dir.path(), &["revert", "--no-edit", &fix]);
        let mut reverted_pin = pin.clone();
        reverted_pin.commit = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        let reverted = collect(dir.path(), reverted_pin, &["access.go".into()]).unwrap();
        let attribution = reverted.files[0]
            .attributions
            .iter()
            .find(|a| a.line_start == 3)
            .unwrap();
        assert_eq!(assertion_kind(&attribution.subject), "revert_candidate");
        assert!(
            reverted.files[0]
                .attributions
                .iter()
                .all(|a| a.commit != fix)
        );
        assert!(collect(dir.path(), pin.clone(), &["../access.go".into()]).is_err());
        assert!(collect(dir.path(), pin.clone(), &[".env.local".into()]).is_err());
        assert!(collect(dir.path(), pin, &["secrets/token".into()]).is_err());
    }

    #[test]
    fn classification_uses_words_not_substrings_or_package_names() {
        assert!(domains("Author", "identifier_words", "id").is_empty());
        assert!(domains("Discharge", "identifier_words", "id").is_empty());
        assert_eq!(
            domains("CheckTenantACL", "identifier_words", "id")[0]["domain"],
            "authorization"
        );
        assert_eq!(
            domains("RefundPayment", "identifier_words", "id")[0]["domain"],
            "payments"
        );
        assert_eq!(
            assertion_kind("Revert \"Fix payment retries\""),
            "revert_candidate"
        );
    }

    #[test]
    fn batched_metadata_keeps_commit_identity_and_deadline() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("a.go"), "package a\n").unwrap();
        fixture_git(dir.path(), &["add", "a.go"]);
        fixture_git(dir.path(), &["commit", "-qm", "First subject"]);
        let first = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        std::fs::write(dir.path().join("a.go"), "package a\nfunc F() {}\n").unwrap();
        fixture_git(dir.path(), &["commit", "-qam", "Second subject"]);
        let second = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        let commits = BTreeSet::from([first.clone(), second.clone()]);
        let records = commit_subjects(dir.path(), &commits, "repo", Instant::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[&first].0, "First subject");
        assert_eq!(records[&second].0, "Second subject");
        assert_eq!(records[&first].1, records[&second].1);
        assert!(records[&first].2 > 0);
        assert!(commit_subjects(dir.path(), &commits, "repo", Instant::now()).is_err());
        assert!(commit_subjects(dir.path(), &BTreeSet::from(["-bad".into()]), "repo", Instant::now() + Duration::from_secs(5)).is_err());
        assert!(commit_subjects(dir.path(), &BTreeSet::from(["0".repeat(40)]), "repo", Instant::now() + Duration::from_secs(5)).is_err());
        let large_subject = "x".repeat(70_000);
        fixture_git(dir.path(), &["commit", "--allow-empty", "-qm", &large_subject]);
        let oversized = fixture_git(dir.path(), &["rev-parse", "HEAD"]);
        assert!(commit_subjects(dir.path(), &BTreeSet::from([first, oversized]), "repo", Instant::now() + Duration::from_secs(5)).is_err());
    }

    #[test]
    fn metadata_batches_keep_more_than_128_retained_commits() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        for i in 0..130 {
            fixture_git(dir.path(), &["commit", "--allow-empty", "-qm", &format!("Retained record {i}")]);
        }
        let ids = fixture_git(dir.path(), &["log", "--format=%H"]);
        let commits = ids.lines().map(str::to_owned).collect();
        let records = commit_subjects(dir.path(), &commits, "repo", Instant::now() + Duration::from_secs(5)).unwrap();
        assert_eq!(records.len(), 130);
        assert!(records.values().any(|v| v.0 == "Retained record 0"));
        assert!(records.values().any(|v| v.0 == "Retained record 129"));
    }

    #[test]
    fn git_output_limits_and_deadlines_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        assert!(
            git(
                dir.path(),
                &["rev-parse", "--git-dir"],
                Instant::now() + Duration::from_secs(1),
                1
            )
            .is_err()
        );
        assert!(
            git(
                dir.path(),
                &["rev-parse", "--git-dir"],
                Instant::now(),
                1024
            )
            .is_err()
        );
    }

    #[test]
    fn annotation_groups_commits_and_counts_omitted_assertions() {
        let pin = Pin {
            repository_id: "repo".into(),
            directory: "app".into(),
            commit: "a".repeat(40),
            snapshot: "b".repeat(64),
            policy_sha256: "c".repeat(64),
            modules: vec![".".into()],
            excluded_modules: vec![],
        };
        let file = FileHistory {
            path: "access.go".into(),
            file_sha256: "d".repeat(64),
            line_count: 6,
            omitted_ranges: 7,
            attributions: (1..=6)
                .map(|i| Attribution {
                    commit: format!("{:040x}", if i == 3 { 1 } else { i }),
                    line_start: i,
                    line_end: i,
                    subject: "Fix tenant isolation".into(),
                    author_id: None,
                    timestamp: None,
                })
                .collect(),
        };
        let options = Options {
            history: vec![History {
                format: FORMAT.into(),
                pin: pin.clone(),
                shallow: true,
                files: vec![file],
                evolution: None,
            }],
        };
        let mut report = json!({"entries":[{"id":"entry","symbol_id":"symbol","name":"example.Check","side":"head","repository_id":"repo","file":"access.go","line":1,"end_line":5,"revision":"old"}]});
        annotate(&mut report, &options, &[], &[pin], 8000);
        let context = &report["entries"][0]["business_context"];
        assert_eq!(
            context["historical_assertions"].as_array().unwrap().len(),
            3
        );
        assert_eq!(context["omitted_assertions"], 1);
        assert_eq!(
            context["historical_assertions"][0]["ranges"],
            json!([{"line_start":1,"line_end":1},{"line_start":3,"line_end":3}])
        );
        assert_eq!(context["history"]["shallow"], true);
        assert_eq!(context["history"]["collector_omitted_ranges"], 7);
        assert_ne!(report["entries"][0]["revision"], "old");
        assert_eq!(report["business_context"]["complete"], false);
    }
}
