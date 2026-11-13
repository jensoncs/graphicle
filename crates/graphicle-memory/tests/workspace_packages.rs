//! A bare import of a workspace member (`@scope/pkg`, `@scope/pkg/sub`)
//! resolves to that member's source entry in the in-memory manifest, as the
//! graph engine's on-disk workspace resolver does.
use graphicle_memory::structure::{analyze, graph};
use serde_json::{Map, Value};

fn build(files: &[(&str, &str)]) -> Value {
    let mut analyses = Map::new();
    for (path, content) in files {
        analyses.insert((*path).to_string(), analyze(path, content).unwrap());
    }
    graph(&Value::Object(analyses)).unwrap()
}

fn targets(raw: &Value, module: &str) -> Vec<String> {
    raw["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["module"] == module)
        .map(|e| e["target"].as_str().unwrap().to_string())
        .collect()
}

fn unresolved(raw: &Value, module: &str) -> bool {
    raw["unresolved"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["module"] == module)
}

const CORE: &str = "export function price() { return 1; }\n";
const APP: &str =
    "import { price } from '@internal/core';\nexport function run() { return price(); }\n";

#[test]
fn a_yarn_workspace_member_resolves_to_its_main_and_carries_calls() {
    let raw = build(&[
        (
            "package.json",
            r#"{"workspaces": {"packages": ["packages/*", "plugins/*"]}}"#,
        ),
        (
            "plugins/core/package.json",
            r#"{"name": "@internal/core", "main": "src/index.ts", "types": "src/index.ts"}"#,
        ),
        ("plugins/core/src/index.ts", CORE),
        ("plugins/app/package.json", r#"{"name": "@internal/app"}"#),
        ("plugins/app/src/a.ts", APP),
    ]);
    assert_eq!(
        targets(&raw, "@internal/core"),
        ["plugins/core/src/index.ts"]
    );
    assert!(!unresolved(&raw, "@internal/core"));
    let calls: Vec<_> = raw["symbol_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["path"].as_str().unwrap(),
                c["callee_path"].as_str().unwrap(),
                c["callee"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        calls,
        [("plugins/app/src/a.ts", "plugins/core/src/index.ts", "price")]
    );
}

#[test]
fn a_subpath_and_a_pnpm_workspace_resolve() {
    let raw = build(&[
        ("pnpm-workspace.yaml", "packages:\n  - 'libs/*'\n"),
        ("libs/core/package.json", r#"{"name": "@internal/core"}"#),
        ("libs/core/src/index.ts", CORE),
        ("libs/core/lib/util.ts", CORE),
        (
            "libs/app/src/a.ts",
            "import { price } from '@internal/core/lib/util';\nimport '@internal/core';\n",
        ),
    ]);
    assert_eq!(
        targets(&raw, "@internal/core/lib/util"),
        ["libs/core/lib/util.ts"]
    );
    assert_eq!(targets(&raw, "@internal/core"), ["libs/core/src/index.ts"]);
}

#[test]
fn exports_into_build_output_fall_back_to_the_source_twin() {
    let raw = build(&[
        ("package.json", r#"{"workspaces": ["packages/*"]}"#),
        (
            "packages/core/package.json",
            r#"{"name": "@internal/core", "exports": {".": {"import": "./dist/index.js"}}}"#,
        ),
        ("packages/core/src/index.ts", CORE),
        ("packages/app/src/a.ts", APP),
    ]);
    assert_eq!(
        targets(&raw, "@internal/core"),
        ["packages/core/src/index.ts"]
    );
}

#[test]
fn a_package_outside_the_workspace_or_with_a_shared_name_stays_unresolved() {
    let raw = build(&[
        ("package.json", r#"{"workspaces": ["packages/*"]}"#),
        (
            "fixtures/core/package.json",
            r#"{"name": "@internal/core", "main": "index.ts"}"#,
        ),
        ("fixtures/core/index.ts", CORE),
        (
            "packages/a/package.json",
            r#"{"name": "@internal/twin", "main": "index.ts"}"#,
        ),
        ("packages/a/index.ts", CORE),
        (
            "packages/b/package.json",
            r#"{"name": "@internal/twin", "main": "index.ts"}"#,
        ),
        ("packages/b/index.ts", CORE),
        (
            "packages/app/src/a.ts",
            "import '@internal/core';\nimport '@internal/twin';\n",
        ),
    ]);
    assert!(targets(&raw, "@internal/core").is_empty());
    assert!(unresolved(&raw, "@internal/core"));
    assert!(targets(&raw, "@internal/twin").is_empty());
    assert!(unresolved(&raw, "@internal/twin"));
}

#[test]
fn the_nearest_workspace_root_and_tsconfig_govern_an_import() {
    let raw = build(&[
        // pnpm's file defines a root that also has package.json workspaces.
        ("package.json", r#"{"workspaces": ["outer/*"]}"#),
        (
            "pnpm-workspace.yaml",
            "packages:\n  - 'apps/*'\n  - 'apps/inner/libs/*'\n",
        ),
        (
            "outer/core/package.json",
            r#"{"name": "@internal/core", "main": "index.ts"}"#,
        ),
        ("outer/core/index.ts", CORE),
        (
            "apps/a/package.json",
            r#"{"name": "@internal/core", "main": "index.ts"}"#,
        ),
        ("apps/a/index.ts", CORE),
        // A nested root lists its own member under the same name.
        (
            "apps/inner/package.json",
            r#"{"name": "inner", "workspaces": ["libs/*"]}"#,
        ),
        (
            "apps/inner/libs/core/package.json",
            r#"{"name": "@internal/core", "main": "index.ts"}"#,
        ),
        ("apps/inner/libs/core/index.ts", CORE),
        ("apps/inner/libs/app/src/a.ts", APP),
        ("apps/b/src/a.ts", "import '@internal/core';\n"),
        // The nearest tsconfig maps `@x/*`; the outer one would not.
        (
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@x/*": ["nowhere/*"]}}}"#,
        ),
        (
            "apps/b/tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@x/*": ["./src/*"]}}}"#,
        ),
        ("apps/b/src/x.ts", CORE),
        ("apps/b/src/y.ts", "import '@x/x';\n"),
    ]);
    let edges = |source: &str, module: &str| -> Vec<String> {
        raw["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["source"] == source && e["module"] == module)
            .map(|e| e["target"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        edges("apps/inner/libs/app/src/a.ts", "@internal/core"),
        ["apps/inner/libs/core/index.ts"]
    );
    // In pnpm's root, apps/a and apps/inner/libs/core share the name.
    assert!(edges("apps/b/src/a.ts", "@internal/core").is_empty());
    assert_eq!(edges("apps/b/src/y.ts", "@x/x"), ["apps/b/src/x.ts"]);
}
