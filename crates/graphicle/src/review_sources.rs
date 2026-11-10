//! Request-local verified source sharing and final retention checks.
use super::*;
use crate::source_object::Content;
use std::sync::Arc;

// One pool belongs to one authorized read transaction. Repository identity is
// part of the key: bytes admitted to one repository never supply another's pin.
#[derive(Default)]
struct SourcePool {
    objects: BTreeMap<String, BTreeMap<String, Arc<Content>>>,
}
impl SourcePool {
    fn load(
        &mut self,
        db: &rusqlite::Connection,
        repository: &str,
        sha: &str,
    ) -> Result<Option<Arc<Content>>> {
        if let Some(content) = self
            .objects
            .get(repository)
            .and_then(|objects| objects.get(sha))
        {
            return Ok(Some(Arc::clone(content)));
        }
        let Some(content) = crate::source_object::load(db, repository, sha)? else {
            return Ok(None);
        };
        let content = Arc::new(content);
        self.objects
            .entry(repository.into())
            .or_default()
            .insert(sha.into(), Arc::clone(&content));
        Ok(Some(content))
    }
}

pub(super) fn load_sides(
    store: &Store,
    workspace: &str,
    token: &str,
    request: &Request,
) -> Result<(Input, Input)> {
    // Sharing across independent read transactions could hide removal/corruption
    // between sides. Both manifests and their bytes use the same SQLite snapshot.
    store.authorized_read(workspace, token, "read", |db, principal| {
        let mut objects = SourcePool::default();
        let base = crate::phase::phase("review_load_base", || {
            load(db, principal, &mut objects, &request.base, &request.profile)
        })?;
        let head = crate::phase::phase("review_load_head", || {
            load(db, principal, &mut objects, &request.head, &request.profile)
        })?;
        Ok((base, head))
    })
}

fn load(
    db: &rusqlite::Connection,
    principal: &store::Principal,
    objects: &mut SourcePool,
    pins: &[Pin],
    profile: &Profile,
) -> Result<Input> {
    let mut input = Input {
        pins: pins.to_vec(),
        sources: BTreeMap::new(),
        issues: vec![],
        bindings: vec![],
    };
    let mut budget = admission::SourceBudget::default();
    for pin in pins {
        checkpoint()?;
        if store::repo(db, principal, &pin.repository_id).is_err() {
            input
                .issues
                .push(issue("unavailable_snapshot", json!(pin.directory)));
            continue;
        }
        let row = store::one(
            db,
            "SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?",
            params![pin.repository_id, pin.snapshot],
        )?;
        let Some(row) = row else {
            input
                .issues
                .push(issue("unavailable_snapshot", json!(pin.directory)));
            continue;
        };
        let manifest: Value = serde_json::from_str(s(&row, "manifest"))?;
        let metadata: Value = serde_json::from_str(s(&row, "metadata"))?;
        let binding = store::one(
            db,
            "SELECT scope_complete,withheld_paths FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?",
            params![
                pin.repository_id,
                pin.snapshot,
                pin.commit,
                pin.policy_sha256
            ],
        )?;
        let legacy =
            metadata["commit"] == pin.commit && metadata["policy_sha256"] == pin.policy_sha256;
        if (binding.is_none() && !legacy) || crate::digest(&manifest) != pin.snapshot {
            input
                .issues
                .push(issue("snapshot_binding_mismatch", json!(pin.directory)));
            continue;
        }
        let complete = binding
            .as_ref()
            .map(|v| v["scope_complete"] == 1)
            .unwrap_or(legacy && metadata["scope_complete"] == true);
        let withheld = match &binding {
            Some(row) if !row["withheld_paths"].is_null() => {
                Some(serde_json::from_str::<Vec<String>>(s(
                    row,
                    "withheld_paths",
                ))?)
            }
            Some(_) => None,
            None => admission::withheld_paths(&metadata)?,
        };
        input.bindings.push(admission::Binding {
            pin: pin.clone(),
            complete,
            withheld,
            original: binding,
        });
        if let Some(preparation) = metadata.get("dependency_preparation") {
            let prepared_profile =
                serde_json::from_value::<Profile>(preparation["profile"].clone());
            if prepared_profile
                .ok()
                .and_then(|p| serde_json::to_value(p).ok())
                != Some(serde_json::to_value(profile)?)
            {
                input.issues.push(issue(
                    "dependency_preparation_profile_mismatch",
                    json!(pin.directory),
                ));
            }
        }
        if !complete {
            input
                .issues
                .push(issue("source_admission_incomplete", json!(pin.directory)));
        }
        let files = manifest
            .as_object()
            .ok_or_else(|| Error::bad("Invalid stored source manifest"))?;
        for (path, entry) in files {
            checkpoint()?;
            store::path(path)?;
            if local::excluded_pinned_source_path(path) {
                input
                    .issues
                    .push(issue("excluded_source", json!(pin.directory)));
                continue;
            }
            let size = entry["bytes"]
                .as_u64()
                .ok_or_else(|| Error::bad("Invalid source size"))?;
            budget.reserve(size, admission::vendored(path, files))?;
            let source = objects.load(db, &pin.repository_id, s(entry, "sha256"))?;
            let Some(content) = source else {
                input
                    .issues
                    .push(issue("missing_source_object", json!(pin.directory)));
                continue;
            };
            if content.bytes().len() != size as usize {
                return Err(Error::conflict(
                    "Pinned source failed integrity verification",
                ));
            }
            input.sources.insert(
                format!("{}/{}", pin.directory, path),
                Source {
                    repository: pin.repository_id.clone(),
                    path: path.clone(),
                    sha: s(entry, "sha256").into(),
                    content,
                },
            );
        }
    }
    Ok(input)
}

// Three parameters per object; stay below SQLite's legacy 999-variable bound.
const RETENTION_BATCH: usize = 256;
fn retention_query(count: usize) -> String {
    let values = std::iter::repeat_n("(?,?,?)", count)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "WITH expected(repository_id,sha256,bytes) AS (VALUES {values})
        SELECT NOT EXISTS (
            SELECT 1 FROM expected WHERE NOT EXISTS (
                SELECT 1 FROM repo_objects AS actual INDEXED BY repo_objects_size
                WHERE actual.repository_id=expected.repository_id
                  AND actual.sha256=expected.sha256 AND actual.bytes=expected.bytes
            )
        )"
    )
}

pub(super) fn check_retained<'a>(
    db: &rusqlite::Connection,
    sources: impl Iterator<Item = &'a Source>,
) -> Result<()> {
    // All checks remain in the final authorized transaction. Batching removes
    // repeated statement executions, not the indexed checks for unique objects.
    let objects: BTreeSet<_> = sources
        .map(|s| {
            (
                s.repository.as_str(),
                s.sha.as_str(),
                s.content.bytes().len(),
            )
        })
        .collect();
    let objects: Vec<_> = objects.into_iter().collect();
    for batch in objects.chunks(RETENTION_BATCH) {
        checkpoint()?;
        let sql = retention_query(batch.len());
        let mut query = db.prepare_cached(&sql)?;
        let parameters = batch
            .iter()
            .flat_map(|object| [&object.0 as &dyn rusqlite::ToSql, &object.1, &object.2]);
        if !query.query_row(rusqlite::params_from_iter(parameters), |row| {
            row.get::<_, bool>(0)
        })? {
            return Err(Error::conflict(
                "Pinned source was removed during review; retry with an available snapshot",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "review_sources_tests.rs"]
mod tests;
