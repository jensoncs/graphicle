//! Bounded sync discovery and explicit, authorized reuse of immutable sources.
use crate::{
    Error, Result, integer,
    store::{self, Principal, Store, s},
    string,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_BYTES: usize = 128 * 1024;
// Admission guarantees one size per digest. Keep the first lexical path as its
// representative, even after uploads. Count against live objects in the same
// read snapshot as the page; a cached missing list would miss uploads/removals.
// SQLite batches the indexed object probes without one Rust query per object.
const MISSING_QUERY: &str = "
WITH manifest AS MATERIALIZED (
    SELECT key AS path, json_extract(value,'$.sha256') AS sha,
           json_extract(value,'$.bytes') AS bytes FROM json_each(?1)
), objects AS MATERIALIZED (
    SELECT min(path) AS path, sha, min(bytes) AS bytes FROM manifest GROUP BY sha
), missing AS MATERIALIZED (
    SELECT m.path FROM objects m WHERE NOT EXISTS (
        SELECT 1 FROM repo_objects o INDEXED BY repo_objects_size
        WHERE o.repository_id=?2 AND o.sha256=m.sha AND o.bytes=m.bytes
    )
)
SELECT total, (?3 IS NULL OR EXISTS(SELECT 1 FROM manifest WHERE path=?3)),
       page.path, json_type(?1)='object'
FROM (SELECT count(*) AS total FROM missing)
LEFT JOIN (
    SELECT path FROM missing WHERE path>coalesce(?3,'') ORDER BY path LIMIT ?4
) page ON 1
ORDER BY page.path";

#[cfg(test)]
#[path = "sync_protocol_tests.rs"]
mod tests;

pub(crate) fn missing_page(
    db: &Connection,
    repository: &str,
    sync: &Value,
    args: &Value,
) -> Result<Value> {
    let limit = integer(args, "missing_limit", 256, 1024)?.max(1);
    let after = match args.get("cursor").filter(|v| !v.is_null()) {
        None => None,
        Some(_) => {
            let cursor = string(args, "cursor", 16000)?;
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| Error::bad("Invalid missing cursor"))?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid missing cursor"))?;
            if value["sync_id"] != sync["id"] || value["snapshot"] != sync["snapshot"] {
                return Err(Error::bad("Missing cursor does not belong to this sync"));
            }
            Some(s(&value, "after").to_owned())
        }
    };
    let mut query = db.prepare_cached(MISSING_QUERY)?;
    let mut rows = query.query(params![s(sync, "manifest"), repository, after, limit + 1])?;
    let mut missing = Vec::new();
    let (mut total, mut bytes, mut more) = (0usize, 0usize, false);
    while let Some(row) = rows.next()? {
        if !row.get::<_, bool>(3)? {
            return Err(Error::bad("Invalid sync manifest"));
        }
        total = row.get(0)?;
        if !row.get::<_, bool>(1)? {
            return Err(Error::bad("Missing cursor does not belong to this sync"));
        }
        let Some(name) = row.get::<_, Option<String>>(2)? else {
            break;
        };
        let cost = serde_json::to_vec(&name)?.len() + 1;
        if missing.len() >= limit || bytes + cost > PAGE_BYTES {
            more = true;
            break;
        }
        bytes += cost;
        missing.push(name);
    }
    let cursor = if more {
        missing.last().map(|last| {
            URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(
                    &json!({"sync_id":sync["id"],"snapshot":sync["snapshot"],"after":last}),
                )
                .expect("JSON cursor"),
            )
        })
    } else {
        None
    };
    Ok(
        json!({"format":"graphicle.sync-missing.v1","sync_id":sync["id"],"snapshot":sync["snapshot"],"missing":missing,
        "missing_total":total,"next_cursor":cursor,"replayed":false}),
    )
}

/// Copy only objects named by explicit, readable, immutable donor snapshots.
/// A digest alone is never authority to read another project's object store.
pub(crate) fn reuse(
    db: &Connection,
    principal: &Principal,
    target: &str,
    files: &Value,
    args: &Value,
) -> Result<Value> {
    let donors: &[Value] = match args.get("reuse_sources").filter(|v| !v.is_null()) {
        None => &[],
        Some(value) => value.as_array().filter(|v| v.len() <= 8).ok_or_else(|| {
            Error::bad("reuse_sources must contain at most 8 snapshot references")
        })?,
    };
    if !donors.is_empty() && !store::permitted(db, principal, "read")? {
        return Err(Error::denied());
    }
    let mut wanted = BTreeMap::new();
    for item in files
        .as_object()
        .ok_or_else(|| Error::bad("Invalid sync manifest"))?
        .values()
    {
        let sha = s(item, "sha256");
        let bytes = item["bytes"]
            .as_u64()
            .ok_or_else(|| Error::bad("Invalid source size"))?;
        if wanted
            .insert(sha.to_owned(), bytes)
            .is_some_and(|old| old != bytes)
        {
            return Err(Error::bad("Conflicting sizes for one source digest"));
        }
    }
    let (mut objects, mut bytes) = (0, 0usize);
    for donor in donors {
        let repository = store::repo(db, principal, string(donor, "repository_id", 100)?)?;
        let source = s(&repository, "id");
        let snapshot = store::hash(string(donor, "snapshot", 64)?)?;
        let saved = store::one(
            db,
            "SELECT manifest FROM repo_snapshots WHERE repository_id=? AND sha256=?",
            params![source, snapshot],
        )?
        .ok_or_else(|| Error::conflict("Reuse snapshot unavailable"))?;
        let manifest: Value = serde_json::from_str(s(&saved, "manifest"))?;
        if crate::digest(&manifest) != snapshot {
            return Err(Error::conflict("Reuse snapshot integrity check failed"));
        }
        let allowed: BTreeSet<_> = manifest
            .as_object()
            .ok_or_else(|| Error::bad("Invalid reuse manifest"))?
            .values()
            .filter_map(|v| Some((v["sha256"].as_str()?, v["bytes"].as_u64()?)))
            .collect();
        for (sha, size) in &wanted {
            if !allowed.contains(&(sha.as_str(), *size)) {
                continue;
            }
            let present: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)",params![target,sha,size],|r|r.get(0))?;
            if present {
                continue;
            }
            let Some(content) = crate::source_object::load(db, source, sha)? else {
                continue;
            };
            if content.bytes().len() as u64 != *size {
                return Err(Error::conflict("Reuse source size mismatch"));
            }
            db.execute("INSERT OR IGNORE INTO repo_objects(repository_id,sha256,content,bytes) VALUES(?,?,?,?)",params![target,sha,content.sql(),size])?;
            objects += 1;
            bytes += content.bytes().len();
        }
    }
    Ok(json!({"objects":objects,"bytes":bytes}))
}

impl Store {
    pub fn sync_missing(&self, workspace: &str, token: &str, args: &Value) -> Result<Value> {
        self.authorized_read(workspace, token, "collect", |db, p| {
            let repository = store::repo(db, p, string(args, "repository_id", 100)?)?;
            let sync = store::one(
                db,
                "SELECT * FROM repo_syncs WHERE repository_id=? AND id=?",
                params![s(&repository, "id"), string(args, "sync_id", 100)?],
            )?
            .ok_or_else(|| Error::bad("Sync not found"))?;
            if s(&sync, "state") != "pending" {
                return Err(Error::conflict("Sync is not pending"));
            }
            missing_page(db, s(&repository, "id"), &sync, args)
        })
    }
}
