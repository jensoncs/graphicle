//! Calls through a method's own receiver (`self.m()`, `cls.m()`, `this.m()`)
//! bind to the enclosing class's method, and CommonJS `require` binds its
//! namespace or destructured names like an ES import.
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

#[test]
fn python_self_and_cls_calls_bind_to_the_enclosing_class() {
    let got = calls(&[(
        "svc.py",
        "class Svc:\n    def run(self):\n        self.load()\n        def inner():\n            self.load()\n        self.missing()\n\n    def load(self):\n        pass\n\n    @classmethod\n    def make(cls):\n        return cls.build()\n\n    @classmethod\n    def build(cls):\n        pass\n\n\nclass Other:\n    def load(self):\n        pass\n",
    )]);
    assert!(
        got.contains(&call("svc.py", "Svc.run", "svc.py", "Svc.load")),
        "{got:?}"
    );
    assert!(
        got.contains(&call("svc.py", "Svc.run.inner", "svc.py", "Svc.load")),
        "{got:?}"
    );
    assert!(
        got.contains(&call("svc.py", "Svc.make", "svc.py", "Svc.build")),
        "{got:?}"
    );
    assert!(!got.iter().any(|c| c[3] == "Other.load"), "{got:?}");
    assert!(!got.iter().any(|c| c[3].ends_with("missing")), "{got:?}");
}

#[test]
fn a_receiver_rebound_in_the_method_is_not_trusted() {
    let got = calls(&[(
        "svc.py",
        "class Svc:\n    def run(self):\n        self = Other()\n        self.load()\n\n    def load(self):\n        pass\n",
    )]);
    assert!(!got.iter().any(|c| c[3] == "Svc.load"), "{got:?}");
}

#[test]
fn javascript_this_calls_bind_to_the_enclosing_class() {
    let got = calls(&[(
        "repo.js",
        "class Repo {\n  find() {\n    return this.load();\n  }\n  load() {\n    return 1;\n  }\n}\nmodule.exports = Repo;\n",
    )]);
    assert!(
        got.contains(&call("repo.js", "Repo.find", "repo.js", "Repo.load")),
        "{got:?}"
    );
}

#[test]
fn commonjs_require_binds_its_namespace_and_destructured_names() {
    let got = calls(&[
        (
            "lib/math.js",
            "function add(a, b) {\n  return a + b;\n}\nconst sub = (a, b) => a - b;\nmodule.exports = { add, sub };\n",
        ),
        (
            "app.js",
            "const math = require('./lib/math');\nconst { sub: minus } = require('./lib/math');\nfunction main() {\n  math.add(1, 2);\n  minus(3, 1);\n}\n",
        ),
    ]);
    assert!(
        got.contains(&call("app.js", "main", "lib/math.js", "add")),
        "{got:?}"
    );
    assert!(
        got.contains(&call("app.js", "main", "lib/math.js", "sub")),
        "{got:?}"
    );
}

#[test]
fn a_reassigned_require_binding_is_not_trusted() {
    let got = calls(&[
        (
            "lib/math.js",
            "function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n",
        ),
        (
            "app.js",
            "let math = require('./lib/math');\nmath = makeOther();\nfunction main() {\n  math.add(1, 2);\n}\n",
        ),
    ]);
    assert!(!got.iter().any(|c| c[3] == "add"), "{got:?}");
}

#[test]
fn a_commonjs_default_exported_class_binds_construction_and_static_calls() {
    let got = calls(&[
        (
            "util/http.js",
            "class Http {\n  get(u) {\n    return u;\n  }\n  static make() {\n    return 1;\n  }\n}\nmodule.exports = Http;\n",
        ),
        (
            "di/index.js",
            "const Http = require('../util/http');\nfunction build() {\n  const http = new Http(1);\n  return Http.make();\n}\n",
        ),
    ]);
    assert!(
        got.contains(&call("di/index.js", "build", "util/http.js", "Http")),
        "{got:?}"
    );
    assert!(
        got.contains(&call("di/index.js", "build", "util/http.js", "Http.make")),
        "{got:?}"
    );
}

#[test]
fn a_commonjs_default_exported_function_binds_its_calls() {
    let got = calls(&[
        (
            "lib/send.js",
            "function send(x) {\n  return x;\n}\nmodule.exports = send;\n",
        ),
        (
            "app.js",
            "const deliver = require('./lib/send');\nfunction main() {\n  deliver(1);\n}\n",
        ),
    ]);
    assert!(
        got.contains(&call("app.js", "main", "lib/send.js", "send")),
        "{got:?}"
    );
}

#[test]
fn constructing_a_class_in_its_own_file_calls_it() {
    let got = calls(&[("a.js", "class A {}\nfunction f() {\n  return new A();\n}\n")]);
    assert!(got.contains(&call("a.js", "f", "a.js", "A")), "{got:?}");
}
