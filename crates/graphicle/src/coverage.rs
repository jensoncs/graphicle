//! Caller-attested execution observations, bound to immutable sources and a build.
//! Observations supplement static evidence; absence never proves non-execution.
use crate::{
    Error, Result,
    review::{Pin, Profile},
    store::{self, Store, s},
};
use graphicle_memory::text;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Range {
    path: String,
    line_start: usize,
    line_end: usize,
    /// Omit for suite-level coverage. Never infer a test from a covered file.
    #[serde(default)]
    test_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Upload {
    repository_id: String,
    commit: String,
    snapshot: String,
    policy_sha256: String,
    profile: Profile,
    environment: String,
    format: String,
    ranges: Vec<Range>,
}

pub fn upload(store: &Store, workspace: &str, token: &str, args: &Value) -> Result<Value> {
    let repository = crate::string(args, "repository_id", 100)?;
    store.authorized(workspace, token, "write", |db, p| {
        store::repo(db, p, repository)?;
        if args.to_string().len() > 4 * 1024 * 1024 {
            return Err(Error::bad("Coverage upload exceeds 4 MiB"));
        }
        let upload: Upload = serde_json::from_value(args.clone())
            .map_err(|e| Error::bad(format!("Expected pinned coverage request: {e}")))?;
        store::hash(&upload.snapshot)?;
        store::hash(&upload.policy_sha256)?;
        crate::review::validate_profile(&upload.profile)?;
        if upload.format != "graphicle.coverage.v1" || upload.environment.is_empty()
            || upload.environment.len() > 200 || upload.ranges.len() > 20_000 {
            return Err(Error::bad("Use graphicle.coverage.v1, an environment and at most 20000 ranges"));
        }
        let binding = store::one(db, "SELECT snapshot FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?",
            params![repository, upload.snapshot, upload.commit, upload.policy_sha256])?;
        if binding.is_none() { return Err(Error::conflict("Coverage needs an admitted commit/snapshot/policy binding")); }
        let snapshot = store::one(db, "SELECT manifest FROM repo_snapshots WHERE repository_id=? AND sha256=?", params![repository, upload.snapshot])?
            .ok_or_else(|| Error::conflict("Coverage snapshot unavailable"))?;
        let manifest: Value = serde_json::from_str(s(&snapshot, "manifest"))?;
        if crate::digest(&manifest) != upload.snapshot { return Err(Error::conflict("Coverage snapshot failed integrity verification")); }
        let mut files = BTreeMap::new();
        for range in &upload.ranges {
            store::path(&range.path)?;
            if range.line_start == 0 || range.line_start > range.line_end
                || range.test_id.as_ref().is_some_and(|id| id.is_empty() || id.len() > 300) {
                return Err(Error::bad("Invalid coverage range or test identity"));
            }
            if !files.contains_key(&range.path) {
                let entry = &manifest[&range.path];
                let source = store::one(db, "SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?", params![repository, s(entry, "sha256")])?
                    .ok_or_else(|| Error::conflict("Coverage source is absent from the pinned snapshot"))?;
                let content = s(&source, "content");
                if text::sha256(content.as_bytes()) != s(entry, "sha256") || entry["bytes"].as_u64() != Some(content.len() as u64) {
                    return Err(Error::conflict("Coverage source failed integrity verification"));
                }
                files.insert(range.path.clone(), content.lines().count());
            }
            if range.line_end > files[&range.path] { return Err(Error::bad("Coverage range exceeds pinned source")); }
        }
        let data = serde_json::to_value(&upload)?;
        let id = crate::digest(&data);
        let inserted = db.execute("INSERT OR IGNORE INTO repo_review_coverage(id,repository_id,snapshot,commit_id,policy_sha256,profile_sha256,data,created_at) VALUES(?,?,?,?,?,?,?,?)",
            params![id, repository, upload.snapshot, upload.commit, upload.policy_sha256, crate::digest(&serde_json::to_value(&upload.profile)?), data.to_string(), crate::now()])?;
        // Bound retained observations independently of snapshot retention.
        db.execute("DELETE FROM repo_review_coverage WHERE repository_id=? AND id NOT IN (SELECT id FROM repo_review_coverage WHERE repository_id=? ORDER BY rowid DESC LIMIT 20)", params![repository, repository])?;
        Ok(json!({"id":id,"replayed":inserted==0,"snapshot":upload.snapshot,"commit":upload.commit,"ranges":upload.ranges.len(),"files":files.len(),"authority":"caller_attested","complete_test_inventory":false}))
    })
}

/// Enrich selected symbols only with observations from the exact selected build.
/// Changes to observations change entry revisions and therefore review deltas.
pub(crate) fn annotate(
    db: &rusqlite::Connection,
    report: &mut Value,
    base: &[Pin],
    head: &[Pin],
    profile: &Profile,
) -> Result<Vec<String>> {
    let profile_sha = crate::digest(&serde_json::to_value(profile)?);
    let mut ids = vec![];
    for (side, pins) in [("base", base), ("head", head)] {
        for pin in pins {
            let runs = store::rows(
                db,
                "SELECT id,data FROM repo_review_coverage WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=? AND profile_sha256=? ORDER BY id",
                params![
                    pin.repository_id,
                    pin.snapshot,
                    pin.commit,
                    pin.policy_sha256,
                    profile_sha
                ],
            )?;
            for run in runs {
                let data: Upload = serde_json::from_str(s(&run, "data"))?;
                ids.push(s(&run, "id").to_string());
                for entry in report["entries"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .filter(|e| e["side"] == side && e["repository_id"] == pin.repository_id)
                {
                    let matches: Vec<_> = data
                        .ranges
                        .iter()
                        .filter(|r| {
                            r.path == s(entry, "file")
                                && r.line_start as u64 <= entry["end_line"].as_u64().unwrap()
                                && r.line_end as u64 >= entry["line"].as_u64().unwrap()
                        })
                        .collect();
                    if matches.is_empty() {
                        continue;
                    }
                    if entry.get("execution_observations").is_none() {
                        entry["execution_observations"] = json!([]);
                    }
                    entry["execution_observations"].as_array_mut().unwrap().push(json!({"run_id":run["id"],"environment":data.environment,"authority":"caller_attested","matched_range_count":matches.len(),"ranges":matches.iter().take(20).collect::<Vec<_>>(),"omitted_ranges":matches.len().saturating_sub(20)}));
                }
            }
        }
    }
    for entry in report["entries"].as_array_mut().unwrap() {
        if entry.get("execution_observations").is_some() {
            entry.as_object_mut().unwrap().remove("revision");
            entry["revision"] = json!(crate::digest(entry));
        }
    }
    ids.sort();
    ids.dedup();
    report["execution_coverage"] = json!({"matching_runs":ids.len(),"authority":"caller_attested","complete_test_inventory":false,"absence_means":"unknown"});
    Ok(ids)
}
