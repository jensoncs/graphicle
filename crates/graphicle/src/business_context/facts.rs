//! Repository facts a caller learned from merged pull requests: the rule, flow
//! or intent a change's title and description state, with the files and
//! symbols it touched. Each is an author claim nobody verified. A review entry
//! carries the facts that name it, most relevant first: a fact on its symbol
//! before one on its file before one on its directory, then the most
//! corroborated, then the newest. A fact a later one superseded is left out; a
//! fact whose file changed since its merge is marked possibly stale. Facts only
//! add questions and priority; nothing here lowers a finding.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const MAX_FACTS: usize = 512;
/// Facts attached to one entry; the rest are counted.
pub const PER_ENTRY: usize = 5;
const MAX_STATEMENT: usize = 500;
const MAX_NAMES: usize = 16;
const KINDS: &[&str] = &[
    "product_rule",
    "intent",
    "flow",
    "constraint",
    "deployment",
    "other",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub id: String,
    pub statement: String,
    pub kind: String,
    pub pr: u64,
    pub merged_commit: String,
    pub merged_at: i64,
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Each named file's digest at the merge commit.
    #[serde(default)]
    pub file_sha256: BTreeMap<String, String>,
    /// Later merged pull requests that stated the same fact.
    #[serde(default)]
    pub corroborations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

pub fn validate(facts: &[Fact], path: impl Fn(&str) -> Result<()>) -> Result<()> {
    if facts.len() > MAX_FACTS {
        return Err(Error::bad("Business facts exceed 512"));
    }
    let id = |s: &str| {
        !s.is_empty()
            && s.len() <= 64
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    for fact in facts {
        let commit = fact.merged_commit.len() == 40
            && fact
                .merged_commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        if !id(&fact.id)
            || fact.superseded_by.as_deref().is_some_and(|s| !id(s))
            || fact.statement.trim().is_empty()
            || fact.statement.chars().count() > MAX_STATEMENT
            || fact.statement.chars().any(char::is_control)
            || !KINDS.contains(&fact.kind.as_str())
            || !commit
            || !(0..=4_102_444_800).contains(&fact.merged_at)
            || fact.files.is_empty()
            || fact.files.len() > MAX_NAMES
            || fact.symbols.len() > MAX_NAMES
            || fact
                .symbols
                .iter()
                .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            || fact.file_sha256.keys().any(|k| !fact.files.contains(k))
        {
            return Err(Error::bad("Invalid business fact"));
        }
        for file in &fact.files {
            path(file)?;
        }
        for sha in fact.file_sha256.values() {
            crate::store::hash(sha)?;
        }
    }
    Ok(())
}

fn terminal(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// How a fact names an entry: 3 its symbol, 2 its file, 1 its directory.
fn relevance(fact: &Fact, file: &str, name: &str) -> u8 {
    let on_file = fact.files.iter().any(|f| f == file);
    if on_file && !name.is_empty() && fact.symbols.iter().any(|s| terminal(s) == terminal(name)) {
        3
    } else if on_file {
        2
    } else if fact.files.iter().any(|f| directory(f) == directory(file)) {
        1
    } else {
        0
    }
}

/// The facts for one entry, most relevant first, and how many more matched.
pub fn for_entry(facts: &[Fact], entry: &Value) -> (Vec<Value>, usize) {
    let file = entry["file"].as_str().unwrap_or("");
    let name = entry["name"].as_str().unwrap_or("");
    let current = entry["file_sha256"].as_str();
    let mut matched: Vec<(u8, &Fact)> = facts
        .iter()
        .filter(|f| f.superseded_by.is_none())
        .map(|f| (relevance(f, file, name), f))
        .filter(|(r, _)| *r > 0)
        .collect();
    matched.sort_by(|(ra, a), (rb, b)| {
        rb.cmp(ra)
            .then(b.corroborations.min(5).cmp(&a.corroborations.min(5)))
            .then(b.merged_at.cmp(&a.merged_at))
            .then(a.id.cmp(&b.id))
    });
    let omitted = matched.len().saturating_sub(PER_ENTRY);
    let out = matched
        .into_iter()
        .take(PER_ENTRY)
        .map(|(r, f)| {
            let stale = f.file_sha256.get(file).is_some_and(|sha| Some(sha.as_str()) != current);
            let how = ["", "directory", "file", "symbol"][r as usize];
            json!({"id": f.id, "statement": f.statement, "kind": f.kind, "pr": f.pr, "merged_commit": f.merged_commit,
                "merged_at": f.merged_at, "match": how,
                "corroborations": f.corroborations, "possibly_stale": stale, "basis": "author_claim_unverified"})
        })
        .collect();
    (out, omitted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(id: &str, files: &[&str], symbols: &[&str], merged_at: i64) -> Fact {
        Fact {
            id: id.into(),
            statement: format!("{id} holds"),
            kind: "product_rule".into(),
            pr: 7,
            merged_commit: "a".repeat(40),
            merged_at,
            files: files.iter().map(|s| s.to_string()).collect(),
            symbols: symbols.iter().map(|s| s.to_string()).collect(),
            file_sha256: BTreeMap::new(),
            corroborations: 0,
            superseded_by: None,
        }
    }

    #[test]
    fn symbol_before_file_before_directory_then_corroborated_then_newest() {
        let mut corroborated = fact("corroborated", &["svc/hold.go"], &[], 1);
        corroborated.corroborations = 3;
        let mut superseded = fact("superseded", &["svc/hold.go"], &["Hold"], 9);
        superseded.superseded_by = Some("symbol".into());
        let mut stale = fact("stale", &["svc/hold.go"], &[], 2);
        stale
            .file_sha256
            .insert("svc/hold.go".into(), "f".repeat(64));
        let facts = [
            fact("directory", &["svc/other.go"], &[], 9),
            fact("file_new", &["svc/hold.go"], &[], 5),
            corroborated,
            fact("symbol", &["svc/hold.go"], &["alerts.Hold"], 1),
            superseded,
            stale,
            fact("elsewhere", &["web/x.ts"], &[], 9),
        ];
        let entry = json!({"file":"svc/hold.go","name":"policy.Hold","file_sha256":"e".repeat(64)});
        let (got, omitted) = for_entry(&facts, &entry);
        let ids: Vec<_> = got.iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            ["symbol", "corroborated", "file_new", "stale", "directory"]
        );
        assert_eq!(omitted, 0);
        assert_eq!(got[0]["match"], "symbol");
        assert_eq!(got[3]["possibly_stale"], true);
        assert_eq!(got[2]["possibly_stale"], false);
        assert!(got.iter().all(|f| f["basis"] == "author_claim_unverified"));
    }

    #[test]
    fn invalid_facts_are_refused() {
        let ok = |f: &Fact| validate(std::slice::from_ref(f), |_| Ok(())).is_ok();
        let good = fact("good", &["a.go"], &[], 1);
        assert!(ok(&good));
        for bad in [
            Fact {
                kind: "verdict".into(),
                ..good.clone()
            },
            Fact {
                statement: "x\u{7}".into(),
                ..good.clone()
            },
            Fact {
                statement: "x".repeat(501),
                ..good.clone()
            },
            Fact {
                files: vec![],
                ..good.clone()
            },
            Fact {
                merged_commit: "A".repeat(40),
                ..good.clone()
            },
            Fact {
                id: "has space".into(),
                ..good.clone()
            },
            Fact {
                file_sha256: BTreeMap::from([("b.go".into(), "f".repeat(64))]),
                ..good.clone()
            },
        ] {
            assert!(!ok(&bad), "{bad:?}");
        }
        assert!(validate(&[good], |_| Err(Error::bad("excluded"))).is_err());
    }
}
