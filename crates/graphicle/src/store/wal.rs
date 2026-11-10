//! SQLite WAL maintenance, backported from upstream 0.1.14 without the
//! PostgreSQL connection layer. One gate per canonical store path prevents
//! checkpoints in this process from starving other writers.
use crate::Result;
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, TryLockError, Weak},
    time::{Duration, Instant},
};

pub(super) const WAL_LIMIT: u64 = 64 << 20;
pub(crate) const CHECKPOINT_WAIT: Duration = Duration::from_secs(2);
const CHECKPOINT_PAUSE: Duration = Duration::from_secs(8);

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct CheckpointGate {
    /// Held throughout a checkpoint, with a cooldown after a busy result.
    not_before: Mutex<Option<Instant>>,
}

pub(super) fn checkpoint_gate(path: &Path) -> Arc<CheckpointGate> {
    static GATES: Mutex<BTreeMap<PathBuf, Weak<CheckpointGate>>> = Mutex::new(BTreeMap::new());
    let key = path
        .canonicalize()
        .ok()
        .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))
        .unwrap_or_else(|| path.to_path_buf());
    let mut gates = GATES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(gate) = gates.get(&key).and_then(Weak::upgrade) {
        return gate;
    }
    gates.retain(|_, gate| gate.strong_count() > 0);
    let gate = Arc::new(CheckpointGate::default());
    gates.insert(key, Arc::downgrade(&gate));
    gate
}

impl CheckpointGate {
    /// `None` if no checkpoint was needed or another checkpoint/cooldown
    /// owns the gate; otherwise whether truncation finished. A long read or
    /// one large transaction can still grow the WAL beyond `limit`.
    pub(super) fn bound(&self, db: &Connection, path: &Path, limit: u64) -> Result<Option<bool>> {
        if !db.is_autocommit() || limit == 0 {
            return Ok(None);
        }
        let mut log = path.as_os_str().to_os_string();
        log.push("-wal");
        if std::fs::metadata(log).map(|m| m.len()).unwrap_or(0) <= limit {
            return Ok(None);
        }
        let mut gate = match self.not_before.try_lock() {
            Ok(gate) => gate,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
        };
        if gate.is_some_and(|until| Instant::now() < until) {
            return Ok(None);
        }
        // Respect this branch's request deadlines, and restore the caller's
        // timeout even when the checkpoint fails.
        let wait = graphicle::process::bounded_timeout(CHECKPOINT_WAIT)?;
        let previous: u64 = db.query_row("PRAGMA busy_timeout", [], |r| r.get(0))?;
        let outcome = db.busy_timeout(wait).and_then(|()| {
            db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                r.get::<_, i64>(0)
            })
        });
        let restored = db.busy_timeout(Duration::from_millis(previous));
        if !matches!(outcome, Ok(0)) {
            *gate = Some(Instant::now() + CHECKPOINT_PAUSE);
        }
        restored?;
        Ok(Some(outcome? == 0))
    }
}
