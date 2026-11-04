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
    // Checksums verified on load or written in the final transaction. Native
    // replacements invalidate these, including repaired export data.
    persisted: BTreeMap<String, String>,
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
            if self.entries.get(&key) != Some(&data) {
                self.persisted.remove(&key);
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
            pool.persisted.insert(key.clone(), sha);
            pool.entries.insert(key, data);
        }
        Ok(pool)
    })
}

/// Save only after the final authorization, retention and binding checks.
pub(crate) fn save_packages(
    db: &Connection,
    anchor: &str,
    pool: &mut PackagePool,
    used: &[String],
    pins: &[Pin],
) -> Result<()> {
    if used.is_empty() { return Ok(()); }
    for key in used.iter().take(PACKAGE_COUNT) {
        let Some(data) = pool.entries.get(key) else {
            continue;
        };
        if store::hash(key).is_err() || data.len() > PACKAGE_ENTRY_BYTES {
            continue;
        }
        let refreshed = if let Some(sha) = pool.persisted.get(key) {
            db.execute("UPDATE repo_review_package_facts SET used_at=? WHERE repository_id=? AND id=? AND sha256=?",params![crate::now(),anchor,key,sha])? > 0
        } else { false };
        if !refreshed {
            let sha = graphicle_memory::text::sha256(data.as_bytes());
            db.execute("INSERT INTO repo_review_package_facts(repository_id,id,data,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET data=excluded.data,sha256=excluded.sha256,used_at=excluded.used_at", params![anchor,key,data,sha,crate::now()])?;
            pool.persisted.insert(key.clone(), sha);
        }
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
) -> Result<Option<(Value, String)>> {
    store.authorized_read(workspace, token, "read", |db, principal| {
        store::repo(db, principal, anchor)?;
        let Some(row) = store::one(db, "SELECT facts,sha256 FROM repo_review_analysis WHERE repository_id=? AND id=?", params![anchor,key])? else {return Ok(None)};
        for input in store::rows(db,"SELECT input_repository FROM repo_review_analysis_inputs WHERE repository_id=? AND id=?",params![anchor,key])? {
            if store::repo(db,principal,s(&input,"input_repository")).is_err() {return Ok(None)}
        }
        let facts:Value=serde_json::from_str(s(&row,"facts"))?;
        let sha = s(&row,"sha256");
        Ok((crate::digest(&facts)==sha).then(|| (facts,sha.to_owned())))
    })
}

/// Called in the final transaction after the review rechecked every input.
pub(crate) fn save(
    db: &Connection,
    anchor: &str,
    key: &str,
    facts: &Value,
    pins: &[Pin],
    verified_sha: Option<&str>,
) -> Result<()> {
    // A cache hit was already checksummed on load. Refresh it without encoding
    // the transport again. If concurrent eviction/replacement removed that row,
    // restore the validated facts after the caller's final retention checks.
    let refreshed = if let Some(sha) = verified_sha {
        db.execute("UPDATE repo_review_analysis SET used_at=? WHERE repository_id=? AND id=? AND sha256=?",params![crate::now(),anchor,key,sha])? > 0
    } else { false };
    if !refreshed {
        let encoded = facts.to_string();
        // Match the native transport bound; facts use gzip/base64 here.
        if encoded.len() > 64 * 1024 * 1024 { return Ok(()); }
        db.execute("INSERT INTO repo_review_analysis(repository_id,id,facts,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET facts=excluded.facts,sha256=excluded.sha256,used_at=excluded.used_at",params![anchor,key,encoded,crate::digest(facts),crate::now()])?;
    }
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
    #[ignore = "manual before/after cache-persistence benchmark"]
    fn cache_persistence_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("cache-persistence-benchmark").unwrap();
        let repo = store.register(s(&ws,"id"), s(&ws,"capability"), &json!({"project_key":"bench","name":"bench"})).unwrap();
        let anchor = s(&repo,"id");
        let mut pool = PackagePool::default();
        for n in 0..8 {
            pool.entries.insert(format!("{n:064x}"), "x".repeat(4 * 1024 * 1024));
        }
        let used: Vec<_> = pool.entries.keys().cloned().collect();
        let facts = json!({"opaque_compiler_transport":"x".repeat(16 * 1024 * 1024)});
        let key = "a".repeat(64);
        store.transaction(|db| {
            save_packages(db,anchor,&mut pool,&used,&[])?;
            save(db,anchor,&key,&facts,&[],None)
        }).unwrap();
        let (_, verified) = load(&store,s(&ws,"id"),s(&ws,"capability"),anchor,&key).unwrap().unwrap();
        for kind in ["packages_32mib", "analysis_16mib"] {
            let mut elapsed = vec![];
            for n in 0..8 {
                let start = std::time::Instant::now();
                store.transaction(|db| {
                    if kind == "packages_32mib" { save_packages(db,anchor,&mut pool,&used,&[]) }
                    else { save(db,anchor,&key,&facts,&[],Some(&verified)) }
                }).unwrap();
                if n > 0 { elapsed.push(start.elapsed().as_secs_f64() * 1000.0); }
            }
            eprintln!("cache_persistence_benchmark kind={kind} samples_ms={elapsed:?}");
        }
    }

    #[test]
    fn cache_refresh_restores_evicted_rows_and_replaces_changed_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("cache-refresh").unwrap();
        let repo = store.register(s(&ws,"id"),s(&ws,"capability"),&json!({"project_key":"refresh","name":"refresh"})).unwrap();
        let anchor = s(&repo,"id");
        let key = "a".repeat(64);
        let facts = json!({"facts":"verified"});
        let mut pool = PackagePool::default();
        pool.entries.insert(key.clone(), "original".into());
        store.transaction(|db| {
            save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])?;
            save(db,anchor,&key,&facts,&[],None)
        }).unwrap();
        let (_, verified) = load(&store,s(&ws,"id"),s(&ws,"capability"),anchor,&key).unwrap().unwrap();
        pool = load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor).unwrap();
        for eviction in [false, true] {
            store.transaction(|db| {
                if eviction {
                    db.execute("DELETE FROM repo_review_analysis", [])?;
                    db.execute("DELETE FROM repo_review_package_facts", [])?;
                } else {
                    db.execute("UPDATE repo_review_analysis SET sha256='changed'", [])?;
                    db.execute("UPDATE repo_review_package_facts SET sha256='changed'", [])?;
                }
                save(db,anchor,&key,&facts,&[],Some(&verified))?;
                save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])?;
                Ok(())
            }).unwrap();
            assert_eq!(load(&store,s(&ws,"id"),s(&ws,"capability"),anchor,&key).unwrap().unwrap().0, facts);
            assert_eq!(load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor).unwrap().entries[&key], "original");
        }
        let mut repaired = PackageResult::default();
        repaired.entries.insert(key.clone(), "repaired by native compiler".into());
        pool.absorb(&mut repaired);
        store.transaction(|db| save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])).unwrap();
        assert_eq!(load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor).unwrap().entries[&key], "repaired by native compiler");
    }

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
            save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[pin])?;
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
            save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])?;
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
