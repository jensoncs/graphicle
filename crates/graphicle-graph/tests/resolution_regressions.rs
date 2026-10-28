use graphicle::{
    extract::{ExtractOptions, extract},
    watch::{RebuildOptions, rebuild},
};
use serde_json::Value;
use std::{fs, path::Path};

// Extraction uses process-wide run caches; edits must not overlap other runs.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn go_internal_and_external_test_calls_resolve_without_same_name_guesses() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/fixture\ngo 1.23\n");
    write(
        &root,
        "main.go",
        "package fixture\ntype A struct{}\ntype B struct{}\nfunc (A) Target() {}\nfunc (B) Target() {}\n",
    );
    write(
        &root,
        "internal_test.go",
        "package fixture\nfunc InternalTestHelper() { A{}.Target() }\n",
    );
    write(
        &root,
        "external_test.go",
        "package fixture_test\nimport f \"example/fixture\"\nfunc ExternalTestHelper() { f.A{}.Target() }\n",
    );
    write(
        &root,
        "excluded_test.go",
        "//go:build graphicle_excluded_fixture\n\npackage fixture\nfunc Excluded() { B{}.Target() }\n",
    );
    let paths = [
        "main.go",
        "internal_test.go",
        "external_test.go",
        "excluded_test.go",
    ]
    .map(|p| root.join(p));
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        ..Default::default()
    };
    for _ in 0..2 {
        let graph = extract(&paths, &options).unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let node = |id: &Value| nodes.iter().find(|n| n["id"] == *id).unwrap();
        let calls: Vec<_> = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["relation"] == "calls")
            .collect();
        for file in ["internal_test.go", "external_test.go"] {
            let call = calls
                .iter()
                .find(|e| node(&e["source"])["source_file"] == file)
                .unwrap_or_else(|| panic!("missing {file}: {graph}"));
            assert_eq!(node(&call["target"])["source_file"], "main.go");
            assert_eq!(node(&call["target"])["source_location"], "L4");
            assert_eq!(call["resolution"], "go_types");
        }
        assert_eq!(calls.len(), 2, "{graph}");
    }
}

fn write(root: &Path, name: &str, body: &str) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn records(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join(".graphicle/graph.json")).unwrap()).unwrap()
}

fn comparable(graph: &Value) -> Vec<String> {
    let mut records = Vec::new();
    for bucket in ["nodes", "links"] {
        for record in graph[bucket].as_array().unwrap() {
            let mut record = record.clone();
            record.as_object_mut().unwrap().shift_remove("community");
            records.push(format!(
                "{bucket}:{}",
                graphicle::extract_normalize::canonical(&record)
            ));
        }
    }
    records.sort();
    records
}

#[test]
fn deleted_and_moved_sources_match_clean_rebuild() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for no_cluster in [true, false] {
        for operation in ["rename", "delete", "directory"] {
            for explicit in [true, false] {
                let temp = tempfile::tempdir().unwrap();
                let root = temp.path().canonicalize().unwrap();
                let original = if operation == "directory" {
                    "old/service.py"
                } else {
                    "service.py"
                };
                write(
                    &root,
                    original,
                    "def target():\n    pass\ndef caller():\n    target()\n",
                );
                let options = RebuildOptions {
                    no_cluster,
                    ..Default::default()
                };
                assert!(rebuild(&root, &options).unwrap().success);
                let changed = match operation {
                    "rename" => {
                        fs::create_dir_all(root.join("package")).unwrap();
                        fs::rename(root.join(original), root.join("package/moved.py")).unwrap();
                        vec![root.join(original), root.join("package/moved.py")]
                    }
                    "directory" => {
                        fs::rename(root.join("old"), root.join("package")).unwrap();
                        vec![root.join("old"), root.join("package")]
                    }
                    _ => {
                        fs::remove_file(root.join(original)).unwrap();
                        vec![root.join(original)]
                    }
                };
                let update = RebuildOptions {
                    changed_paths: explicit.then_some(changed),
                    ..options.clone()
                };
                let result = rebuild(&root, &update).unwrap();
                assert!(result.success, "{operation}: {:?}", result.messages);
                let incremental = records(&root);
                assert!(
                    incremental["nodes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|n| n["source_file"] != original),
                    "{operation}, explicit={explicit}, no_cluster={no_cluster}: {incremental}"
                );
                if operation != "delete" {
                    fs::remove_dir_all(root.join(".graphicle")).unwrap();
                    assert!(rebuild(&root, &options).unwrap().success);
                    assert_eq!(
                        comparable(&incremental),
                        comparable(&records(&root)),
                        "{operation}, explicit={explicit}, no_cluster={no_cluster}"
                    );
                } else {
                    assert!(incremental["nodes"].as_array().unwrap().is_empty());
                    assert!(incremental["links"].as_array().unwrap().is_empty());
                }
            }
        }
    }
}

#[test]
fn rebound_python_import_is_unsupported_on_fresh_and_cached_extraction() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "a.py", "def target():\n    pass\n");
    write(
        &root,
        "b.py",
        "from a import target\ndef replacement():\n    pass\ntarget = replacement\ndef caller():\n    target()\n",
    );
    let paths = vec![root.join("a.py"), root.join("b.py")];
    for precise in [false, true] {
        let options = ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.clone()),
            precise,
            ..Default::default()
        };
        for _ in 0..2 {
            let graph = extract(&paths, &options).unwrap();
            assert!(
                !graph["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["relation"] == "calls"
                        && e["source"] == "b_caller"
                        && e["target"] == "a_target"),
                "{graph}"
            );
            assert!(
                graph["unresolved_calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["reason"] == "unsupported_rebinding"),
                "{graph}"
            );
        }
    }
}

#[test]
fn go_calls_use_receiver_identity_and_active_build() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example.test/fixture\n\ngo 1.20\n");
    write(
        &root,
        "main.go",
        "package fixture\ntype A struct{}\ntype B struct{}\nfunc (A) Target() {}\nfunc (B) Target() {}\nfunc Run() { A{}.Target() }\nfunc Typed(a *A) { a.Target() }\nfunc Dynamic(a interface{ Target() }) { a.Target() }\nfunc Value(f func()) { f() }\nfunc Build() { Active() }\ntype Embedded struct{ interfaceField }\ntype interfaceField interface{ Target() }\nfunc Promoted(a Embedded) { a.Target() }\n",
    );
    write(
        &root,
        "active.go",
        "//go:build !graphicle_excluded_fixture\n\npackage fixture\nfunc Active() {}\n",
    );
    write(
        &root,
        "excluded.go",
        "//go:build graphicle_excluded_fixture\n\npackage fixture\nfunc Active() {}\nfunc Wrong() { Active() }\n",
    );
    let paths = ["main.go", "active.go", "excluded.go"].map(|p| root.join(p));
    let graph = extract(
        &paths,
        &ExtractOptions {
            root: Some(root.clone()),
            use_cache: false,
            ..Default::default()
        },
    )
    .unwrap();
    let calls: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "calls")
        .collect();
    assert!(
        calls.iter().all(|e| e["resolution"] == "go_types"),
        "{graph}"
    );
    let node = |id: &Value| {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == *id)
            .unwrap()
    };
    assert!(
        !calls
            .iter()
            .any(|e| node(&e["target"])["source_location"] == "L5"
                && node(&e["target"])["source_file"] == "main.go"),
        "{graph}"
    );
    assert!(
        !calls
            .iter()
            .any(|e| node(&e["target"])["source_file"] == "excluded.go"
                || node(&e["source"])["source_file"] == "excluded.go"),
        "{graph}"
    );
    if std::process::Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        for caller in ["Run()", "Typed()", "Build()"] {
            let call = calls
                .iter()
                .find(|e| node(&e["source"])["label"] == caller)
                .unwrap_or_else(|| panic!("missing {caller}: {graph}"));
            let target = node(&call["target"]);
            assert_eq!(target["source_location"], "L4", "{graph}");
        }
    }
    for line in [8, 9, 13] {
        assert!(
            graph["unresolved_calls"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["source_location"] == format!("L{line}")),
            "{graph}"
        );
    }
}

#[test]
fn python_rebinding_invalidates_resolved_cache_and_restores_clean_import() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "a.py", "def target():\n    pass\n");
    let paths = [root.join("a.py"), root.join("b.py")];
    let options = ExtractOptions {
        root: Some(root.clone()),
        use_cache: false,
        ..Default::default()
    };
    for rebound in [false, true, true, false] {
        write(
            &root,
            "b.py",
            &format!(
                "from a import target\n{}def caller():\n    target()\n",
                if rebound {
                    "target = unknown_factory()\n"
                } else {
                    ""
                }
            ),
        );
        let (graph, _) =
            graphicle::incremental::extract(&paths, &options, Some(cache.path()), "rebinding-test")
                .unwrap();
        let imported = graph["edges"].as_array().unwrap().iter().any(|e| {
            e["relation"] == "calls" && e["source"] == "b_caller" && e["target"] == "a_target"
        });
        assert_eq!(imported, !rebound, "{graph}");
        if rebound {
            assert!(
                graph["unresolved_calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|u| u["reason"] == "unsupported_rebinding"),
                "{graph}"
            );
        }
    }
}

#[test]
fn updates_preserve_unsupported_calls_until_their_source_is_deleted() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for no_cluster in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        write(&root, "a.py", "def target():\n    pass\n");
        write(
            &root,
            "b.py",
            "from a import target\ntarget = None\ndef caller():\n    target()\n",
        );
        write(&root, "c.py", "def unrelated():\n    return 1\n");
        let options = RebuildOptions {
            no_cluster,
            ..Default::default()
        };
        assert!(rebuild(&root, &options).unwrap().success);
        assert!(
            records(&root).to_string().contains("unsupported_rebinding"),
            "no_cluster={no_cluster}: {}",
            records(&root)
        );
        write(&root, "c.py", "def unrelated():\n    return 2\n");
        assert!(
            rebuild(
                &root,
                &RebuildOptions {
                    changed_paths: Some(vec![root.join("c.py")]),
                    ..options.clone()
                }
            )
            .unwrap()
            .success
        );
        assert!(
            records(&root).to_string().contains("unsupported_rebinding"),
            "no_cluster={no_cluster}: {}",
            records(&root)
        );
        fs::remove_file(root.join("b.py")).unwrap();
        assert!(
            rebuild(
                &root,
                &RebuildOptions {
                    changed_paths: Some(vec![root.join("b.py")]),
                    ..options
                }
            )
            .unwrap()
            .success
        );
        assert!(!records(&root).to_string().contains("unsupported_rebinding"));
    }
}

#[test]
fn cached_external_go_dependency_keeps_local_calls_offline() {
    use std::{io::Write, process::Command};

    // Isolate environment changes in children; fixture setup uses only a local
    // file proxy, never the network or the developer's module cache.
    if let Some(root) = std::env::var_os("GRAPHICLE_GO_CACHED_DEPENDENCY") {
        let root = std::path::PathBuf::from(root);
        let graph = extract(
            &[root.join("main.go")],
            &ExtractOptions {
                root: Some(root.clone()),
                use_cache: false,
                ..Default::default()
            },
        )
        .unwrap();
        let calls: Vec<_> = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["relation"] == "calls")
            .collect();
        let unresolved = graph["unresolved_calls"].as_array().unwrap();
        if std::env::var_os("GRAPHICLE_GO_CACHE_MISSING").is_some() {
            assert!(calls.is_empty(), "{graph}");
            assert_eq!(unresolved.len(), 2, "{graph}");
            assert!(unresolved.iter().all(|u| u["reason"] == "unresolved_type"));
        } else {
            assert_eq!(calls.len(), 1, "{graph}");
            assert_eq!(calls[0]["resolution"], "go_types", "{graph}");
            let nodes = graph["nodes"].as_array().unwrap();
            let node = |id: &Value| nodes.iter().find(|n| n["id"] == *id).unwrap();
            assert_eq!(node(&calls[0]["source"])["label"], "Handle()");
            assert_eq!(node(&calls[0]["target"])["label"], "helper()");
            // The external call is typed but intentionally has no graph node.
            assert_eq!(unresolved.len(), 1, "{graph}");
            assert_eq!(unresolved[0]["reason"], "target_not_in_graph", "{graph}");

            // Ambient cache contents must not enter an explicitly pinned review.
            let report = graphicle::extractors::go_types::review(
                &root,
                &serde_json::json!({"roots":[{"directory":".","module":"."}],
                    "goos":"linux","goarch":"amd64","tags":[],"tests":false,"workspace":""}),
            )
            .unwrap();
            assert!(
                !report["worlds"][0]["issues"].as_array().unwrap().is_empty(),
                "{report}"
            );
        }
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let proxy = root.join("proxy");
    let cache = root.join("modules");
    let app = root.join("app");
    let dep_mod = "module example.test/dep\ngo 1.23\n";
    write(&proxy, "example.test/dep/@v/v1.0.0.mod", dep_mod);
    write(
        &proxy,
        "example.test/dep/@v/v1.0.0.info",
        r#"{"Version":"v1.0.0","Time":"2026-01-01T00:00:00Z"}"#,
    );
    let mut archive = zip::ZipWriter::new(
        fs::File::create(proxy.join("example.test/dep/@v/v1.0.0.zip")).unwrap(),
    );
    for (name, source) in [
        ("go.mod", dep_mod),
        (
            "dep.go",
            "package dep\nfunc Equal(a, b int) bool { return a == b }\n",
        ),
    ] {
        archive
            .start_file(
                format!("example.test/dep@v1.0.0/{name}"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(source.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
    write(
        &app,
        "go.mod",
        "module example.test/app\ngo 1.23\nrequire example.test/dep v1.0.0\n",
    );
    write(
        &app,
        "main.go",
        "package app\nimport \"example.test/dep\"\nfunc helper() bool { return dep.Equal(1, 1) }\nfunc Handle() bool { return helper() }\n",
    );
    let downloaded = Command::new("go")
        .args(["mod", "download", "example.test/dep"])
        .current_dir(&app)
        .env("GOMODCACHE", &cache)
        .env(
            "GOPROXY",
            reqwest::Url::from_directory_path(&proxy).unwrap().as_str(),
        )
        .env("GOSUMDB", "off")
        .env("GOVCS", "*:off")
        .env("GOTOOLCHAIN", "local")
        .env("GOENV", "off")
        // Let TempDir remove the seeded cache after the test.
        .env("GOFLAGS", "-modcacherw")
        .env("GOWORK", "off")
        .env("GOPRIVATE", "")
        .env("GONOPROXY", "")
        .env("GONOSUMDB", "")
        .output()
        .unwrap();
    assert!(downloaded.status.success(), "{downloaded:?}");
    let before = ["go.mod", "go.sum"].map(|p| fs::read(app.join(p)).unwrap());
    for missing in [false, true] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "cached_external_go_dependency_keeps_local_calls_offline",
                "--nocapture",
            ])
            .env("GRAPHICLE_GO_CACHED_DEPENDENCY", &app)
            .env(
                "GOMODCACHE",
                if missing {
                    root.join("empty")
                } else {
                    cache.clone()
                },
            )
            // If extraction honors inherited settings it will download the
            // missing dependency from this proxy and the negative check fails.
            .env(
                "GOPROXY",
                reqwest::Url::from_directory_path(&proxy).unwrap().as_str(),
            )
            .env("GOFLAGS", "-mod=mod")
            .env("GOSUMDB", "off");
        if missing {
            command.env("GRAPHICLE_GO_CACHE_MISSING", "1");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            before,
            ["go.mod", "go.sum"].map(|p| fs::read(app.join(p)).unwrap())
        );
    }
}

#[test]
fn standard_http_import_keeps_local_go_calls() {
    // A child process can inherit hostile Go settings without racing other tests.
    if std::env::var_os("GRAPHICLE_GO_HOSTILE_FIXTURE").is_none() {
        let user_cache = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "standard_http_import_keeps_local_go_calls",
                "--nocapture",
            ])
            .env("GRAPHICLE_GO_HOSTILE_FIXTURE", "1")
            .env("GOOS", "linux")
            .env("GOARCH", "amd64")
            .env("CGO_ENABLED", "1")
            .env("GOFLAGS", "-mod=mod")
            .env("GOPRIVATE", "*")
            .env("GONOPROXY", "*")
            .env("GOMODCACHE", user_cache.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_dir(user_cache.path()).unwrap().count(), 0);
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/httpfixture\ngo 1.23\n");
    write(
        &root,
        "main.go",
        "package sample\nimport \"net/http\"\nfunc Target() int { return http.StatusOK }\nfunc Caller() int { return Target() }\n",
    );
    let graph = extract(
        &[root.join("main.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "calls" && e["resolution"] == "go_types"),
        "{graph}"
    );
}
