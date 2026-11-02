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

pub(crate) fn missing_page(
    db: &Connection,
    repository: &str,
    sync: &Value,
    args: &Value,
) -> Result<Value> {
    let limit = integer(args, "missing_limit", 256, 1024)?.max(1);
    let manifest: BTreeMap<String, Value> = serde_json::from_str(s(sync, "manifest"))?;
    let after = match args.get("cursor").filter(|v| !v.is_null()) {
        None => String::new(),
        Some(_) => {
            let cursor = string(args, "cursor", 16000)?;
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| Error::bad("Invalid missing cursor"))?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| Error::bad("Invalid missing cursor"))?;
            if value["sync_id"] != sync["id"]
                || value["snapshot"] != sync["snapshot"]
                || !manifest.contains_key(s(&value, "after"))
            {
                return Err(Error::bad("Missing cursor does not belong to this sync"));
            }
            s(&value, "after").to_owned()
        }
    };
    let mut exists = db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM repo_objects WHERE repository_id=? AND sha256=? AND bytes=?)",
    )?;
    let (mut missing, mut seen) = (Vec::new(), BTreeSet::new());
    let (mut total, mut bytes, mut more) = (0usize, 0usize, false);
    for (name, item) in &manifest {
        // A single upload supplies every path with these bytes. The first
        // lexical path is a stable representative, even between page uploads.
        if !seen.insert(s(item, "sha256"))
            || exists.query_row(
                params![repository, s(item, "sha256"), item["bytes"].as_u64()],
                |r| r.get::<_, bool>(0),
            )?
        {
            continue;
        }
        total += 1;
        if name <= &after {
            continue;
        }
        let cost = serde_json::to_vec(name)?.len() + 1;
        if missing.len() >= limit || bytes + cost > PAGE_BYTES {
            more = true;
            continue;
        }
        // A byte bound, like the item bound, ends a contiguous lexical page.
        if more {
            continue;
        }
        bytes += cost;
        missing.push(name.clone());
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
