use graphicle::extractors::go_types;
use serde_json::{Value, json};
fn analyze(source: &str, mode: &str, needle: &str, new_name: &str) -> Value {
    go_types::semantic(&json!({"mode":mode,"files":{"main.go":source},"file":"main.go","offset":source.find(needle).unwrap(),"new_name":new_name,"goos":"linux","goarch":"amd64"})).unwrap()
}
#[test]
fn early_return_guards_and_overwritten_values_have_exact_statement_paths() {
    let source = "package app\nfunc F(x int, allowed bool) int { if !allowed { return 0 }; y:=x+1; y=3; return y }\n";
    let r = analyze(source, "flow", "func F", "");
    assert_eq!(r["coverage"]["complete"], true, "{r}");
    let nodes = r["statements"].as_array().unwrap();
    let node = |text: &str| {
        nodes
            .iter()
            .find(|n| n["byte_start"] == source.find(text).unwrap())
            .unwrap()["id"]
            .clone()
    };
    let ret = node("return y");
    let overwrite = node("y=3");
    let initial = node("y:=");
    let deps: Vec<_> = r["data_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to"] == ret)
        .collect();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0]["from"], overwrite);
    assert_ne!(deps[0]["from"], initial);
    assert!(
        r["gates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["statement"] == ret
                && g["condition"] == node("!allowed")
                && g["outcome"] == "false")
    );
}
#[test]
fn branch_joins_keep_all_reaching_definitions_and_shadowed_variables_stay_distinct() {
    let source = "package app\nfunc F(x int, yes bool) int { y:=x; if yes { y=1 } else { y=2 }; { y:=3; _=y }; return y }\n";
    let r = analyze(source, "flow", "func F", "");
    let nodes = r["statements"].as_array().unwrap();
    let ret = nodes
        .iter()
        .find(|n| n["byte_start"] == source.find("return y").unwrap())
        .unwrap();
    let edges: Vec<_> = r["data_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to"] == ret["id"])
        .collect();
    assert_eq!(edges.len(), 2, "{r}");
    for edge in edges {
        let from = &nodes[edge["from"].as_u64().unwrap() as usize];
        assert!(
            [source.find("y=1").unwrap(), source.find("y=2").unwrap()]
                .contains(&(from["byte_start"].as_u64().unwrap() as usize))
        );
    }
}
#[test]
fn unsupported_control_short_circuit_and_missing_dependencies_are_visible() {
    for (source, issue) in [
        (
            "package app\nfunc F(x int) int { switch x {case 1: x++}; return x }",
            "unsupported_control_statement",
        ),
        (
            "package app\nfunc F(x bool,y bool) bool { if x && y { return true }; return false }",
            "short_circuit_expression_not_expanded",
        ),
        (
            "package app\nimport missing \"example.invalid/absent\"\nfunc F(x int) int { return missing.Run(x) }",
            "type_information_incomplete",
        ),
    ] {
        let r = analyze(source, "flow", "func F", "");
        assert_eq!(r["coverage"]["complete"], false, "{r}");
        assert!(
            r["coverage"]["issues"]
                .as_array()
                .unwrap()
                .contains(&json!(issue))
        );
    }
}
#[test]
fn flow_bounds_and_build_selection_are_explicit() {
    let source = format!("package app\nfunc F() {{ {} }}", "x:=1; _=x; ".repeat(110));
    let r = analyze(&source, "flow", "func F", "");
    assert_eq!(r["error"], "statement_analysis_limit");
    let source = "//go:build windows\n\npackage app\nfunc F(x int) int { return x }";
    let r = analyze(source, "flow", "func F", "");
    assert_eq!(r["error"], "target outside selected build");
}
#[test]
fn rename_preserves_shadowing_and_rejects_capture_even_when_after_code_typechecks() {
    let source = "package app\nfunc F(x int) int { { x:=2; _=x }; return x }";
    let r = analyze(source, "rename", "x int", "value");
    assert_eq!(r["validated"], true, "{r}");
    assert_eq!(r["edits"].as_array().unwrap().len(), 2);
    let source = "package app\nvar value=3\nfunc F(x int) int { return x + value }";
    let r = analyze(source, "rename", "x int", "value");
    assert_eq!(r["validated"], false);
    assert_eq!(r["error"], "rename_changes_a_binding");
}

#[test]
fn loops_breaks_and_continues_keep_possible_reaching_definitions() {
    let source = "package app\nfunc F(x int) int { y:=0; for i:=0; i<x; i++ { if i==2 { continue }; y=i; if y>4 { break } }; return y }";
    let r = analyze(source, "flow", "func F", "");
    assert_eq!(r["coverage"]["complete"], true, "{r}");
    let nodes = r["statements"].as_array().unwrap();
    let ret = nodes
        .iter()
        .find(|n| n["byte_start"] == source.find("return y").unwrap())
        .unwrap();
    let writes: Vec<_> = r["data_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to"] == ret["id"])
        .collect();
    assert_eq!(writes.len(), 2, "{r}");
    assert!(nodes.iter().any(|n| n["kind"] == "break"));
    assert!(nodes.iter().any(|n| n["kind"] == "continue"));
    let nonterminating = analyze("package app\nfunc F() { for {} }", "flow", "func F", "");
    assert_eq!(nonterminating["coverage"]["complete"], false);
    assert_eq!(nonterminating["gates"], json!([]));
}

#[test]
fn empty_ranges_preserve_preloop_values() {
    let source = "package app\nfunc F(items []int) int { value:=7; for _,value=range items { _=value }; return value }";
    let r = analyze(source, "flow", "func F", "");
    let nodes = r["statements"].as_array().unwrap();
    let ret = nodes
        .iter()
        .find(|n| n["byte_start"] == source.find("return value").unwrap())
        .unwrap();
    let writes: Vec<_> = r["data_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to"] == ret["id"])
        .collect();
    assert_eq!(writes.len(), 2, "{r}");
}

#[test]
fn named_return_values_and_void_guards_are_modeled_without_inventing_unknown_values() {
    let source = "package app\nfunc F(x int) (result int) { result=x+1; return }";
    let r = analyze(source, "flow", "func F", "");
    assert_eq!(r["coverage"]["complete"], true, "{r}");
    let ret = r["statements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "return")
        .unwrap();
    assert_eq!(ret["reads"][0]["name"], "result");
    assert!(
        r["data_edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["to"] == ret["id"] && e["name"] == "result")
    );
    let r = analyze(
        "package app\nfunc F(ok bool) { if !ok { return }; x:=1; _=x }",
        "flow",
        "func F",
        "",
    );
    assert_eq!(r["coverage"]["complete"], true, "{r}");
}

#[test]
fn range_inputs_are_evaluated_once_and_grouped_declarations_flow_in_order() {
    let source = "package app\nfunc F(items []int) int { var (a=1; b=a); for _,v:=range items { items=nil; b+=v }; return b }";
    let r = analyze(source, "flow", "func F", "");
    assert_eq!(r["coverage"]["complete"], true, "{r}");
    let nodes = r["statements"].as_array().unwrap();
    let a = nodes
        .iter()
        .find(|n| n["byte_start"] == source.find("a=1").unwrap())
        .unwrap();
    let b = nodes
        .iter()
        .find(|n| n["byte_start"] == source.find("b=a").unwrap())
        .unwrap();
    assert!(
        r["data_edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["from"] == a["id"] && e["to"] == b["id"])
    );
    let input = nodes.iter().find(|n| n["kind"] == "range_input").unwrap();
    let origins: Vec<_> = r["data_edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["to"] == input["id"])
        .collect();
    assert_eq!(origins.len(), 1, "{r}");
    assert_eq!(origins[0]["from"], r["entry"]);
    let r = analyze(
        "package app\nfunc F(p *int) int { return *p }",
        "flow",
        "func F",
        "",
    );
    assert_eq!(r["coverage"]["complete"], false);
    assert!(
        r["coverage"]["issues"]
            .as_array()
            .unwrap()
            .contains(&json!("heap_and_external_effects_not_followed"))
    );
}

#[test]
fn cross_target_import_metadata_is_not_mistaken_for_host_bindings() {
    let source =
        "package app\nimport \"strings\"\nfunc F(x string) string { return strings.TrimSpace(x) }";
    // wasm cannot be the target of the native helper process used in these tests.
    let r = go_types::semantic(&json!({"mode":"rename","files":{"main.go":source},"file":"main.go","offset":source.find("x string").unwrap(),"new_name":"value","goos":"js","goarch":"wasm"})).unwrap();
    assert_eq!(r["validated"], false, "{r}");
    assert_eq!(r["error"], "type_information_incomplete");
}
