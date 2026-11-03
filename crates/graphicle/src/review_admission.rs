//! Exact omission attestations and conservative build-input accounting.
use super::{Analysis, Input, Pin};
use crate::{Error, Result, local, store};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const MAX_VENDOR_BYTES: usize = 256 * 1024 * 1024;

pub(crate) fn source_limits() -> Value {
    json!({"scope":"owned-and-go-vendor-v1",
        "owned_bytes_per_side":super::MAX_SOURCE_BYTES,
        "vendor_bytes_per_side":MAX_VENDOR_BYTES,
        "files_per_side":super::MAX_FILES,
        "bytes_per_file":super::MAX_SOURCE_BYTES})
}

// Directory spelling alone is not dependency provenance. Only an admitted Go
// module with its sibling vendor manifest receives the separate byte allowance.
// This is resource accounting, never a source-policy or integrity exemption.
pub(super) fn vendored(path: &str, files: &serde_json::Map<String, Value>) -> bool {
    let mut prefix = String::new();
    for component in path.split('/') {
        if component == "vendor"
            && files.contains_key(&format!("{prefix}go.mod"))
            && files.contains_key(&format!("{prefix}vendor/modules.txt"))
        {
            return true;
        }
        prefix.push_str(component);
        prefix.push('/');
    }
    false
}

#[derive(Default)]
pub(super) struct SourceBudget {
    owned_bytes: usize,
    vendor_bytes: usize,
    files: usize,
}
impl SourceBudget {
    // Count every admitted manifest entry before loading it, including missing
    // objects; missing bytes cannot create additional allowance for other pins.
    pub(super) fn reserve(&mut self, bytes: u64, vendor: bool) -> Result<()> {
        if bytes > super::MAX_SOURCE_BYTES as u64 {
            return Err(Error::bad("Review source bound exceeded (64 MiB per file)"));
        }
        if self.files >= super::MAX_FILES {
            return Err(Error::bad(
                "Review source bound exceeded (20000 files per side, source and vendor combined)",
            ));
        }
        let (used, limit, message) = if vendor {
            (
                &mut self.vendor_bytes,
                MAX_VENDOR_BYTES,
                "Review source bound exceeded (256 MiB of Go vendor inputs per side)",
            )
        } else {
            (
                &mut self.owned_bytes,
                super::MAX_SOURCE_BYTES,
                "Review source bound exceeded (64 MiB of repository source per side)",
            )
        };
        let next = used
            .checked_add(bytes as usize)
            .filter(|n| *n <= limit)
            .ok_or_else(|| Error::bad(message))?;
        *used = next;
        self.files += 1;
        Ok(())
    }
}

pub(super) struct Binding {
    pub pin: Pin,
    pub complete: bool,
    pub withheld: Option<Vec<String>>,
    // Preserve presence as well as values for the final concurrency check.
    pub original: Option<Value>,
}

pub(crate) fn migrate(db: &rusqlite::Connection) -> Result<()> {
    store::ensure_column(db, "repo_snapshot_bindings", "withheld_paths", "TEXT")?;
    // Old writers only replace scope_complete. Their new attestation must not
    // inherit an exhaustive list from a newer writer. New writers set the list
    // after replacing scope_complete, in the same sync transaction.
    db.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS repo_scope_resets_withheld
        AFTER UPDATE OF scope_complete ON repo_snapshot_bindings
        BEGIN
            UPDATE repo_snapshot_bindings SET withheld_paths=NULL
            WHERE repository_id=NEW.repository_id AND snapshot=NEW.snapshot
                AND commit_id=NEW.commit_id AND policy_sha256=NEW.policy_sha256;
        END;",
    )?;
    Ok(())
}

/// Presence attests an exhaustive list, including an explicitly empty list.
/// The enclosing metadata has the sync protocol's 64 KiB bound.
pub(crate) fn withheld_paths(metadata: &Value) -> Result<Option<Vec<String>>> {
    let Some(value) = metadata.get("withheld_paths").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let paths = value
        .as_array()
        .filter(|v| v.len() <= super::MAX_FILES)
        .ok_or_else(|| Error::bad("withheld_paths must be a bounded array of exact paths"))?;
    let mut result = BTreeSet::new();
    for path in paths {
        let path = path
            .as_str()
            .ok_or_else(|| Error::bad("Invalid withheld path"))?;
        store::path(path)?;
        if !result.insert(path.to_owned()) {
            return Err(Error::bad("Duplicate withheld path"));
        }
    }
    if metadata["scope_complete"] == true && !result.is_empty() {
        return Err(Error::bad(
            "scope_complete conflicts with nonempty withheld_paths",
        ));
    }
    Ok(Some(result.into_iter().collect()))
}

fn non_build_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    // The generic source classifier includes JSON. Only these exact credential
    // names are exempt; a sensitive prefix must not hide e.g. .env.py source.
    if super::unsupported_source(path, "")
        && !matches!(
            name,
            "credentials.json" | "secrets.json" | "local-session.json"
        )
    {
        return false;
    }
    let extension = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    // A sensitive-looking name can still be compiler input, e.g. .env.go.
    if [
        "go", "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx", "f", "for", "f90", "m", "s",
        "swig", "swigcxx", "syso",
    ]
    .contains(&extension.to_ascii_lowercase().as_str())
    {
        return false;
    }
    local::sensitive(name)
        || matches!(extension, "md" | "rst")
        || matches!(name, "LICENSE" | "NOTICE")
}

fn may_embed(package: &Value, path: &str) -> bool {
    let Some(directory) = package["directory"].as_str() else {
        return true;
    };
    let Some(relative) = path.strip_prefix(&format!("{directory}/")) else {
        return false;
    };
    let Some(patterns) = package["embed_patterns"].as_array() else {
        return true;
    };
    patterns.iter().any(|pattern| {
        let Some(pattern) = pattern.as_str() else {
            return true;
        };
        // Go path.Match and glob differ on class negation and escaping. Never
        // use a non-match in those forms as proof an omitted file is irrelevant.
        if pattern.contains(['[', '\\']) {
            return true;
        }
        let Ok(pattern) = glob::Pattern::new(pattern.strip_prefix("all:").unwrap_or(pattern))
        else {
            return true;
        };
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..Default::default()
        };
        // Matching a directory embeds its descendants. Including hidden files
        // even without all: is conservative and avoids inventing Go build facts.
        let mut candidate = relative;
        loop {
            if pattern.matches_with(candidate, options) {
                return true;
            }
            match candidate.rsplit_once('/') {
                Some((parent, _)) => candidate = parent,
                None => return false,
            }
        }
    })
}

pub(super) fn assess(input: &Input, analysis: &Analysis, side: &str) -> (Vec<Value>, Vec<Value>) {
    let mut issues = analysis.issues.clone();
    let mut summaries = vec![];
    for binding in &input.bindings {
        let Some(paths) = &binding.withheld else {
            continue;
        };
        let directory = &binding.pin.directory;
        let prefix = format!("{directory}/");
        let packages: Vec<_> = analysis
            .packages
            .iter()
            .filter(|p| {
                let dir = store::s(p, "directory");
                dir == directory || dir.starts_with(&prefix)
            })
            .collect();
        let unresolved: Vec<_> = paths
            .iter()
            .filter(|path| {
                // Only Go has a compiler-owned build inventory. Missing source can
                // add edges absent from this graph, even in an apparently unrelated
                // package, so code and unknown inputs always remain conservative.
                packages.is_empty()
                    || !non_build_file(path)
                    || packages
                        .iter()
                        .any(|p| may_embed(p, &format!("{prefix}{path}")))
            })
            .collect();
        if !binding.complete && unresolved.is_empty() {
            issues.retain(|issue| {
                !(issue["reason"] == "source_admission_incomplete" && issue["detail"] == *directory)
            });
        }
        summaries.push(json!({"directory":directory,"side":side,"exhaustive":true,
            "withheld_count":paths.len(),"unresolved_count":unresolved.len(),
            "outside_build_count":paths.len()-unresolved.len(),
            "paths_sha256":crate::digest(&json!(paths)),
            "scope":"Go build inputs; runtime file reads are not proven"}));
    }
    (issues, summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_vendor_budgets_are_separate_bounded_and_shared_by_pins() {
        const MIB: u64 = 1024 * 1024;
        let mut budget = SourceBudget::default();
        // The largest observed closure plus largest owned tree both fit.
        budget.reserve(21 * MIB, false).unwrap();
        budget.reserve(60 * MIB, true).unwrap();
        budget.reserve(38 * MIB, true).unwrap();
        budget.reserve(43 * MIB, false).unwrap();
        assert!(budget.reserve(1, false).is_err());
        for bytes in [60, 60, 38] {
            budget.reserve(bytes * MIB, true).unwrap();
        }
        assert!(budget.reserve(1, true).is_err());
        assert!(SourceBudget::default().reserve(u64::MAX, true).is_err());
        assert!(SourceBudget::default().reserve(65 * MIB, true).is_err());
        let mut count = SourceBudget::default();
        for n in 0..super::super::MAX_FILES {
            count.reserve(0, n % 2 == 0).unwrap();
        }
        assert!(count.reserve(0, true).is_err());
        assert!(count.reserve(0, false).is_err());
    }

    #[test]
    fn vendor_allowance_requires_matching_admitted_module_and_manifest() {
        let manifest = json!({"go.mod":{}, "vendor/modules.txt":{},
            "nested/go.mod":{}, "nested/vendor/modules.txt":{}});
        let files = manifest.as_object().unwrap();
        for path in [
            "vendor/modules.txt",
            "vendor/example/dep/code.go",
            "nested/vendor/example/dep/data.bin",
        ] {
            assert!(vendored(path, files), "{path}");
        }
        for path in [
            "own.go",
            "vendor.go",
            "myvendor/code.go",
            "examples/vendor/code.go",
            "nested/vendorish/code.go",
        ] {
            assert!(!vendored(path, files), "{path}");
        }
        assert!(!vendored(
            "vendor/example/dep/code.go",
            json!({"go.mod":{}}).as_object().unwrap()
        ));
        assert!(!vendored(
            "vendor/example/dep/code.go",
            json!({"vendor/modules.txt":{}}).as_object().unwrap()
        ));
    }

    #[test]
    fn embed_patterns_include_directory_descendants_and_hidden_files() {
        for pattern in ["assets", "assets/*", "all:assets/?.key", "assets/[ab].key"] {
            assert!(
                may_embed(
                    &json!({"directory":"app","embed_patterns":[pattern]}),
                    "app/assets/a.key"
                ),
                "{pattern}"
            );
        }
        assert!(may_embed(
            &json!({"directory":"app","embed_patterns":["*"]}),
            "app/.env"
        ));
        assert!(!may_embed(
            &json!({"directory":"app","embed_patterns":["public/*"]}),
            "app/assets/a.key"
        ));
        assert!(may_embed(
            &json!({"directory":"app","embed_patterns":["["]}),
            "app/assets/a.key"
        ));
        assert!(may_embed(
            &json!({"directory":"app","embed_patterns":["assets/[^a].key"]}),
            "app/assets/b.key"
        ));
        assert!(may_embed(
            &json!({"directory":"app","embed_patterns":["assets/[!a].key"]}),
            "app/assets/a.key"
        ));
        assert!(!non_build_file("private/.env.go"));
        assert!(!non_build_file("private/.env.S"));
        assert!(!non_build_file("private/.env.py"));
        assert!(!non_build_file("requirements.txt"));
        assert!(!non_build_file("vendor/modules.txt"));
        assert!(!non_build_file("go.mod"));
        assert!(!non_build_file("secrets/consumer.go"));
        assert!(non_build_file("private/credentials.json"));
    }
}
