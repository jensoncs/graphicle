//! Store-owned immutable compiler facts. All cached spans are verified again;
//! cache lookup never replaces source authorization or pin validation.
use crate::{
    Result,
    review::Pin,
    store::{self, Store, s},
};
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const PACKAGE_ENTRY_BYTES: usize = 4 * 1024 * 1024;
const PACKAGE_COUNT: usize = 4096;

fn cache_note(action: &str, anchor: &str, key: &str, result: &str) {
    crate::phase::note(action, &format!("anchor={anchor} key={key} result={result}"));
}

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
    pub fn absorb(&mut self, result: &mut PackageResult) -> Value {
        let mut bytes: usize = self.entries.values().map(String::len).sum();
        let before = json!({"entries":self.entries.len(),"bytes":bytes});
        let offered = json!({"entries":result.entries.len(),"bytes":result.entries.values().map(String::len).sum::<usize>()});
        let (mut accepted, mut invalid_key, mut entry_limit, mut byte_limit, mut count_limit) = (0, 0, 0, 0, 0);
        for (key, data) in std::mem::take(&mut result.entries) {
            if store::hash(&key).is_err() { invalid_key += 1; continue; }
            if data.len() > PACKAGE_ENTRY_BYTES { entry_limit += 1; continue; }
            let previous = self.entries.get(&key).map_or(0, String::len);
            let total = bytes - previous + data.len();
            if total > PACKAGE_BYTES { byte_limit += 1; continue; }
            if previous == 0 && self.entries.len() >= PACKAGE_COUNT { count_limit += 1; continue; }
            if self.entries.get(&key) != Some(&data) {
                self.persisted.remove(&key);
            }
            self.entries.insert(key, data);
            bytes = total;
            accepted += 1;
        }
        let used_unavailable = result.used.iter().filter(|key| !self.entries.contains_key(*key)).count();
        result.used.retain(|key| self.entries.contains_key(key));
        result.used.sort();
        result.used.dedup();
        json!({"before":before,"offered":offered,"accepted_entries":accepted,
            "refused":{"invalid_key":invalid_key,"entry_limit":entry_limit,"byte_limit":byte_limit,"count_limit":count_limit},
            "used_unavailable":used_unavailable,"after":{"entries":self.entries.len(),"bytes":bytes}})
    }
}

pub(crate) fn load_packages(
    store: &Store,
    workspace: &str,
    token: &str,
    anchor: &str,
    trace_key: &str,
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
        let (pool, stats) = load_package_rows(db, anchor, &denied, crate::phase::enabled())?;
        if crate::phase::enabled() {
            crate::phase::note("review_package_cache_load", &format!("anchor={anchor} key={trace_key} stats={stats}"));
        }
        Ok(pool)
    })
}

// Runs within the authorized read transaction, using the caller's denied-input
// set. Keeping counters beside each refusal avoids inferring causes from totals.
fn load_package_rows(db: &Connection, anchor: &str, denied: &BTreeSet<String>, trace: bool) -> Result<(PackagePool, Value)> {
    let mut pool = PackagePool::default();
    let mut bytes = 0;
    let (mut considered, mut unavailable, mut invalid_key, mut byte_limit, mut checksum) = (0, 0, 0, 0, 0);
    let mut query = db.prepare("SELECT id,data,sha256 FROM repo_review_package_facts WHERE repository_id=? AND length(data)<=? ORDER BY used_at DESC,rowid DESC LIMIT ?")?;
    let mut rows = query.query(params![anchor,PACKAGE_ENTRY_BYTES,PACKAGE_COUNT])?;
    while let Some(row) = rows.next()? {
        considered += 1;
        let key: String = row.get(0)?;
        if denied.contains(&key) { unavailable += 1; continue; }
        if store::hash(&key).is_err() { invalid_key += 1; continue; }
        let data: String = row.get(1)?;
        // Leave room for newly checked packages in the bounded native pool.
        if bytes + data.len() > PACKAGE_BYTES / 2 { byte_limit += 1; continue; }
        let sha: String = row.get(2)?;
        if graphicle_memory::text::sha256(data.as_bytes()) != sha { checksum += 1; continue; }
        bytes += data.len();
        pool.persisted.insert(key.clone(), sha);
        pool.entries.insert(key, data);
    }
    let mut stats = json!({"considered_entries":considered,"loaded_entries":pool.entries.len(),"loaded_bytes":bytes,
        "load_byte_limit":PACKAGE_BYTES/2,"entry_byte_limit":PACKAGE_ENTRY_BYTES,"entry_count_limit":PACKAGE_COUNT,
        "skipped":{"input_unavailable":unavailable,"invalid_key":invalid_key,"byte_limit":byte_limit,"checksum_mismatch":checksum}});
    if trace {
        let (count, stored_bytes, eligible): (usize, usize, usize) = db.query_row(
            "SELECT count(*),coalesce(sum(length(data)),0),coalesce(sum(length(data)<=?),0) FROM repo_review_package_facts WHERE repository_id=?",
            params![PACKAGE_ENTRY_BYTES,anchor], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        stats["stored_entries"] = json!(count);
        stats["stored_bytes"] = json!(stored_bytes);
        stats["skipped"]["entry_limit"] = json!(count - eligible);
        stats["skipped"]["count_limit"] = json!(eligible.saturating_sub(PACKAGE_COUNT));
    }
    Ok((pool, stats))
}

/// Save only after the final authorization, retention and binding checks.
pub(crate) fn save_packages(
    db: &Connection,
    anchor: &str,
    pool: &mut PackagePool,
    used: &[String],
    pins: &[Pin],
) -> Result<Value> {
    if used.is_empty() { return Ok(json!({"status":"unused"})); }
    let (mut written, mut refreshed_count, mut unavailable, mut invalid_key, mut entry_limit) = (0, 0, 0, 0, 0);
    for key in used.iter().take(PACKAGE_COUNT) {
        graphicle::process::check_deadline()?;
        let Some(data) = pool.entries.get(key) else {
            unavailable += 1;
            continue;
        };
        if store::hash(key).is_err() { invalid_key += 1; continue; }
        if data.len() > PACKAGE_ENTRY_BYTES { entry_limit += 1; continue; }
        let refreshed = if let Some(sha) = pool.persisted.get(key) {
            db.execute("UPDATE repo_review_package_facts SET used_at=? WHERE repository_id=? AND id=? AND sha256=?",params![crate::now(),anchor,key,sha])? > 0
        } else { false };
        if !refreshed {
            let sha = graphicle_memory::text::sha256(data.as_bytes());
            db.execute("INSERT INTO repo_review_package_facts(repository_id,id,data,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET data=excluded.data,sha256=excluded.sha256,used_at=excluded.used_at", params![anchor,key,data,sha,crate::now()])?;
            pool.persisted.insert(key.clone(), sha);
            written += 1;
        } else {
            refreshed_count += 1;
        }
        for pin in pins {
            db.execute("INSERT OR IGNORE INTO repo_review_package_inputs(repository_id,id,input_repository,snapshot) VALUES(?,?,?,?)",params![anchor,key,pin.repository_id,pin.snapshot])?;
        }
    }
    // Enforce both byte and entry limits, evicting least-recently-used facts.
    let evicted = db.execute("DELETE FROM repo_review_package_facts WHERE repository_id=? AND id NOT IN (SELECT id FROM (SELECT id,row_number() OVER (ORDER BY used_at DESC,rowid DESC) AS n,sum(length(data)) OVER (ORDER BY used_at DESC,rowid DESC) AS bytes FROM repo_review_package_facts WHERE repository_id=?) WHERE n<=? AND bytes<=?)",params![anchor,anchor,PACKAGE_COUNT,PACKAGE_BYTES])?;
    let mut stats = json!({"status":"pending_commit","requested_entries":used.len(),"written_entries":written,"refreshed_entries":refreshed_count,
        "skipped":{"unavailable":unavailable,"invalid_key":invalid_key,"entry_limit":entry_limit,"count_limit":used.len().saturating_sub(PACKAGE_COUNT)},
        "evicted_entries":evicted,"store_byte_limit":PACKAGE_BYTES,"store_entry_limit":PACKAGE_COUNT});
    if crate::phase::enabled() {
        let (count, bytes): (usize, usize) = db.query_row("SELECT count(*),coalesce(sum(length(data)),0) FROM repo_review_package_facts WHERE repository_id=?", [anchor], |r| Ok((r.get(0)?,r.get(1)?)))?;
        stats["retained_entries"] = json!(count);
        stats["retained_bytes"] = json!(bytes);
    }
    Ok(stats)
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
        let Some(row) = store::one(db, "SELECT facts,sha256 FROM repo_review_analysis WHERE repository_id=? AND id=?", params![anchor,key])? else {
            cache_note("review_cache_lookup",anchor,key,"not_found");
            return Ok(None)
        };
        for input in store::rows(db,"SELECT input_repository FROM repo_review_analysis_inputs WHERE repository_id=? AND id=?",params![anchor,key])? {
            if store::repo(db,principal,s(&input,"input_repository")).is_err() {
                cache_note("review_cache_lookup",anchor,key,"input_unavailable");
                return Ok(None)
            }
        }
        let facts:Value=serde_json::from_str(s(&row,"facts")).inspect_err(|_| cache_note("review_cache_lookup",anchor,key,"invalid_encoding"))?;
        let sha = s(&row,"sha256");
        let valid = crate::digest(&facts)==sha;
        cache_note("review_cache_lookup",anchor,key,if valid {"hit"} else {"checksum_mismatch"});
        Ok(valid.then(|| (facts,sha.to_owned())))
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
        if encoded.len() > 64 * 1024 * 1024 {
            cache_note("review_cache_write",anchor,key,"oversized");
            return Ok(());
        }
        db.execute("INSERT INTO repo_review_analysis(repository_id,id,facts,sha256,used_at) VALUES(?,?,?,?,?) ON CONFLICT(repository_id,id) DO UPDATE SET facts=excluded.facts,sha256=excluded.sha256,used_at=excluded.used_at",params![anchor,key,encoded,crate::digest(facts),crate::now()])?;
    }
    for pin in pins {
        db.execute("INSERT OR IGNORE INTO repo_review_analysis_inputs(repository_id,id,input_repository,snapshot) VALUES(?,?,?,?)",params![anchor,key,pin.repository_id,pin.snapshot])?;
    }
    let evicted = db.execute("DELETE FROM repo_review_analysis WHERE repository_id=? AND id NOT IN (SELECT id FROM repo_review_analysis WHERE repository_id=? ORDER BY used_at DESC,rowid DESC LIMIT 4)",params![anchor,anchor])?;
    cache_note("review_cache_write",anchor,key,&format!("{} evicted={evicted}", if refreshed {"refresh_pending_commit"} else {"write_pending_commit"}));
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
                    if kind == "packages_32mib" { save_packages(db,anchor,&mut pool,&used,&[]).map(|_| ()) }
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
        pool = load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor, "").unwrap();
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
            assert_eq!(load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor, "").unwrap().entries[&key], "original");
        }
        let mut repaired = PackageResult::default();
        repaired.entries.insert(key.clone(), "repaired by native compiler".into());
        pool.absorb(&mut repaired);
        store.transaction(|db| save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])).unwrap();
        assert_eq!(load_packages(&store,s(&ws,"id"),s(&ws,"capability"),anchor, "").unwrap().entries[&key], "repaired by native compiler");
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
        let stats = pool.absorb(&mut result);
        assert_eq!(pool.entries.len(), PACKAGE_COUNT);
        assert_eq!(result.used.len(), PACKAGE_COUNT);
        assert_eq!(stats["refused"]["invalid_key"], 1);
        assert_eq!(stats["refused"]["count_limit"], 1);
        assert_eq!(stats["accepted_entries"], PACKAGE_COUNT);
        assert_eq!(stats["used_unavailable"], 2);
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
        let stats = pool.absorb(&mut result);
        assert_eq!(
            pool.entries.values().map(String::len).sum::<usize>(),
            PACKAGE_BYTES
        );
        assert_eq!(pool.entries.len(), 16);
        assert_eq!(stats["refused"]["byte_limit"], 1);
        assert_eq!(stats["refused"]["entry_limit"], 1);
        assert_eq!(stats["after"]["bytes"], PACKAGE_BYTES);
    }

    #[test]
    fn package_load_diagnostics_distinguish_capacity_access_and_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let ws = store.create_workspace("package-load-diagnostics").unwrap();
        let repo = store.register(s(&ws,"id"),s(&ws,"capability"),&json!({"project_key":"load","name":"load"})).unwrap();
        let anchor = s(&repo,"id");
        let denied = BTreeSet::from(["d".repeat(64)]);
        store.transaction(|db| {
            let insert = |key: &str, data: &str, sha: &str, time: &str| db.execute(
                "INSERT INTO repo_review_package_facts VALUES(?,?,?,?,?)", params![anchor,key,data,sha,time]);
            let payload = "x".repeat(PACKAGE_ENTRY_BYTES);
            let sha = graphicle_memory::text::sha256(payload.as_bytes());
            for n in 0..9 { insert(&format!("{n:064x}"),&payload,&sha,"a")?; }
            insert("invalid-key","x","irrelevant","z")?;
            insert(&"c".repeat(64),"x","corrupt","z")?;
            insert(&"d".repeat(64),"x","irrelevant","z")?;
            insert(&"e".repeat(64),&"x".repeat(PACKAGE_ENTRY_BYTES+1),"irrelevant","z")?;
            let (pool, stats) = load_package_rows(db,anchor,&denied,true)?;
            assert_eq!(pool.entries.len(),8);
            assert_eq!(stats["stored_entries"],13);
            assert_eq!(stats["loaded_bytes"],PACKAGE_BYTES/2);
            for reason in ["input_unavailable","invalid_key","checksum_mismatch","byte_limit","entry_limit"] {
                assert_eq!(stats["skipped"][reason],1,"{stats}");
            }
            assert_eq!(stats["skipped"]["count_limit"],0);
            db.execute("DELETE FROM repo_review_package_facts WHERE repository_id=?",[anchor])?;
            let sha = graphicle_memory::text::sha256(b"x");
            for n in 0..PACKAGE_COUNT+1 { insert(&format!("{n:064x}"),"x",&sha,"a")?; }
            let (pool, stats) = load_package_rows(db,anchor,&BTreeSet::new(),true)?;
            assert_eq!(pool.entries.len(),PACKAGE_COUNT);
            assert_eq!(stats["stored_entries"],PACKAGE_COUNT+1);
            assert_eq!(stats["skipped"]["count_limit"],1);
            Ok(())
        }).unwrap();
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
            load_packages(&store, workspace, s(&reader, "capability"), anchor, "")
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
            load_packages(&store, workspace, s(&reader, "capability"), anchor, "")
                .unwrap()
                .entries
                .is_empty()
        );
        assert_eq!(
            load_packages(&store, workspace, owner, anchor, "")
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
            let stats = save_packages(db,anchor,&mut pool,std::slice::from_ref(&key),&[])?;
            assert_eq!(stats["evicted_entries"],3);
            assert_eq!(stats["status"],"pending_commit");
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
