//! Select compiler working directories from admitted module files, not suffixes
//! found anywhere below an otherwise non-Go repository root.
use super::{Input, Profile, basic, issue};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) struct Scope {
    pub roots: Vec<Value>,
    pub issues: Vec<Value>,
}

/// Ephemeral compiler inputs are private, create-only files. They need visibility
/// to the helper, not crash durability: the authoritative bytes remain in SQLite.
pub(super) fn materialize(input: &Input) -> Result<tempfile::TempDir> {
    let tree = {
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700)).tempdir()?
        }
        #[cfg(not(unix))] { tempfile::tempdir()? }
    };
    materialize_sources(input, tree.path())?;
    Ok(tree)
}

// Called only on the new scratch tree, before starting the compiler. Input
// paths and pin directories were validated at admission/load. Exclusive creates
// preserve collision protection; any failure drops the whole tree in the caller.
fn materialize_sources(input: &Input, root: &Path) -> Result<()> {
    use std::io::Write;
    let mut directories = BTreeSet::new();
    for pin in &input.pins {
        let directory = Path::new(&pin.directory);
        std::fs::create_dir(root.join(directory))?;
        directories.insert(directory);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for (path, source) in &input.sources {
        let target = root.join(path);
        let parent = Path::new(path).parent().unwrap();
        if directories.insert(parent) {
            std::fs::create_dir_all(root.join(parent))?;
        }
        let mut file = options.open(&target)?;
        file.write_all(source.content.bytes())?;
    }
    Ok(())
}

/// Preserve existing cache identities when the collector has no exclusions.
pub(super) fn with_exclusions(mut identity: Value, input: &Input) -> Value {
    let exclusions: Vec<_> = input.pins.iter().filter(|p| !p.excluded_modules.is_empty())
        .map(|p| json!({"repository_id":p.repository_id,"directory":p.directory,"modules":p.excluded_modules}))
        .collect();
    if !exclusions.is_empty() {
        identity["excluded_modules"] = json!(exclusions);
    }
    identity
}

/// Reuse only native Go facts, never the report or its snapshot bindings.
/// Keep every path: additions/deletions can affect discovery and embed matches.
/// Only known documentation/configuration content is omitted, and never below
/// a package with an embed directive. This deliberately over-invalidates instead
/// of duplicating Go's directive parser, glob rules, or build-tag selection.
pub(super) fn fact_key(input: &Input, profile: &Profile, identity: &str) -> String {
    let embed_directories: BTreeSet<_> = input
        .sources
        .iter()
        .filter_map(|(path, source)| {
            if path.ends_with(".go")
                && source
                    .content
                    .text()
                    .is_none_or(|text| text.contains("//go:embed"))
            {
                path.rsplit_once('/').map(|(directory, _)| directory)
            } else {
                None
            }
        })
        .collect();
    let sources: Vec<_> = input
        .sources
        .iter()
        .map(|(path, source)| {
            let file = Path::new(path);
            let content_only = matches!(
                file.extension().and_then(|e| e.to_str()),
                Some("md" | "rst" | "txt" | "json" | "yaml" | "yml")
            ) && file.file_name().and_then(|n| n.to_str())
                != Some("modules.txt")
                && path != &profile.workspace
                && !embed_directories
                    .iter()
                    .any(|directory| within(path, directory));
            json!([
                path,
                source.repository,
                (!content_only).then_some(&source.sha)
            ])
        })
        .collect();
    crate::digest(&with_exclusions(
        json!({
            "identity":identity,"profile":profile,"fact_policy":"pinned-go-inputs-v4",
            "roots":input.pins.iter().map(|p|json!([p.repository_id,p.directory,p.modules,p.policy_sha256])).collect::<Vec<_>>(),
            "sources":sources
        }),
        input,
    ))
}

fn within(path: &str, directory: &str) -> bool {
    path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|p| p.starts_with('/'))
}

// go list ./... does not traverse these directory names. An explicitly selected
// root is allowed; only descendants are filtered here.
fn ignored(relative: &str) -> bool {
    relative.split('/').any(|part| {
        part == "vendor" || part == "testdata" || part.starts_with('.') || part.starts_with('_')
    })
}

pub(super) fn scope(input: &Input) -> Result<Scope> {
    let mut out = Scope {
        roots: vec![],
        issues: vec![],
    };
    for pin in &input.pins {
        let modules: BTreeSet<_> = input
            .sources
            .keys()
            .filter_map(|path| {
                let directory = path.strip_suffix("/go.mod")?;
                (within(directory, &pin.directory) && basic::language(input, path) != Some("helm"))
                    .then_some(directory)
            })
            .collect();
        let mut selected = BTreeSet::new();
        // Only actual admitted module roots can be excluded: excluding an
        // arbitrary package directory cannot change go list ./... semantics.
        let excluded: BTreeSet<_> = pin
            .excluded_modules
            .iter()
            .filter_map(|relative| {
                let root = if relative == "." {
                    pin.directory.clone()
                } else {
                    format!("{}/{relative}", pin.directory)
                };
                modules.contains(root.as_str()).then_some(root)
            })
            .collect();
        let is_excluded = |path: &str| excluded.iter().any(|root| within(path, root));
        for requested in &pin.modules {
            let directory = if requested == "." {
                pin.directory.clone()
            } else {
                format!("{}/{requested}", pin.directory)
            };
            if !is_excluded(&directory) && modules.iter().any(|module| within(&directory, module)) {
                selected.insert(directory.clone());
            }
            // A repository can have nested modules but no top-level module.
            // Each module needs its own invocation: ./... skips nested modules.
            for module in &modules {
                if let Some(relative) = module.strip_prefix(&format!("{directory}/"))
                    && !ignored(relative)
                    && !is_excluded(module)
                {
                    selected.insert((*module).to_string());
                }
            }
        }
        if !excluded.is_empty() {
            out.issues.push(issue("go_module_excluded", json!({"directory":pin.directory,
                "modules":excluded.iter().map(|root|root.strip_prefix(&format!("{}/",pin.directory)).unwrap_or(".")).collect::<Vec<_>>(),
                "scope":"excluded from root discovery; admitted dependency sources remain available"})));
        }
        if selected.len() > 64 {
            return Err(Error::bad(
                "Go module discovery exceeds 64 roots per repository; narrow modules",
            ));
        }
        for root in &selected {
            let module = root
                .strip_prefix(&format!("{}/", pin.directory))
                .unwrap_or(".");
            out.roots
                .push(json!({"directory":pin.directory,"module":module}));
        }
        let mut missing_module = vec![];
        let mut outside_scope = vec![];
        for path in input
            .sources
            .keys()
            .filter(|p| within(p, &pin.directory) && p.ends_with(".go"))
        {
            if basic::language(input, path) == Some("helm") {
                continue;
            }
            if is_excluded(path) {
                continue;
            }
            if selected.iter().any(|root| within(path, root)) {
                continue;
            }
            let directory = path.rsplit_once('/').map(|(p, _)| p).unwrap();
            let relative = directory
                .strip_prefix(&format!("{}/", pin.directory))
                .unwrap_or("");
            if ignored(relative) {
                continue;
            }
            if modules.iter().any(|module| within(path, module)) {
                outside_scope.push(path);
            } else {
                missing_module.push(path);
            }
        }
        for (reason, files) in [
            ("go_source_without_module", missing_module),
            ("go_source_outside_module_scope", outside_scope),
        ] {
            if !files.is_empty() {
                out.issues.push(issue(reason,json!({"directory":pin.directory,"files":files.len(),"examples":files.iter().take(8).collect::<Vec<_>>()})));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::{Pin, Source};
    use crate::source_object::Content;
    use std::collections::BTreeMap;

    fn input(files: &[(&str, &str)]) -> Input {
        Input {
            pins: vec![Pin {
                repository_id: "repo".into(),
                directory: "app".into(),
                snapshot: "snapshot".into(),
                commit: "commit".into(),
                policy_sha256: "policy".into(),
                modules: vec![".".into()],
                excluded_modules: vec![],
            }],
            sources: files
                .iter()
                .map(|(path, text)| {
                    (
                        format!("app/{path}"),
                        Source {
                            repository: "repo".into(),
                            path: (*path).into(),
                            sha: graphicle_memory::text::sha256(text.as_bytes()),
                            content: Content::Text((*text).into()),
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
            issues: vec![],
            bindings: vec![],
        }
    }

    #[test]
    #[ignore = "manual before/after compiler snapshot materialization benchmark"]
    fn snapshot_materialization_benchmark() {
        for size in [5_000usize, 20_000, 50_000] {
            let mut sources = input(&[]);
            let text = format!("//{}\n", "x".repeat(1021));
            let sha = graphicle_memory::text::sha256(text.as_bytes());
            for i in 0..size {
                let path = format!("pkg/{:04}/file_{i:06}.go", i / 100);
                sources.sources.insert(format!("app/{path}"), Source {
                    repository:"repo".into(),path,sha:sha.clone(),content:Content::Text(text.clone()),
                });
            }
            let start = std::time::Instant::now();
            let tree = materialize(&sources).unwrap();
            let materialize_ms = start.elapsed().as_secs_f64() * 1000.0;
            let root = tree.path().to_owned();
            // Verify every file outside the timed write/cleanup intervals.
            for (path,source) in &sources.sources {
                assert_eq!(std::fs::read(root.join(path)).unwrap(),source.content.bytes());
            }
            let start = std::time::Instant::now();
            drop(tree);
            let cleanup_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert!(!root.exists());
            eprintln!("MATERIALIZATION_BENCH={}", json!({"files":size,"bytes_per_file":text.len(),"leaf_directories":size/100,"materialize_ms":materialize_ms,"cleanup_ms":cleanup_ms}));
        }
    }

    #[test]
    fn compiler_scratch_preserves_exact_bytes_permissions_and_cleanup() {
        let mut sources = input(&[("main.go", "package app\n"), ("vendor/dep/asset", "")]);
        let asset = sources.sources.get_mut("app/vendor/dep/asset").unwrap();
        asset.content = Content::Binary(vec![0,255,128,10]);
        asset.sha = graphicle_memory::text::sha256(asset.content.bytes());
        let tree = materialize(&sources).unwrap();
        let root = tree.path().to_owned();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
        }
        for (path, source) in &sources.sources {
            let file = root.join(path);
            assert_eq!(std::fs::read(&file).unwrap(), source.content.bytes());
            #[cfg(unix)] {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
        drop(tree);
        assert!(!root.exists());
    }

    #[test]
    fn compiler_scratch_rejects_path_collisions_without_overwriting() {
        let tree = tempfile::tempdir().unwrap();
        let root = tree.path();
        // This path sorts before its descendant, so it becomes a file first.
        let sources = input(&[("collision", "original"), ("collision/nested.go", "replacement")]);
        assert!(materialize_sources(&sources,root).is_err());
        assert_eq!(std::fs::read(root.join("app/collision")).unwrap(), b"original");
        assert!(materialize(&sources).is_err());
        // Existing files and directories must survive failed exclusive opens.
        let mut sources = input(&[("existing", "replacement")]);
        sources.pins.clear();
        std::fs::write(root.join("app/existing"),b"original").unwrap();
        assert!(materialize_sources(&sources,root).is_err());
        assert_eq!(std::fs::read(root.join("app/existing")).unwrap(),b"original");
        std::fs::remove_file(root.join("app/existing")).unwrap();
        std::fs::create_dir(root.join("app/existing")).unwrap();
        assert!(materialize_sources(&sources,root).is_err());
        assert!(root.join("app/existing").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn compiler_scratch_exclusive_create_does_not_follow_a_leaf_symlink() {
        use std::os::unix::fs::symlink;
        let tree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target");
        std::fs::write(&target,b"original").unwrap();
        let mut sources = input(&[("link", "replacement")]);
        sources.pins.clear(); // Prepopulate the otherwise fresh tree for this race/collision check.
        std::fs::create_dir(tree.path().join("app")).unwrap();
        symlink(&target,tree.path().join("app/link")).unwrap();
        assert!(materialize_sources(&sources,tree.path()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(),b"original");
    }

    #[test]
    fn compiler_scratch_matches_previous_materialization_facts() {
        use std::io::Write;
        let mut sources = input(&[
            ("go.mod", "module example/app\ngo 1.23\nrequire example/dep v1.0.0\n"),
            ("vendor/modules.txt", "# example/dep v1.0.0\n## explicit; go 1.23\nexample/dep\n"),
            ("vendor/example/dep/dep.go", "package dep\nfunc Value() int {return 7}\n"),
            ("main.go", "package app\nimport (\"embed\"; \"example/dep\")\n//go:embed assets/data.bin\nvar data embed.FS\nfunc Value() int {return dep.Value()}\n"),
            ("assets/data.bin", ""),
        ]);
        let asset = sources.sources.get_mut("app/assets/data.bin").unwrap();
        asset.content = Content::Binary(vec![0,255,128,10]);
        asset.sha = graphicle_memory::text::sha256(asset.content.bytes());
        // Previous algorithm: temporary file plus no-clobber publication.
        let before = tempfile::tempdir().unwrap();
        for pin in &sources.pins { std::fs::create_dir(before.path().join(&pin.directory)).unwrap(); }
        for (path,source) in &sources.sources {
            let target = before.path().join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap()).unwrap();
            file.write_all(source.content.bytes()).unwrap();
            file.persist_noclobber(target).unwrap();
        }
        let after = materialize(&sources).unwrap();
        let request = json!({"roots":[{"directory":"app","module":"."}],"goos":"linux","goarch":"amd64","tests":false});
        let old = graphicle::extractors::go_types::review(before.path(),&request).unwrap();
        let new = graphicle::extractors::go_types::review(after.path(),&request).unwrap();
        assert_eq!(old,new);
        assert!(new["worlds"][0]["symbols"].as_array().is_some_and(|symbols| !symbols.is_empty()),"{new}");
    }

    #[test]
    fn cache_identity_preserves_build_and_trust_boundaries() {
        let profile: Profile =
            serde_json::from_value(json!({"goos":"linux","goarch":"amd64"})).unwrap();
        for path in [
            "code.go",
            "code_test.go",
            "go.mod",
            "go.sum",
            "go.work",
            "go.work.sum",
            "vendor/modules.txt",
            "native.s",
            "native.h",
            "native.c",
            "native.syso",
            "unknown.data",
        ] {
            assert_ne!(
                fact_key(&input(&[(path, "before")]), &profile, "helper"),
                fact_key(&input(&[(path, "after")]), &profile, "helper"),
                "{path}"
            );
        }
        let original = input(&[("code.go", "package app"), ("README.md", "before")]);
        let key = fact_key(&original, &profile, "helper");
        for mutation in ["policy", "repository", "directory", "modules", "exclusions"] {
            let mut changed = input(&[("code.go", "package app"), ("README.md", "before")]);
            let pin = &mut changed.pins[0];
            match mutation {
                "policy" => pin.policy_sha256.push('x'),
                "repository" => pin.repository_id.push('x'),
                "directory" => pin.directory.push('x'),
                "modules" => pin.modules.push("sub".into()),
                _ => pin.excluded_modules.push("sub".into()),
            }
            assert_ne!(key, fact_key(&changed, &profile, "helper"), "{mutation}");
        }
        assert_ne!(key, fact_key(&original, &profile, "new-helper"));
        let mut changed_profile = profile.clone();
        changed_profile.tests = true;
        assert_ne!(key, fact_key(&original, &changed_profile, "helper"));
        // GOWORK may select a file with any extension, not just go.work.
        changed_profile.workspace = "app/workspace.json".into();
        assert_ne!(
            fact_key(
                &input(&[("workspace.json", "before")]),
                &changed_profile,
                "helper"
            ),
            fact_key(
                &input(&[("workspace.json", "after")]),
                &changed_profile,
                "helper"
            )
        );
    }

    #[test]
    fn cache_identity_keeps_vendor_and_profile_excluded_embeds() {
        let profile: Profile =
            serde_json::from_value(json!({"goos":"linux","goarch":"amd64"})).unwrap();
        let code = "//go:build ignored\n\npackage asset\n//go:embed assets\nvar data string\n";
        let before = input(&[
            ("vendor/lib/embed_test.go", code),
            ("vendor/lib/assets/x.json", "before"),
            ("deploy.yaml", "before"),
        ]);
        let embedded = input(&[
            ("vendor/lib/embed_test.go", code),
            ("vendor/lib/assets/x.json", "after"),
            ("deploy.yaml", "before"),
        ]);
        let unrelated = input(&[
            ("vendor/lib/embed_test.go", code),
            ("vendor/lib/assets/x.json", "before"),
            ("deploy.yaml", "after"),
        ]);
        assert_ne!(
            fact_key(&before, &profile, "helper"),
            fact_key(&embedded, &profile, "helper")
        );
        assert_eq!(
            fact_key(&before, &profile, "helper"),
            fact_key(&unrelated, &profile, "helper")
        );
    }
}
