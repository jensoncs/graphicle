//! Rust methods are named by the type their `impl` is for, and calls reach
//! them through `self.m()`, `Self::m()`, `Type::m()` and receivers whose
//! type the code states: a typed parameter, a typed `let`, a struct literal
//! or a `Type::f()` result.
use graphicle_memory::structure::{analyze, graph};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

fn calls(files: &[(&str, &str)]) -> BTreeSet<[String; 4]> {
    let mut analyses = Map::new();
    for (path, content) in files {
        analyses.insert((*path).to_string(), analyze(path, content).unwrap());
    }
    let raw = graph(&Value::Object(analyses)).unwrap();
    raw["symbol_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            ["path", "caller", "callee_path", "callee"].map(|k| r[k].as_str().unwrap().to_string())
        })
        .collect()
}

fn call(a: &str, b: &str, c: &str, d: &str) -> [String; 4] {
    [a.into(), b.into(), c.into(), d.into()]
}

const LIB: &str = "pub mod store;
use crate::store::Store;
pub struct Cache {
    n: u32,
}
impl Cache {
    pub fn new() -> Self {
        Self::empty()
    }
    fn empty() -> Self {
        Cache { n: 0 }
    }
    pub fn get(&self) -> u32 {
        self.peek()
    }
    fn peek(&self) -> u32 {
        self.n
    }
}
impl Default for Cache {
    fn default() -> Self {
        Cache::new()
    }
}
pub fn run(store: &Store) -> u32 {
    let c = Cache::new();
    let s = Store::open()?;
    let t: Store = make();
    let lit = Cache { n: 1 };
    store.save();
    t.save();
    lit.peek();
    c.get() + s.size()
}
";

const STORE: &str = "pub struct Store;
impl Store {
    pub fn open() -> Result<Store, ()> {
        Ok(Store)
    }
    pub fn save(&self) {}
    pub fn size(&self) -> u32 {
        0
    }
}
";

#[test]
fn methods_are_called_through_self_and_type_paths() {
    let got = calls(&[("src/lib.rs", LIB), ("src/store.rs", STORE)]);
    let lib = "src/lib.rs";
    for (caller, callee) in [
        ("Cache.new", "Cache.empty"),
        ("Cache.get", "Cache.peek"),
        ("Cache.default", "Cache.new"),
        ("run", "Cache.new"),
    ] {
        assert!(
            got.contains(&call(lib, caller, lib, callee)),
            "{caller} -> {callee}: {got:?}"
        );
    }
    assert!(
        got.contains(&call(lib, "run", "src/store.rs", "Store.open")),
        "{got:?}"
    );
}

#[test]
fn receivers_of_a_stated_type_reach_its_methods() {
    let got = calls(&[("src/lib.rs", LIB), ("src/store.rs", STORE)]);
    let lib = "src/lib.rs";
    assert!(got.contains(&call(lib, "run", lib, "Cache.get")), "{got:?}");
    assert!(
        got.contains(&call(lib, "run", lib, "Cache.peek")),
        "{got:?}"
    );
    for method in ["Store.save", "Store.size"] {
        assert!(
            got.contains(&call(lib, "run", "src/store.rs", method)),
            "{method}: {got:?}"
        );
    }
}

#[test]
fn a_bare_call_inside_an_impl_is_not_its_method() {
    let got = calls(&[(
        "src/lib.rs",
        "fn helper() -> u32 {
    0
}
pub struct A;
impl A {
    fn helper(&self) -> u32 {
        helper()
    }
}
",
    )]);
    assert!(
        got.contains(&call("src/lib.rs", "A.helper", "src/lib.rs", "helper")),
        "{got:?}"
    );
    assert!(!got.iter().any(|c| c[3] == "A.helper"), "{got:?}");
}

#[test]
fn a_receiver_bound_twice_or_untyped_is_not_trusted() {
    let got = calls(&[(
        "src/lib.rs",
        "pub struct A;
impl A {
    pub fn new() -> Self {
        A
    }
    pub fn go(&self) {}
}
pub fn f(x: Vec<A>) {
    let a = A::new();
    let a = other();
    a.go();
    x.go();
    let y = make();
    y.go();
}
",
    )]);
    assert!(
        !got.iter().any(|c| c[1] == "f" && c[3] == "A.go"),
        "{got:?}"
    );
}

#[test]
fn same_named_methods_of_two_types_stay_apart() {
    let got = calls(&[(
        "src/lib.rs",
        "pub struct A;
pub struct B;
impl A {
    pub fn new() -> Self {
        A
    }
}
impl B {
    pub fn new() -> Self {
        B
    }
}
pub fn f() {
    A::new();
    B::new();
}
",
    )]);
    assert!(
        got.contains(&call("src/lib.rs", "f", "src/lib.rs", "A.new")),
        "{got:?}"
    );
    assert!(
        got.contains(&call("src/lib.rs", "f", "src/lib.rs", "B.new")),
        "{got:?}"
    );
}
