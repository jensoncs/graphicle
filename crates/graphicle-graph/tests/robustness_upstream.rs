// Adapted from Graphify 0.9.100-0.9.109 (7c305ba8, daaf455d), Apache-2.0;
// see NOTICE.md.
//! `source_file` stays repo-relative when the scan root is reached through a
//! symlink (#4624): otherwise every `page.tsx` reads as one file and pruning
//! one deleted page prunes another. A file whose extraction fails keeps its
//! last good records in an incremental update (#5189).
use graphicle::api::{ExtractOptions, extract};
use graphicle::watch::{RebuildOptions, rebuild};
use serde_json::Value;
use std::{collections::BTreeSet, os::unix::fs::PermissionsExt, path::Path};

const PAGE: &str = "export default function ListPage() {\n  return helper();\n}\nfunction helper() { return 1; }\n";

/// `<tmp>/link -> <tmp>/real`, with the tempdir itself left unresolved (on
/// macOS it sits under the `/var -> /private/var` symlink too).
fn linked_root(tmp: &Path) -> std::path::PathBuf {
    let real = tmp.join("real");
    std::fs::create_dir(&real).unwrap();
    let link = tmp.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    link
}

fn source_files(files: &[&str]) -> BTreeSet<String> {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = linked_root(tmp.path());
    let paths = files
        .iter()
        .map(|rel| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, PAGE).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let cache = tempfile::tempdir().unwrap();
    let result = extract(
        &paths,
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(cache.path().to_path_buf()),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["source_file"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn source_file_is_repo_relative_under_a_symlinked_root() {
    assert_eq!(
        source_files(&["web/app/customers/page.tsx"]),
        BTreeSet::from(["web/app/customers/page.tsx".to_string()])
    );
}

#[test]
fn same_basename_files_stay_distinct_under_a_symlinked_root() {
    assert_eq!(
        source_files(&["web/app/a/page.tsx", "web/app/b/page.tsx"]),
        BTreeSet::from([
            "web/app/a/page.tsx".to_string(),
            "web/app/b/page.tsx".to_string()
        ])
    );
}

/// Only the directory is resolved: a file symlink stays its own source, not
/// a second spelling of its target (#2832).
#[test]
fn a_file_symlink_under_a_symlinked_root_stays_distinct() {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = linked_root(tmp.path());
    std::fs::create_dir_all(root.join("web")).unwrap();
    std::fs::write(root.join("web/page.tsx"), PAGE).unwrap();
    std::os::unix::fs::symlink(root.join("web/page.tsx"), root.join("web/alias.tsx")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let result = extract(
        &[root.join("web/page.tsx"), root.join("web/alias.tsx")],
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(cache.path().to_path_buf()),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    let files: BTreeSet<_> = result["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["source_file"].as_str())
        .collect();
    assert_eq!(
        files,
        BTreeSet::from(["web/alias.tsx", "web/page.tsx"]),
        "{result}"
    );
}

// -- a failed source keeps its last good records (0.9.109, #5189) ---------

fn labels(root: &Path, file: &str) -> BTreeSet<String> {
    let graph: Value =
        serde_json::from_slice(&std::fs::read(root.join(".graphicle/graph.json")).unwrap())
            .unwrap();
    graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["source_file"] == file)
        .filter_map(|n| n["label"].as_str().map(str::to_string))
        .collect()
}

/// An unreadable file (permissions, a write in progress) fails extraction.
/// The update keeps what the graph knew about it and replaces the files that
/// did extract; once readable again, its new content replaces the old.
#[test]
fn a_failed_source_keeps_its_last_good_records_until_it_extracts() {
    for no_cluster in [true, false] {
        failed_source_round_trip(no_cluster);
    }
}

fn failed_source_round_trip(no_cluster: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (bad, good) = (root.join("bad.py"), root.join("good.py"));
    std::fs::write(&bad, "def helper():\n    return 1\n").unwrap();
    std::fs::write(&good, "def first():\n    return 1\n").unwrap();
    let full = RebuildOptions {
        no_cluster,
        ..Default::default()
    };
    assert!(rebuild(&root, &full).unwrap().success);
    assert!(labels(&root, "bad.py").contains("helper()"));

    std::fs::write(&bad, "def renamed():\n    return 2\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::write(&good, "def second():\n    return 2\n").unwrap();
    let update = RebuildOptions {
        changed_paths: Some(vec![bad.clone(), good.clone()]),
        ..full.clone()
    };
    let result = rebuild(&root, &update);
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    let result = result.unwrap();
    assert!(
        labels(&root, "bad.py").contains("helper()"),
        "{:?}",
        result.messages
    );
    assert!(
        labels(&root, "good.py").contains("second()"),
        "{:?}",
        result.messages
    );
    assert!(
        !labels(&root, "good.py").contains("first()"),
        "{:?}",
        result.messages
    );

    let retry = RebuildOptions {
        changed_paths: Some(vec![bad.clone()]),
        ..full
    };
    assert!(rebuild(&root, &retry).unwrap().success);
    let after = labels(&root, "bad.py");
    assert!(
        after.contains("renamed()") && !after.contains("helper()"),
        "{after:?}"
    );
}

/// A failed source is listed as failed and not as extracted, so a merge of
/// this result keeps its stored records, and it publishes no records.
#[test]
fn a_failed_source_is_not_extraction_provenance() {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (bad, good) = (root.join("bad.py"), root.join("good.py"));
    std::fs::write(&bad, "def helper():\n    return 1\n").unwrap();
    std::fs::write(&good, "def other():\n    return 1\n").unwrap();
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = extract(
        &[bad.clone(), good.clone()],
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.join("cache")),
            parallel: false,
            ..Default::default()
        },
    );
    std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o644)).unwrap();
    let result = result.unwrap();
    let list = |key: &str| -> Vec<String> {
        result[key]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(
        list("failed_sources"),
        vec![bad.to_string_lossy().into_owned()]
    );
    assert_eq!(
        list("extracted_sources"),
        vec![good.to_string_lossy().into_owned()]
    );
    assert!(
        result["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["source_file"] != "bad.py"),
        "{result}"
    );
}
