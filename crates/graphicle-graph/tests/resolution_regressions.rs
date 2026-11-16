use graphicle::{
    extract::{ExtractOptions, extract},
    watch::{RebuildOptions, rebuild},
};
use serde_json::Value;
use std::{fs, path::Path};

// Extraction uses process-wide run caches; edits must not overlap other runs.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn js_method_import_aliases_keep_the_method_caller_and_respect_shadowing() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(
        &root,
        "sender.js",
        concat!(
            "import { pauseFor as retryWait } from './policy.js';\n",
            "export class Sender { async send(attempt) { return retryWait(attempt); } }\n",
            "export class Shadow { send(retryWait) { return retryWait(2); } }\n",
            "export class Local { send() { const retryWait = x => x; return retryWait(2); } }\n"
        ),
    );
    write(
        &root,
        "policy.js",
        "export const pauseFor = n => Math.min(n, 30);\n",
    );
    let options = ExtractOptions {
        root: Some(root.clone()),
        cache_root: Some(root.clone()),
        ..Default::default()
    };
    for _ in 0..2 {
        let graph = extract(&[root.join("sender.js"), root.join("policy.js")], &options).unwrap();
        let calls: Vec<_> = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["relation"] == "calls" && e["target"] == "policy_pausefor")
            .collect();
        assert!(
            calls.iter().any(|e| e["source"] == "sender_sender_send"),
            "{graph:#}"
        );
        assert!(
            !calls
                .iter()
                .any(|e| e["source"] == "sender_shadow_send" || e["source"] == "sender_local_send"),
            "{graph:#}"
        );
    }
}

#[test]
fn go_calls_resolve_without_a_home_or_user_build_cache() {
    const CHILD: &str = "GRAPHICLE_GO_NO_HOME_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
        // A fresh process is required: the helper is cached in a OnceLock.
        // Match the query evaluator's minimal environment, without changing
        // process-global variables underneath concurrent Rust tests.
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .env_clear()
            .args([
                "--exact",
                "go_calls_resolve_without_a_home_or_user_build_cache",
                "--nocapture",
            ])
            .env(CHILD, "1");
        for key in [
            "PATH",
            "SystemRoot",
            "ComSpec",
            "PATHEXT",
            "TMPDIR",
            "TEMP",
            "TMP",
        ] {
            if let Some(value) = std::env::var_os(key) {
                child.env(key, value);
            }
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    assert!(std::env::var_os("HOME").is_none());
    assert!(std::env::var_os("GOCACHE").is_none());
    for module in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        if module {
            write(&root, "go.mod", "module example/delivery\ngo 1.23\n");
        }
        write(
            &root,
            "sender.go",
            "package delivery\ntype Sender struct{}\nfunc (Sender) Send(n int) int { return Delay(n) }\n",
        );
        write(
            &root,
            "policy.go",
            "package delivery\nfunc Delay(n int) int { return n + 1 }\n",
        );
        write(
            &root,
            "other/other.go",
            "package other\nfunc Delay(n int) int { return n * 2 }\n",
        );
        let graph = extract(
            &[
                root.join("sender.go"),
                root.join("policy.go"),
                root.join("other/other.go"),
            ],
            &ExtractOptions {
                root: Some(root.clone()),
                use_cache: false,
                ..Default::default()
            },
        )
        .unwrap();
        let nodes = graph["nodes"].as_array().unwrap();
        let calls: Vec<_> = graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["relation"] == "calls")
            .collect();
        assert_eq!(calls.len(), 1, "module={module}: {graph}");
        let node = |id: &Value| nodes.iter().find(|n| n["id"] == *id).unwrap();
        assert_eq!(node(&calls[0]["source"])["source_file"], "sender.go");
        assert_eq!(node(&calls[0]["target"])["source_file"], "policy.go");
        assert_eq!(calls[0]["resolution"], "go_types");
        assert!(
            graph["unresolved_calls"].as_array().unwrap().is_empty(),
            "{graph}"
        );
    }
}

#[test]
fn extracted_callback_candidates_remain_inferred_and_keep_unresolved_dispatch() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/fixture\ngo 1.23\n");
    write(
        &root,
        "main.go",
        "package fixture\nfunc A() {}\nfunc B() {}\nfunc Invoke(callback func()) { callback() }\nfunc Use() { Invoke(A) }\n",
    );
    let graph = extract(
        &[root.join("main.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root),
            ..Default::default()
        },
    )
    .unwrap();
    let calls: Vec<_> = graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "may_call")
        .collect();
    assert_eq!(calls.len(), 1, "{graph}");
    assert_eq!(calls[0]["evidence"], "function_value_candidate");
    assert_eq!(calls[0]["confidence"], "INFERRED");
    let nodes = graph["nodes"].as_array().unwrap();
    let target = nodes
        .iter()
        .find(|n| n["id"] == calls[0]["target"])
        .unwrap();
    assert_eq!(target["source_location"], "L2");
    assert!(
        graph["unresolved_calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["reason"] == "unresolved_type"),
        "{graph}"
    );
}

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
    // A promoted method of an embedded named interface is that interface's
    // method (L12); an anonymous interface's method has no node to name.
    let promoted = calls
        .iter()
        .find(|e| node(&e["source"])["label"] == "Promoted()")
        .unwrap_or_else(|| panic!("{graph}"));
    assert_eq!(node(&promoted["target"])["source_location"], "L12");
    for line in [8, 9] {
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
fn go_interface_calls_reach_the_interface_method_and_dispatch_to_its_implementers() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/fixture\ngo 1.23\n");
    write(
        &root,
        "main.go",
        concat!(
            "package fixture\n",
            "import \"example/fixture/store\"\n",
            "type Store interface {\n",
            "\tLoad() int\n",
            "}\n",
            "type mem struct{}\n",
            "func (*mem) Load() int { return 2 }\n",
            "type wrong struct{}\n",
            "func (wrong) Load() string { return \"\" }\n",
            "type Handlers struct{ store Store }\n",
            "func (h *Handlers) Get() int { return h.store.Load() }\n",
            "func Wire() *Handlers { return &Handlers{store: store.Disk{}} }\n",
        ),
    );
    write(
        &root,
        "store/disk.go",
        "package store\ntype Disk struct{}\nfunc (Disk) Load() int { return 1 }\n",
    );
    let graph = extract(
        &[root.join("main.go"), root.join("store/disk.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            use_cache: false,
            ..Default::default()
        },
    )
    .unwrap();
    let nodes = graph["nodes"].as_array().unwrap();
    let at = |id: &Value| {
        let n = nodes.iter().find(|n| n["id"] == *id).unwrap();
        format!("{}:{}", n["source_file"].as_str().unwrap(), n["source_location"].as_str().unwrap())
    };
    let edges = graph["edges"].as_array().unwrap();
    // The call names the interface method: that much is static.
    let call = edges
        .iter()
        .find(|e| e["relation"] == "calls" && at(&e["source"]) == "main.go:L11")
        .unwrap_or_else(|| panic!("{graph}"));
    assert_eq!(at(&call["target"]), "main.go:L4");
    assert_eq!(call["confidence"], "EXTRACTED");
    assert_eq!(call["evidence"], "type");
    // Which implementation runs is not: every repository type that satisfies
    // the interface, never a same-named method with another signature.
    let dispatch: Vec<_> = edges
        .iter()
        .filter(|e| e["relation"] == "dispatches_to")
        .collect();
    let mut pairs: Vec<_> = dispatch
        .iter()
        .map(|e| (at(&e["source"]), at(&e["target"])))
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        [
            ("main.go:L4".to_string(), "main.go:L7".to_string()),
            ("main.go:L4".to_string(), "store/disk.go:L3".to_string()),
        ],
        "{graph}"
    );
    for e in &dispatch {
        assert_eq!(e["confidence"], "INFERRED");
        assert_eq!(e["confidence_score"], 0.75);
        assert_eq!(e["resolution"], "go_types");
    }
    assert!(
        !graph["unresolved_calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["source_location"] == "L11"),
        "{graph}"
    );
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
            // The package is incomplete without its dependency, but go/types
            // still identifies the local call exactly. The dependency's call
            // stays untyped: had extraction downloaded the module, it would
            // read `target_not_in_graph` instead.
            assert_eq!(calls.len(), 1, "{graph}");
            assert_eq!(calls[0]["resolution"], "go_types", "{graph}");
            assert_eq!(unresolved.len(), 1, "{graph}");
            assert_eq!(unresolved[0]["reason"], "unresolved_type", "{graph}");
            assert_eq!(unresolved[0]["callee"], "dep.Equal", "{graph}");
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

/// A Go call the type checker could not resolve keeps its callee text, so an
/// answer about `RunAthenaServer` can say this call may name it; a call of a
/// function literal names nothing.
#[test]
fn unresolved_go_calls_keep_their_callee_text() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/unresolved\ngo 1.23\n");
    write(
        &root,
        "main.go",
        "package main\n\nimport \"example.com/missing/server\"\n\nfunc main() {\n\tserver.RunAthenaServer()\n\tfunc() {}()\n}\n",
    );
    let graph = extract(
        &[root.join("main.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            use_cache: false,
            ..Default::default()
        },
    )
    .unwrap();
    let unresolved = graph["unresolved_calls"].as_array().unwrap();
    let at = |line: &str| {
        unresolved
            .iter()
            .find(|u| u["source_location"] == line)
            .unwrap_or_else(|| panic!("no unresolved call at {line}: {graph}"))
    };
    assert_eq!(at("L6")["callee"], "server.RunAthenaServer", "{graph}");
    assert!(at("L7").get("callee").is_none(), "{graph}");
}

/// The helper reports every call it type-checked. A builtin or conversion is
/// not a function call and leaves nothing unresolved; a call inside a
/// function literal belongs to the enclosing function. Neither may report the
/// file as having no type information.
#[test]
fn go_builtins_and_closure_calls_do_not_mark_a_typed_file_incomplete() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/builtins\ngo 1.23\n");
    write(
        &root,
        "main.go",
        "package main\n\nfunc helper() int { return 1 }\n\nfunc main() {\n\txs := []int{}\n\txs = append(xs, helper())\n\t_ = int64(len(xs))\n\tfunc() { helper() }()\n}\n",
    );
    let graph = extract(
        &[root.join("main.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            use_cache: false,
            ..Default::default()
        },
    )
    .unwrap();
    let unresolved = graph["unresolved_calls"].as_array().unwrap();
    assert!(
        unresolved
            .iter()
            .all(|u| u["reason"] != "type_information_unavailable"),
        "{graph}"
    );
    for builtin in ["append", "len", "int64"] {
        assert!(unresolved.iter().all(|u| u["callee"] != builtin), "{graph}");
    }
    assert!(
        unresolved.iter().all(|u| u["callee"] != "helper"),
        "{graph}"
    );
    assert!(
        go_calls(&graph).contains(&("main()".into(), "helper()".into(), "L9".into())),
        "{graph}"
    );
}

/// `(caller label, callee label, call line)` of every Go `calls` edge.
fn go_calls(graph: &Value) -> Vec<(String, String, String)> {
    let label = |id: &Value| {
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == *id)
            .and_then(|n| n["label"].as_str())
            .unwrap_or("?")
            .to_string()
    };
    graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "calls")
        .map(|e| {
            (
                label(&e["source"]),
                label(&e["target"]),
                e["source_location"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn go_graph(source: &str) -> Value {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root, "go.mod", "module example/closures\ngo 1.22\n");
    write(&root, "p.go", source);
    extract(
        &[root.join("p.go")],
        &ExtractOptions {
            root: Some(root.clone()),
            use_cache: false,
            ..Default::default()
        },
    )
    .unwrap()
}

/// A closure is part of the function that writes it (upstream's call walk
/// enters `func_literal` too): callbacks, goroutines, defers, a closure kept
/// in a local and a closure nested in another all credit that function.
#[test]
fn go_closure_calls_belong_to_the_enclosing_function() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let graph = go_graph(concat!(
        "package p\n",                                                     // 1
        "\n",                                                              // 2
        "import \"sort\"\n",                                               // 3
        "\n",                                                              // 4
        "func helper() int { return 1 }\n",                                // 5
        "func other(x int) int { return x }\n",                            // 6
        "\n",                                                              // 7
        "func Run(xs []int) {\n",                                          // 8
        "\tsort.Slice(xs, func(i, j int) bool { return helper() < 0 })\n", // 9
        "\tgo func() { helper() }()\n",                                    // 10
        "\tdefer func() { other(2) }()\n",                                 // 11
        "\tf := func() { other(3) }\n",                                    // 12
        "\tf()\n",                                                         // 13
        "\tfunc() { func() { helper() }() }()\n",                          // 14
        "}\n",
    ));
    let calls = go_calls(&graph);
    for (callee, line) in [
        ("helper()", "L9"),
        ("helper()", "L10"),
        ("other()", "L11"),
        ("other()", "L12"),
        ("helper()", "L14"),
    ] {
        assert!(
            calls.contains(&("Run()".into(), callee.into(), line.into())),
            "{callee} {line}: {calls:?}"
        );
    }
    let unresolved = graph["unresolved_calls"].as_array().unwrap();
    assert!(
        unresolved
            .iter()
            .all(|u| u["callee"] != "helper" && u["callee"] != "other"),
        "{graph}"
    );
}

/// A call in a package-level `var` initialiser, directly or in a closure it
/// holds (the swappable-hook pattern), is credited to the file, which owns
/// the initialiser (upstream #2360).
#[test]
fn go_package_var_initialiser_calls_belong_to_the_file() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let graph = go_graph(concat!(
        "package p\n",                                        // 1
        "\n",                                                 // 2
        "func build() int { return 1 }\n",                    // 3
        "func helper() int { return 2 }\n",                   // 4
        "\n",                                                 // 5
        "var value = build()\n",                              // 6
        "var Hook = func() int { return helper() }\n",        // 7
        "var (\n\tsize = len(\"x\")\n\tkind = int64(3)\n)\n", // 8-11
    ));
    let calls = go_calls(&graph);
    for (callee, line) in [("build()", "L6"), ("helper()", "L7")] {
        assert!(
            calls.contains(&("p.go".into(), callee.into(), line.into())),
            "{callee} {line}: {calls:?}"
        );
    }
    let unresolved = graph["unresolved_calls"].as_array().unwrap();
    assert!(unresolved.is_empty(), "{graph}");
}
