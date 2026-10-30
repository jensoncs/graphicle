//! Store-owned immutable compiler facts. All cached spans are verified again;
//! cache lookup never replaces source authorization or pin validation.
use crate::{
    Result,
    review::Pin,
    store::{self, Store, s},
};
use rusqlite::{Connection, params};
use serde_json::Value;

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
