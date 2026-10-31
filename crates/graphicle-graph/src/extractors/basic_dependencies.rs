//! Bounded, source-only dependency declarations. Never execute shell commands,
//! evaluate HCL, resolve host paths, or read outside the supplied source bytes.
use crate::Result;
use serde_json::{Value, json};
use tree_sitter::Node;

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (0..node.named_child_count()).filter_map(move |i| node.named_child(super::base::ts_index(i)?))
}

/// Returns literal path candidates, not proof that a shell command executes or
/// that a Terraform module is installed. Nonliteral inputs stay unresolved.
pub fn extract(language: &str, source: &str) -> Result<Value> {
    if source.len() > 4 * 1024 * 1024 {
        return Ok(json!({"dependencies":[],"issues":["source_size_limit"]}));
    }
    if language == "terraform_json" {
        let Ok(document) = serde_json::from_str::<Value>(source) else {
            return Ok(json!({"dependencies":[],"issues":["syntax_errors"]}));
        };
        let modules = document["module"].as_object();
        let dependencies: Vec<_> = modules.into_iter().flat_map(|m| m.values()).take(4096).map(|module| {
            let literal = module["source"].as_str().filter(|s| !s.contains(['$', '%']));
            // serde_json does not expose member byte spans. Cite the containing
            // file rather than inventing a line/offset for this declaration.
            json!({"kind":"module_source","path":literal,"line":1,"byte_start":0,"byte_end":source.len(),"dynamic":literal.is_none()})
        }).collect();
        let issues = if modules.is_some_and(|m| m.len() > 4096) {
            vec!["dependency_analysis_limit"]
        } else {
            vec![]
        };
        return Ok(json!({"dependencies":dependencies,"issues":issues}));
    }
    let grammar = match language {
        "bash" => tree_sitter_bash::LANGUAGE.into(),
        "terraform" => graphicle_tree_sitter_hcl::LANGUAGE.into(),
        _ => return Err("Unsupported basic dependency grammar".into()),
    };
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar)?;
    let tree = parser
        .parse(source, None)
        .ok_or("Basic dependency parse failed")?;
    let mut issues = vec![];
    if tree.root_node().has_error() {
        issues.push("syntax_errors");
    }
    let mut dependencies = vec![];
    let mut pending = vec![tree.root_node()];
    let mut visited = 0;
    while let Some(node) = pending.pop() {
        visited += 1;
        if visited > 100_000 || dependencies.len() >= 4096 {
            issues.push("dependency_analysis_limit");
            break;
        }
        let candidate = if language == "bash" && node.kind() == "command" {
            let command = node.child_by_field_name("name").map(|n| text(n, source));
            if matches!(command, Some("source" | "." | "bash" | "sh")) {
                node.child_by_field_name("argument")
                    .map(|n| (n, "shell_path"))
            } else {
                None
            }
        } else if language == "terraform"
            && node.kind() == "block"
            && children(node)
                .next()
                .is_some_and(|n| text(n, source) == "module")
        {
            children(node)
                .find(|n| n.kind() == "body")
                .and_then(|body| {
                    children(body)
                        .find(|attr| {
                            attr.kind() == "attribute"
                                && attr
                                    .named_child(0)
                                    .is_some_and(|n| text(n, source) == "source")
                        })
                        .and_then(|attr| children(attr).last())
                        .map(|n| (n, "module_source"))
                })
        } else {
            None
        };
        if let Some((value, kind)) = candidate {
            let raw = text(value, source);
            let literal = if language == "terraform" {
                serde_json::from_str::<String>(raw)
                    .ok()
                    .filter(|v| !v.contains(['$', '%']))
            } else if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
                Some(raw[1..raw.len() - 1].to_owned())
            } else {
                let unquoted = raw
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(raw);
                (!unquoted.is_empty()
                    && !unquoted.contains(['$', '`', '\\', '*', '?', '[', ' ', '\n'])
                    && !unquoted.starts_with('-'))
                .then(|| unquoted.to_owned())
            };
            dependencies.push(json!({"kind":kind,"path":literal,"line":value.start_position().row+1,
                "byte_start":value.start_byte(),"byte_end":value.end_byte(),"dynamic":literal.is_none()}));
        }
        pending.extend(children(node));
    }
    // Syntax errors can change command/block boundaries. Do not publish targets.
    if tree.root_node().has_error() {
        dependencies.clear();
    }
    Ok(json!({"dependencies":dependencies,"issues":issues}))
}
