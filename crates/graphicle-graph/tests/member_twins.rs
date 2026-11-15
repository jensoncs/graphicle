// Adapted from Graphify 0.9.108-0.9.109 (bd720f1f, 7b6519c4), Apache-2.0;
// see NOTICE.md.
//! `make_id` drops a leading `_` and folds case, so `_bar()`/`bar()` and
//! `run()`/`Run()` of one Java or Kotlin class minted one id: the second
//! declaration vanished and its calls were credited to the survivor. Ported
//! from upstream's `test_java_underscore_ids.py` and
//! `test_kotlin_underscore_ids.py`.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;
use std::collections::BTreeSet;

struct Graph {
    nodes: Vec<Value>,
    calls: BTreeSet<(String, String)>,
}

impl Graph {
    /// The one node of `file` labelled `label`.
    fn one(&self, file: &str, label: &str) -> String {
        let ids: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| n["source_file"] == file && n["label"] == label)
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids.len(), 1, "{file} {label}: {:?}", self.nodes);
        ids[0].clone()
    }
    fn calls(&self, from: &str, to: &str) -> bool {
        self.calls.contains(&(from.to_string(), to.to_string()))
    }
}

fn graph(files: &[(&str, &str)]) -> Graph {
    graphicle::rcfile::set_language_overrides(None);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let paths = files
        .iter()
        .map(|(rel, text)| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let result = extract(
        &paths,
        &ExtractOptions {
            root: Some(root.clone()),
            cache_root: Some(root.join("cache")),
            parallel: false,
            ..Default::default()
        },
    )
    .unwrap();
    Graph {
        nodes: result["nodes"].as_array().unwrap().clone(),
        calls: result["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["relation"] == "calls")
            .map(|e| {
                (
                    e["source"].as_str().unwrap().to_string(),
                    e["target"].as_str().unwrap().to_string(),
                )
            })
            .collect(),
    }
}

// -- Java (0.9.108, #5151) ------------------------------------------------

const FOO_JAVA: &str = "package demo;

public class Foo {
    public void bar() { helper(); }
    private void _bar() { other(); }
    void helper() {}
    void other() {}
    void Run() {}
    void run() {}
    public void caller() { _bar(); bar(); run(); Run(); }
}
";

#[test]
fn java_underscore_twin_keeps_its_own_node_and_calls() {
    let g = graph(&[("demo/Foo.java", FOO_JAVA)]);
    let f = "demo/Foo.java";
    let (bar, ubar) = (g.one(f, ".bar()"), g.one(f, "._bar()"));
    let (helper, other, caller) = (
        g.one(f, ".helper()"),
        g.one(f, ".other()"),
        g.one(f, ".caller()"),
    );
    assert_ne!(bar, ubar);
    // The one public name keeps the plain id; a name that collides with
    // nothing is untouched.
    assert!(
        bar.ends_with("_foo_bar") && helper.ends_with("_foo_helper"),
        "{bar} {helper}"
    );
    assert!(
        g.calls(&bar, &helper) && g.calls(&ubar, &other),
        "{:?}",
        g.calls
    );
    assert!(
        !g.calls(&bar, &other) && !g.calls(&ubar, &helper),
        "{:?}",
        g.calls
    );
    assert!(
        g.calls(&caller, &bar) && g.calls(&caller, &ubar),
        "{:?}",
        g.calls
    );
}

#[test]
fn java_case_only_twins_are_both_kept() {
    let g = graph(&[("demo/Foo.java", FOO_JAVA)]);
    let f = "demo/Foo.java";
    let (run, upper, caller) = (
        g.one(f, ".run()"),
        g.one(f, ".Run()"),
        g.one(f, ".caller()"),
    );
    assert_ne!(run, upper);
    assert!(
        g.calls(&caller, &run) && g.calls(&caller, &upper),
        "{:?}",
        g.calls
    );
}

#[test]
fn java_constructor_enum_and_interface_bodies() {
    let g = graph(&[(
        "demo/Widget.java",
        "package demo;
public class Widget {
    Widget() { init(); }
    void widget() { tidy(); }
    void init() {}
    void tidy() {}
    enum Mode { A; void _go() {} void go() { _go(); } }
    interface Cb { void _x(); void X(); }
}
",
    )]);
    let f = "demo/Widget.java";
    let (ctor, method) = (g.one(f, ".Widget()"), g.one(f, ".widget()"));
    let (go, ugo) = (g.one(f, ".go()"), g.one(f, "._go()"));
    assert!(ctor != method && go != ugo);
    assert_ne!(g.one(f, "._x()"), g.one(f, ".X()"));
    assert!(g.calls(&ctor, &g.one(f, ".init()")), "{:?}", g.calls);
    assert!(g.calls(&method, &g.one(f, ".tidy()")), "{:?}", g.calls);
    assert!(g.calls(&go, &ugo) && !g.calls(&go, &go), "{:?}", g.calls);
}

#[test]
fn java_cross_file_receiver_calls_pick_the_right_twin() {
    let g = graph(&[
        ("demo/Foo.java", FOO_JAVA),
        (
            "demo/User.java",
            "package demo;
public class User {
    void lower(Foo f) { f.run(); }
    void upper(Foo f) { f.Run(); }
}
",
        ),
    ]);
    let (foo, usr) = ("demo/Foo.java", "demo/User.java");
    let (lower, upper) = (g.one(usr, ".lower()"), g.one(usr, ".upper()"));
    assert!(g.calls(&lower, &g.one(foo, ".run()")), "{:?}", g.calls);
    assert!(g.calls(&upper, &g.one(foo, ".Run()")), "{:?}", g.calls);
    assert!(!g.calls(&lower, &g.one(foo, ".Run()")), "{:?}", g.calls);
}

// -- Kotlin (0.9.109, #5152) ----------------------------------------------

const FOO_KT: &str = "package demo

class Foo {
    fun bar() { helper() }
    fun _bar() { other() }
    fun helper() {}
    fun other() {}
    fun caller() { _bar(); bar(); run(); Run() }
    companion object {
        fun run() { helper() }
        fun Run() {}
    }
}

class User(val foo: Foo) {
    fun use() { foo._bar(); foo.bar() }
}
";

#[test]
fn kotlin_underscore_twin_keeps_its_own_node_and_calls() {
    let g = graph(&[("demo/Foo.kt", FOO_KT)]);
    let f = "demo/Foo.kt";
    let (bar, ubar) = (g.one(f, ".bar()"), g.one(f, "._bar()"));
    let (helper, other, caller) = (
        g.one(f, ".helper()"),
        g.one(f, ".other()"),
        g.one(f, ".caller()"),
    );
    assert_ne!(bar, ubar);
    assert!(
        bar.ends_with("_foo_bar") && helper.ends_with("_foo_helper"),
        "{bar} {helper}"
    );
    assert!(
        g.calls(&bar, &helper) && g.calls(&ubar, &other),
        "{:?}",
        g.calls
    );
    assert!(
        !g.calls(&bar, &other) && !g.calls(&ubar, &helper),
        "{:?}",
        g.calls
    );
    assert!(
        g.calls(&caller, &bar) && g.calls(&caller, &ubar),
        "{:?}",
        g.calls
    );
}

#[test]
fn kotlin_typed_receiver_calls_reach_each_twin() {
    let g = graph(&[("demo/Foo.kt", FOO_KT)]);
    let f = "demo/Foo.kt";
    let usage = g.one(f, ".use()");
    assert!(g.calls(&usage, &g.one(f, ".bar()")), "{:?}", g.calls);
    assert!(g.calls(&usage, &g.one(f, "._bar()")), "{:?}", g.calls);
}

#[test]
fn kotlin_case_only_twins_in_a_companion_are_kept() {
    let g = graph(&[("demo/Foo.kt", FOO_KT)]);
    let f = "demo/Foo.kt";
    let (run, upper, caller) = (
        g.one(f, ".run()"),
        g.one(f, ".Run()"),
        g.one(f, ".caller()"),
    );
    let helper = g.one(f, ".helper()");
    assert_ne!(run, upper);
    assert!(
        g.calls(&run, &helper) && !g.calls(&upper, &helper),
        "{:?}",
        g.calls
    );
    assert!(
        g.calls(&caller, &run) && g.calls(&caller, &upper),
        "{:?}",
        g.calls
    );
}
