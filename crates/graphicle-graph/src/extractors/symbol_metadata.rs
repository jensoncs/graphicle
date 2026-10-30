//! Structured declaration facts from the extractor's existing syntax tree.
//! Types/names are source spellings, not compiler equivalence or runtime values.
use serde_json::{Value, json};
use tree_sitter::Node;

pub fn supported(language: &str) -> bool {
    matches!(
        language,
        "python" | "javascript" | "typescript" | "go" | "java"
    )
}
fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}
fn text(n: Node<'_>, source: &[u8]) -> String {
    n.utf8_text(source).unwrap_or("").to_owned()
}
/// Whitespace outside strings is not a type identity; retain exact type spelling
/// except surrounding annotation punctuation. No alias/assignability inference.
fn spelling(n: Node<'_>, source: &[u8]) -> String {
    text(n, source)
        .trim()
        .trim_start_matches(':')
        .trim()
        .to_owned()
}
pub fn declaration(n: Node<'_>, source: &[u8], language: &str) -> Option<Value> {
    if !supported(language) {
        return None;
    }
    let mut parameters = Vec::new();
    let mut issues = Vec::new();
    if n.has_error() {
        issues.push("syntax_error");
    }
    let list = n.child_by_field_name("parameters");
    let params = list
        .map(children)
        .unwrap_or_else(|| n.child_by_field_name("parameter").into_iter().collect());
    for p in params {
        if parameters.len() >= 128 {
            issues.push("parameter_limit");
            break;
        }
        if matches!(
            p.kind(),
            "positional_separator" | "keyword_separator" | "comment"
        ) {
            continue;
        }
        let mut ty = p.child_by_field_name("type").map(|t| spelling(t, source));
        if ty.as_ref().is_some_and(|t| t.len() > 256) {
            issues.push("metadata_field_limit");
            ty = None;
        }
        let names: Vec<_> = if language == "go" {
            let mut cursor = p.walk();
            p.children_by_field_name("name", &mut cursor).collect()
        } else if p.kind() == "identifier" {
            vec![p]
        } else {
            p.child_by_field_name("name")
                .or_else(|| p.child_by_field_name("pattern"))
                .or_else(|| p.child_by_field_name("left"))
                .or_else(|| p.named_child(0))
                .into_iter()
                .collect()
        };
        if names.is_empty() && language == "go" {
            parameters.push(
                json!({"name":null,"type":ty,"byte_start":p.start_byte(),"byte_end":p.end_byte()}),
            );
        }
        for mut name in names {
            if parameters.len() >= 128 {
                issues.push("parameter_limit");
                break;
            }
            if matches!(
                name.kind(),
                "list_splat_pattern" | "dictionary_splat_pattern" | "rest_pattern"
            ) && let Some(inner) = name.named_child(0)
            {
                name = inner;
            }
            if !matches!(name.kind(), "identifier" | "field_identifier") {
                issues.push("unsupported_parameter_pattern");
                continue;
            }
            if name.end_byte() - name.start_byte() > 256 {
                issues.push("metadata_field_limit");
                continue;
            }
            parameters.push(json!({"name":text(name,source),"type":ty,"byte_start":p.start_byte(),"byte_end":p.end_byte()}));
        }
    }
    let mut decorators = vec![];
    let mut annotations = vec![];
    let header = n
        .parent()
        .filter(|p| p.kind() == "decorated_definition")
        .unwrap_or(n);
    let mut pending = children(header);
    let mut visited = 0;
    while let Some(child) = pending.pop() {
        visited += 1;
        if visited > 1024 {
            issues.push("annotation_limit");
            break;
        }
        if matches!(
            child.kind(),
            "decorator" | "annotation" | "marker_annotation"
        ) {
            let raw = text(child, source);
            let name = raw
                .trim_start_matches('@')
                .split('(')
                .next()
                .unwrap_or("")
                .trim();
            if name.len() > 256 {
                issues.push("metadata_field_limit");
                continue;
            }
            let item =
                json!({"name":name,"byte_start":child.start_byte(),"byte_end":child.end_byte()});
            if language == "java" {
                annotations.push(item);
            } else {
                decorators.push(item);
            }
        } else if matches!(child.kind(), "modifiers") {
            pending.extend(children(child));
        }
    }
    issues.sort_unstable();
    issues.dedup();
    Some(
        json!({"version":1,"language":language,"basis":"syntax","complete":issues.is_empty() && !n.has_error(),"issues":issues,
        "byte_start":n.start_byte(),"byte_end":n.end_byte(),"line":n.start_position().row+1,"end_line":n.end_position().row+1,
        "parameters":parameters,"decorators":decorators,"annotations":annotations,
        "coverage":{"parameter_name":true,"parameter_type":language!="javascript","decorator":matches!(language,"python"|"javascript"|"typescript"),"annotation":language=="java"}}),
    )
}
/// Keep overloads distinct even when an older extractor shares their graph node.
pub fn attach(nodes: &mut [Value], id: &str, metadata: Option<Value>) {
    if let Some(metadata) = metadata
        && let Some(node) = nodes.iter_mut().find(|n| n["id"] == id)
    {
        let values = node
            .as_object_mut()
            .unwrap()
            .entry("symbol_metadata")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap();
        if !values.contains(&metadata) {
            values.push(metadata);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(source: &str, language: &str) -> Value {
        let grammar = match language {
            "python" => tree_sitter_python::LANGUAGE.into(),
            "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "javascript" => tree_sitter_javascript::LANGUAGE.into(),
            "go" => tree_sitter_go::LANGUAGE.into(),
            "java" => tree_sitter_java::LANGUAGE.into(),
            _ => unreachable!(),
        };
        let mut p = tree_sitter::Parser::new();
        p.set_language(&grammar).unwrap();
        let t = p.parse(source, None).unwrap();
        assert!(!t.root_node().has_error(), "{}", t.root_node().to_sexp());
        let mut pending = vec![t.root_node()];
        while let Some(n) = pending.pop() {
            if matches!(
                n.kind(),
                "function_definition" | "function_declaration" | "method_declaration"
            ) {
                return declaration(n, source.as_bytes(), language).unwrap();
            };
            pending.extend(children(n));
        }
        panic!("No declaration")
    }
    #[test]
    fn parameters_and_decorators_are_declarations_not_body_mentions() {
        let m = parse(
            "@route.post('/orders')\ndef submit(tenant: Tenant, retries: int = 2, *, callback=None):\n    unrelated = 'admin: Root'\n    return callback(tenant)\n",
            "python",
        );
        assert_eq!(m["parameters"][0]["name"], "tenant");
        assert_eq!(m["parameters"][0]["type"], "Tenant");
        assert_eq!(m["parameters"].as_array().unwrap().len(), 3);
        assert_eq!(m["decorators"][0]["name"], "route.post");
        assert_eq!(m["complete"], true);
    }
    #[test]
    fn grouped_go_parameters_and_java_annotations_are_exact() {
        let m = parse(
            "package p\nfunc F(left, right *Request, callback func(int) bool) {}",
            "go",
        );
        assert_eq!(m["parameters"].as_array().unwrap().len(), 3);
        assert_eq!(m["parameters"][1]["name"], "right");
        assert_eq!(m["parameters"][1]["type"], "*Request");
        let m = parse(
            "class C { @Override public void run(Request request) {} }",
            "java",
        );
        assert_eq!(m["annotations"][0]["name"], "Override");
        assert_eq!(m["parameters"][0]["type"], "Request");
    }
    #[test]
    fn typescript_reports_destructuring_as_partial_and_javascript_has_no_type_coverage() {
        let m = parse(
            "function F(tenant: Tenant, {unsafe}: Options) {}",
            "typescript",
        );
        assert_eq!(m["parameters"][0]["type"], "Tenant");
        assert_eq!(m["complete"], false);
        let m = parse("function F(tenant) {}", "javascript");
        assert_eq!(m["coverage"]["parameter_type"], false);
        assert!(!supported("bash"));
    }
}
