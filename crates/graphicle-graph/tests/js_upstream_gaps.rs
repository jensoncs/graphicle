// Adapted from Graphify 0.9.98-0.9.103 (fe55ad7c, 69954b53), Apache-2.0;
// see NOTICE.md.
//! JS/TS import fixes of Graphify 0.9.98-0.9.103 that the precise pass does
//! not already cover: a relative specifier with a `?query` or `#fragment`
//! resolves to the real file, and a TypeScript import type
//! (`import('m').T`) is not a runtime dependency.
use graphicle::api::{ExtractOptions, extract};
use serde_json::Value;

struct Graph {
    root: String,
    nodes: Vec<Value>,
    edges: Vec<Value>,
}

impl Graph {
    fn name(&self, id: &Value) -> String {
        self.nodes
            .iter()
            .find(|n| n["id"] == *id)
            .map(|n| {
                let file = n["source_file"].as_str().unwrap_or("");
                let file = file
                    .strip_prefix(&self.root)
                    .unwrap_or(file)
                    .trim_start_matches('/');
                format!("{file}:{}", n["label"].as_str().unwrap_or(""))
            })
            .unwrap_or_else(|| format!("?:{}", id.as_str().unwrap_or("")))
    }
    /// `(source, target, relation)` of every edge, by file and label.
    fn triples(&self) -> Vec<(String, String, String)> {
        self.edges
            .iter()
            .map(|e| {
                (
                    self.name(&e["source"]),
                    self.name(&e["target"]),
                    e["relation"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }
    fn has(&self, source: &str, target: &str, relation: &str) -> bool {
        self.triples()
            .iter()
            .any(|(s, t, r)| s == source && t == target && r == relation)
    }
}

fn graph(files: &[(&str, &[u8])], precise: bool) -> Graph {
    graphicle::rcfile::set_language_overrides(None);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
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
            cache_root: Some(root.join("graphify-out")),
            parallel: false,
            precise,
            ..Default::default()
        },
    )
    .unwrap();
    Graph {
        root: root.to_string_lossy().into_owned(),
        nodes: result["nodes"].as_array().unwrap().clone(),
        edges: result["edges"].as_array().unwrap().clone(),
    }
}

// -- `?query` / `#fragment` specifiers (0.9.98, #4860) --------------------

#[test]
fn a_query_string_import_resolves_to_the_real_file_with_no_phantom() {
    for precise in [false, true] {
        let g = graph(
            &[
                ("lib/foo.js", b"export function foo() {}\n"),
                (
                    "page.js",
                    b"import { foo } from './lib/foo.js?v=134';\nfunction run() { foo(); }\n",
                ),
            ],
            precise,
        );
        let t = g.triples();
        assert!(
            g.has("page.js:page.js", "lib/foo.js:foo.js", "imports_from"),
            "{t:?}"
        );
        assert!(
            g.has("page.js:page.js", "lib/foo.js:foo()", "imports"),
            "{t:?}"
        );
        assert!(g.has("page.js:run()", "lib/foo.js:foo()", "calls"), "{t:?}");
        assert!(
            !t.iter().any(|(_, target, _)| target.contains("_v_134")),
            "{t:?}"
        );
    }
}

#[test]
fn a_fragment_and_a_query_with_a_fragment_resolve_too() {
    for spec in ["./lib/foo.js#frag", "./lib/foo.js?v=1#frag"] {
        let page = format!("import {{ foo }} from '{spec}';\n");
        let g = graph(
            &[
                ("lib/foo.js", b"export function foo() {}\n"),
                ("page.js", page.as_bytes()),
            ],
            false,
        );
        let t = g.triples();
        assert!(
            g.has("page.js:page.js", "lib/foo.js:foo.js", "imports_from"),
            "{spec}: {t:?}"
        );
    }
}

#[test]
fn a_query_string_does_not_affect_an_external_module() {
    let g = graph(&[("page.js", b"import _ from 'lodash-es';\n")], false);
    let t = g.triples();
    assert!(
        t.iter().any(|(s, target, r)| s == "page.js:page.js"
            && r == "imports_from"
            && target.contains("lodash")),
        "{t:?}"
    );
}

// -- TypeScript import types (0.9.103, #4992) -----------------------------

const TYPE_IMPORTS: &[&str] = &[
    "export function accept(page: import('@playwright/test').Page): void {}",
    "function f(): import('@playwright/test').Page { throw 1; }",
    "type Page = import('@playwright/test').Page;",
    "type Module = typeof import('@playwright/test');",
    "interface Options { page: import('@playwright/test').Page }",
    "function f<T extends import('@playwright/test').Page>() {}",
    "function f<T = import('@playwright/test').Page>() {}",
    "function f() { const page: import('@playwright/test').Page = undefined; }",
    "f<import('@playwright/test').Page>();\nfunction after() {}",
    "f<typeof import('@playwright/test')>();\nfunction after() {}",
];

/// Any edge to the imported package, or a node standing in for it.
fn reaches_playwright(g: &Graph) -> bool {
    g.edges.iter().any(|e| {
        e["target"]
            .as_str()
            .is_some_and(|t| t.contains("playwright"))
            && matches!(
                e["relation"].as_str(),
                Some("dynamic_import" | "imports_from")
            )
    }) || g.nodes.iter().any(|n| {
        n["source_file"]
            .as_str()
            .is_some_and(|s| s.contains("@playwright/test"))
    })
}

#[test]
fn a_type_import_is_not_a_runtime_dependency() {
    for suffix in ["ts", "tsx", "mts", "cts"] {
        for source in TYPE_IMPORTS {
            let name = format!("example.{suffix}");
            let g = graph(&[(name.as_str(), source.as_bytes())], false);
            assert!(
                !reaches_playwright(&g),
                "{suffix}: {source}: {:?}",
                g.triples()
            );
            if source.contains("function after") {
                assert!(
                    g.nodes.iter().any(|n| n["label"] == "after()"),
                    "{suffix}: {source}: {:?}",
                    g.nodes
                );
            }
        }
    }
}

#[test]
fn a_runtime_import_survives_a_type_import_of_the_same_package() {
    for suffix in ["ts", "tsx", "mts", "cts"] {
        for prefix in ["", "// caf\u{e9}\r\n", "\u{feff}// caf\u{e9}\r\n"] {
            let source = format!(
                "{prefix}export function load(page: import('@playwright/test').Page) {{\r\n  return import('@playwright/test');\r\n}}\r\n"
            );
            let name = format!("example.{suffix}");
            let g = graph(&[(name.as_str(), source.as_bytes())], false);
            assert!(
                reaches_playwright(&g),
                "{suffix} {prefix:?}: {:?}",
                g.triples()
            );
        }
    }
}

#[test]
fn a_type_import_is_dropped_after_crlf_lines_and_a_bom() {
    for prefix in ["// caf\u{e9}\r\n\r\n", "\u{feff}// caf\u{e9}\r\n"] {
        let source = format!(
            "{prefix}type P = import('@playwright/test').Page;\r\nconst lazy = import('runtime-pkg');\r\n"
        );
        let g = graph(&[("example.ts", source.as_bytes())], false);
        assert!(!reaches_playwright(&g), "{prefix:?}: {:?}", g.triples());
        assert!(
            g.edges
                .iter()
                .any(|e| e["target"].as_str().is_some_and(|t| t.contains("runtime"))),
            "{prefix:?}: {:?}",
            g.triples()
        );
    }
}

#[test]
fn a_runtime_comparison_import_is_not_a_type_import() {
    for suffix in ["js", "ts", "tsx"] {
        let name = format!("example.{suffix}");
        let g = graph(
            &[(
                name.as_str(),
                b"const x = a < b, import(\"runtime\") > (d);\n",
            )],
            false,
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e["target"].as_str().is_some_and(|t| t.contains("runtime"))),
            "{suffix}: {:?}",
            g.triples()
        );
    }
}

// -- `class A extends X` with `const X = require('./m')` (0.9.102, #4688) --

impl Graph {
    /// `(target label, target file)` of each `relation` edge out of `source`.
    fn out(&self, source_label: &str, relation: &str) -> Vec<(String, String)> {
        let mut v = self
            .edges
            .iter()
            .filter(|e| e["relation"] == relation)
            .filter_map(|e| {
                let s = self.nodes.iter().find(|n| n["id"] == e["source"])?;
                let t = self.nodes.iter().find(|n| n["id"] == e["target"])?;
                (s["label"] == source_label).then(|| {
                    let file = t["source_file"].as_str().unwrap_or("");
                    (
                        t["label"].as_str().unwrap_or("").to_string(),
                        file.rsplit('/').next().unwrap_or("").to_string(),
                    )
                })
            })
            .collect::<Vec<_>>();
        v.sort();
        v
    }
    fn has_node(&self, file: &str, label: &str) -> bool {
        self.nodes.iter().any(|n| {
            n["label"] == label
                && n["source_file"]
                    .as_str()
                    .is_some_and(|f| f.ends_with(&format!("/{file}")))
        })
    }
}

const GEN: &[u8] = b"class Generator { run() {} }\nmodule.exports = Generator;\n";

fn pair(label: &str, file: &str) -> Vec<(String, String)> {
    vec![(label.into(), file.into())]
}

#[test]
fn a_required_class_is_inherited_from() {
    for precise in [false, true] {
        let g = graph(
            &[
                ("gen.js", GEN),
                (
                    "use.js",
                    b"const Generator = require('./gen');\nclass A extends Generator { go() {} }\n",
                ),
            ],
            precise,
        );
        assert!(
            !g.has_node("use.js", "Generator"),
            "{precise}: {:?}",
            g.nodes
        );
        assert_eq!(
            g.out("A", "inherits"),
            pair("Generator", "gen.js"),
            "{precise}: {:?}",
            g.triples()
        );
    }
}

#[test]
fn let_var_ts_and_import_equals_requires_are_inherited_from() {
    for (name, head) in [
        ("use_let.js", "let Generator = require('./gen');\n"),
        ("use_var.js", "var Generator = require('./gen');\n"),
        ("use.ts", "const Generator = require('./gen');\n"),
        ("use_eq.ts", "import Generator = require('./gen');\n"),
    ] {
        let source = format!("{head}class A extends Generator {{ go() {{}} }}\n");
        let g = graph(&[("gen.js", GEN), (name, source.as_bytes())], false);
        assert_eq!(
            g.out("A", "inherits"),
            pair("Generator", "gen.js"),
            "{name}: {:?}",
            g.triples()
        );
    }
}

#[test]
fn an_esm_default_import_is_still_inherited_from() {
    let g = graph(
        &[
            (
                "gen.js",
                b"class Generator { run() {} }\nexport default Generator;\n",
            ),
            (
                "use.js",
                b"import Generator from './gen';\nclass A extends Generator { go() {} }\n",
            ),
        ],
        false,
    );
    assert_eq!(
        g.out("A", "inherits"),
        pair("Generator", "gen.js"),
        "{:?}",
        g.triples()
    );
}

#[test]
fn a_renamed_require_follows_the_default_export() {
    let g = graph(
        &[
            ("gen.js", GEN),
            (
                "use.js",
                b"const Foo = require('./gen');\nclass A extends Foo { go() {} }\n",
            ),
        ],
        false,
    );
    assert!(!g.has_node("use.js", "Foo"), "{:?}", g.nodes);
    assert_eq!(
        g.out("A", "inherits"),
        pair("Generator", "gen.js"),
        "{:?}",
        g.triples()
    );
}

#[test]
fn new_on_a_required_constructor_still_resolves() {
    let g = graph(
        &[
            ("logger.js", b"function Logger(p) { this.p = p }\nmodule.exports = Logger\n"),
            ("use.js", b"const Logger = require('./logger')\nfunction Watcher() { return new Logger('w') }\n"),
        ],
        false,
    );
    assert!(!g.has_node("use.js", "Logger"), "{:?}", g.nodes);
    assert_eq!(
        g.out("Watcher()", "calls"),
        pair("Logger()", "logger.js"),
        "{:?}",
        g.triples()
    );
}

#[test]
fn bare_exports_assignment_is_not_a_default_export() {
    let g = graph(
        &[
            (
                "lib.js",
                b"class Decoy {}\nclass Real {}\nexports = Decoy;\nmodule.exports = Real;\n",
            ),
            (
                "use.js",
                b"const X = require('./lib');\nclass A extends X {}\n",
            ),
        ],
        false,
    );
    assert_eq!(
        g.out("A", "inherits"),
        pair("Real", "lib.js"),
        "{:?}",
        g.triples()
    );
    let g = graph(
        &[
            ("lib.js", b"class Decoy {}\nexports = Decoy;\n"),
            (
                "use.js",
                b"const X = require('./lib');\nclass A extends X {}\n",
            ),
        ],
        false,
    );
    assert_eq!(g.out("A", "inherits"), vec![], "{:?}", g.triples());
    assert!(!g.has_node("use.js", "X"), "{:?}", g.nodes);
}

#[test]
fn a_chained_module_exports_uses_the_final_value() {
    let g = graph(
        &[
            (
                "lib.js",
                b"class Foo { run() {} }\nmodule.exports = exports = Foo;\n",
            ),
            (
                "use.js",
                b"const X = require('./lib');\nclass A extends X {}\n",
            ),
        ],
        false,
    );
    assert_eq!(
        g.out("A", "inherits"),
        pair("Foo", "lib.js"),
        "{:?}",
        g.triples()
    );
}
