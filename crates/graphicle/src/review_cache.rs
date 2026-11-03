//! Store-owned immutable compiler facts. All cached spans are verified again;
//! cache lookup never replaces source authorization or pin validation.
use crate::{
    Result,
    review::Pin,
    store::{self, Store, s},
};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const PACKAGE_ENTRY_BYTES: usize = 4 * 1024 * 1024;
const PACKAGE_COUNT: usize = 4096;

/// The bounded package pool is scoped to an authorized anchor repository.
/// Package keys are independently recomputed from the current native build.
#[derive(Default)]
pub(crate) struct PackagePool {
    pub identity: String,
    pub entries: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct PackageResult {
    pub entries: BTreeMap<String, String>,
    pub used: Vec<String>,
    pub hits: usize,
    pub misses: usize,
    pub rejected: usize,
}

impl PackagePool {
    pub fn absorb(&mut self, result: &mut PackageResult) {
        let mut bytes: usize = self.entries.values().map(String::len).sum();
        for (key, data) in std::mem::take(&mut result.entries) {
            if store::hash(&key).is_err() || data.len() > PACKAGE_ENTRY_BYTES {
                continue;
            }
            let previous = self.entries.get(&key).map_or(0, String::len);
            let total = bytes - previous + data.len();
            if total > PACKAGE_BYTES || (previous == 0 && self.entries.len() >= PACKAGE_COUNT) {
                continue;
            }
            self.entries.insert(key, data);
            bytes = total;
        }
        result.used.retain(|key| self.entries.contains_key(key));
        result.used.sort();
        result.used.dedup();
    }
}

pub(crate) fn load_packages(
    store: &Store,
    workspace: &str,
    token: &str,
    anchor: &str,
) -> Result<PackagePool> {
    store.authorized_read(workspace, token, "read", |db, principal| {
        store::repo(db, principal, anchor)?;
        let mut denied = BTreeSet::new();
        let mut access = BTreeMap::new();
        for input in store::rows(db,"SELECT DISTINCT id,input_repository FROM repo_review_package_inputs WHERE repository_id=?",[anchor])? {
            let repository = s(&input,"input_repository");
            if !*access.entry(repository.to_owned()).or_insert_with(|| store::repo(db,principal,repository).is_ok()) {
                denied.insert(s(&input,"id").to_owned());
            }
        }
        let mut pool = PackagePool::default();
        let mut bytes = 0;
        let mut query = db.prepare("SELECT id,data,sha256 FROM repo_review_package_facts WHERE repository_id=? AND length(data)<=? ORDER BY used_at DESC,rowid DESC LIMIT ?")?;
        let mut rows = query.query(params![anchor,PACKAGE_ENTRY_BYTES,PACKAGE_COUNT])?;
        while let Some(row) = rows.next()? {
            let key: String = row.get(0)?;
            if denied.contains(&key) || store::hash(&key).is_err() { continue; }
            let data: String = row.get(1)?;
            // Leave room for newly checked packages in the bounded native pool.
            if bytes + data.len() > PACKAGE_BYTES / 2 { continue; }
            let sha: String = row.get(2)?;
            if graphicle_memory::text::sha256(data.as_bytes()) != sha { continue; }
            bytes += data.len();
            pool.entries.insert(key, data);
        }
        Ok(pool)
    })
}

/// Save only after the final authorization, retention and binding checks.
pub(crate) fn save_packages(
    db: &Connection,
    anchor: &str,
    pool: &PackagePool,
    used: &[String],
    pins: &[Pin],
) -> Result<()> {
    for key in used.iter().take(PACKAGE_COUNT) {
        let Some(data) = pool.entries.get(key) else {
            continue;
        };
        if store::hash(key).is_err() || data.len() > PACKAGE_ENTRY_BYTES {
            continue;
        }
        db.execute("INSERT INTO repo_review_package_facts(repository_id,id,data,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET data=excluded.data,sha256=excluded.sha256,used_at=excluded.used_at", params![anchor,key,data,graphicle_memory::text::sha256(data.as_bytes()),crate::now()])?;
        for pin in pins {
            db.execute("INSERT OR IGNORE INTO repo_review_package_inputs(repository_id,id,input_repository,snapshot) VALUES(?,?,?,?)",params![anchor,key,pin.repository_id,pin.snapshot])?;
        }
    }
    // Enforce both byte and entry limits, evicting least-recently-used facts.
    db.execute("DELETE FROM repo_review_package_facts WHERE repository_id=? AND id NOT IN (SELECT id FROM (SELECT id,row_number() OVER (ORDER BY used_at DESC,rowid DESC) AS n,sum(length(data)) OVER (ORDER BY used_at DESC,rowid DESC) AS bytes FROM repo_review_package_facts WHERE repository_id=?) WHERE n<=? AND bytes<=?)",params![anchor,anchor,PACKAGE_COUNT,PACKAGE_BYTES])?;
    Ok(())
}

pub(crate) fn load(
    store: &Store,
    workspace: &str,
    token: &str,
    anchor: &str,
    key: &str,
) -> Result<Option<Value>> {
    store.authorized_read(workspace, token, "read", |db, principal| {
        store::repo(db, principal, anchor)?;
        let Some(row) = store::one(db, "SELECT facts,sha256 FROM repo_review_analysis WHERE repository_id=? AND id=?", params![anchor,key])? else {return Ok(None)};
        for input in store::rows(db,"SELECT input_repository FROM repo_review_analysis_inputs WHERE repository_id=? AND id=?",params![anchor,key])? {
            if store::repo(db,principal,s(&input,"input_repository")).is_err() {return Ok(None)}
        }
        let facts:Value=serde_json::from_str(s(&row,"facts"))?;
        Ok((crate::digest(&facts)==s(&row,"sha256")).then_some(facts))
    })
}

/// Called in the final transaction after the review rechecked every input.
pub(crate) fn save(
    db: &Connection,
    anchor: &str,
    key: &str,
    facts: &Value,
    pins: &[Pin],
) -> Result<()> {
    let encoded = facts.to_string();
    // Match the native transport bound; facts use gzip/base64 on this boundary.
    if encoded.len() > 64 * 1024 * 1024 {
        return Ok(());
    }
    db.execute("INSERT INTO repo_review_analysis(repository_id,id,facts,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET used_at=excluded.used_at",params![anchor,key,encoded,crate::digest(facts),crate::now()])?;
    for pin in pins {
        db.execute("INSERT OR IGNORE INTO repo_review_analysis_inputs(repository_id,id,input_repository,snapshot) VALUES(?,?,?,?)",params![anchor,key,pin.repository_id,pin.snapshot])?;
    }
    db.execute("DELETE FROM repo_review_analysis WHERE repository_id=? AND id NOT IN (SELECT id FROM repo_review_analysis WHERE repository_id=? ORDER BY used_at DESC,rowid DESC LIMIT 4)",params![anchor,anchor])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn package_pool_enforces_entry_and_total_bounds() {
        let mut pool = PackagePool::default();
        let mut result = PackageResult::default();
        for n in 0..PACKAGE_COUNT + 1 {
            result.entries.insert(format!("{n:064x}"), "data".into());
        }
        result.entries.insert("not-a-hash".into(), "data".into());
        result.used = result.entries.keys().cloned().collect();
        pool.absorb(&mut result);
        assert_eq!(pool.entries.len(), PACKAGE_COUNT);
        assert_eq!(result.used.len(), PACKAGE_COUNT);
        let mut pool = PackagePool::default();
        let mut result = PackageResult::default();
        for n in 0..17 {
            result
                .entries
                .insert(format!("{n:064x}"), "x".repeat(PACKAGE_ENTRY_BYTES));
        }
        result
            .entries
            .insert("f".repeat(64), "x".repeat(PACKAGE_ENTRY_BYTES + 1));
        pool.absorb(&mut result);
        assert_eq!(
            pool.entries.values().map(String::len).sum::<usize>(),
            PACKAGE_BYTES
        );
        assert_eq!(pool.entries.len(), 16);
    }

    #[test]
    fn package_store_rechecks_input_access_and_evicts_old_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("package-cache-test").unwrap();
        let workspace = s(&ws, "id");
        let owner = s(&ws, "capability");
        let anchor = store
            .register(
                workspace,
                owner,
                &json!({"project_key":"anchor","name":"anchor"}),
            )
            .unwrap();
        let input = store
            .register(
                workspace,
                owner,
                &json!({"project_key":"input","name":"input"}),
            )
            .unwrap();
        let anchor = s(&anchor, "id");
        let input = s(&input, "id");
        let reader = store
            .grant(workspace, owner, &json!({"label":"reader","role":"reader"}))
            .unwrap();
        let mut pool = PackagePool::default();
        let key = "a".repeat(64);
        pool.entries.insert(key.clone(), "payload".into());
        store.transaction(|db| {
            db.execute("INSERT INTO repo_snapshots VALUES(?,?,'{}','{}',?)",params![input,"snapshot",crate::now()])?;
            let pin: Pin = serde_json::from_value(json!({"repository_id":input,"snapshot":"snapshot","commit":"commit","policy_sha256":"policy","directory":"input"})).unwrap();
            save_packages(db,anchor,&pool,std::slice::from_ref(&key),&[pin])?;
            Ok(())
        }).unwrap();
        assert_eq!(
            load_packages(&store, workspace, s(&reader, "capability"), anchor)
                .unwrap()
                .entries
                .len(),
            1
        );
        store
            .transaction(|db| {
                crate::graph_projection::set_repo_access(
                    db,
                    workspace,
                    input,
                    s(&reader, "id"),
                    "revoked",
                )
            })
            .unwrap();
        assert!(
            load_packages(&store, workspace, s(&reader, "capability"), anchor)
                .unwrap()
                .entries
                .is_empty()
        );
        assert_eq!(
            load_packages(&store, workspace, owner, anchor)
                .unwrap()
                .entries
                .len(),
            1
        );
        store.transaction(|db| {
            db.execute("DELETE FROM repo_snapshots WHERE repository_id=?",[input])?;
            let count: usize = db.query_row("SELECT count(*) FROM repo_review_package_facts",[],|row|row.get(0))?;
            assert_eq!(count,0);
            // The store applies its own quota, independent of transport bounds.
            for n in 0..PACKAGE_COUNT + 2 {
                db.execute("INSERT INTO repo_review_package_facts VALUES(?,?,?,?,?)",params![anchor,format!("{n:064x}"),"x",graphicle_memory::text::sha256(b"x"),"old"])?;
            }
            save_packages(db,anchor,&pool,std::slice::from_ref(&key),&[])?;
            let (count,oldest): (usize,usize) = db.query_row("SELECT count(*),sum(id=?) FROM repo_review_package_facts WHERE repository_id=?",params![format!("{:064x}",0),anchor],|r|Ok((r.get(0)?,r.get(1)?)))?;
            assert_eq!(count,PACKAGE_COUNT);
            assert_eq!(oldest,0);
            db.execute("DELETE FROM repo_projects WHERE id=?",[anchor])?;
            let count: usize = db.query_row("SELECT count(*) FROM repo_review_package_facts",[],|row|row.get(0))?;
            assert_eq!(count,0);
            Ok(())
        }).unwrap();
    }
}
