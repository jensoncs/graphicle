//! Bounded Python environment syntax using the already parsed source tree.
//! Only unconditional module imports establish bindings. Any rebinding of an
//! alias anywhere in the file withholds that alias; no execution/order inference.
use super::{Limits, assignment_names, pattern_names, value};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

#[derive(Clone, Copy, PartialEq)]
enum Binding {
    Os,
    Getenv,
    Environ,
    Mutation,
}
struct Import {
    kind: Binding,
    end: usize,
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn attribute<'a, 'b>(node: Node<'a>, raw: &'b [u8]) -> Option<(Node<'a>, &'b str)> {
    (node.kind() == "attribute").then_some(())?;
    Some((
        node.child_by_field_name("object")?,
        value(node.child_by_field_name("attribute")?, raw),
    ))
}
fn binding(
    node: Node<'_>,
    raw: &[u8],
    imports: &BTreeMap<String, Import>,
    unsafe_names: &BTreeSet<String>,
    at: usize,
) -> Option<Binding> {
    if node.kind() == "identifier" {
        let name = value(node, raw);
        let import = imports.get(name)?;
        return (!unsafe_names.contains(name) && at >= import.end).then_some(import.kind);
    }
    let (object, member) = attribute(node, raw)?;
    if object.kind() != "identifier" {
        return None;
    }
    if binding(object, raw, imports, unsafe_names, at)? != Binding::Os {
        return None;
    }
    match member {
        "getenv" => Some(Binding::Getenv),
        "environ" => Some(Binding::Environ),
        "putenv" | "unsetenv" => Some(Binding::Mutation),
        _ => None,
    }
}
fn key(node: Node<'_>, raw: &[u8]) -> Option<String> {
    // Interpolation, escapes and concatenation remain dynamic. Never evaluate Python.
    if node.kind() != "string" {
        return None;
    }
    let text = value(node, raw);
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || text.as_bytes().last() != Some(&quote) || text.len() < 2 {
        return None;
    }
    let key = &text[1..text.len() - 1];
    graphicle::extractors::environment::key(key).then(|| key.to_owned())
}
fn written(node: Node<'_>) -> bool {
    let mut current = node;
    for _ in 0..32 {
        let Some(parent) = current.parent() else {
            return false;
        };
        if parent.kind() == "delete_statement" {
            return true;
        }
        if matches!(
            parent.kind(),
            "assignment" | "augmented_assignment" | "named_expression"
        ) {
            return parent
                .child_by_field_name("left")
                .or_else(|| parent.child_by_field_name("name"))
                .is_some_and(|target| {
                    target.start_byte() <= node.start_byte() && node.end_byte() <= target.end_byte()
                });
        }
        if matches!(
            parent.kind(),
            "expression_statement" | "return_statement" | "call" | "module" | "block"
        ) {
            return false;
        }
        current = parent;
    }
    true
}

pub(super) fn analyze(root: Node<'_>, raw: &[u8], limits: &Limits) -> Value {
    let mut issues = BTreeSet::new();
    let mut nodes = vec![];
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if nodes.len() >= limits.nodes {
            return json!({"operations":[],"issues":["python_environment_node_limit"]});
        }
        nodes.push(node);
        pending.extend(children(node));
    }
    if root.has_error() {
        return json!({"operations":[],"issues":["python_environment_parse_incomplete"]});
    }
    let mut imports = BTreeMap::<String, Import>::new();
    let mut counts = BTreeMap::<String, usize>::new();
    let mut unsafe_names = BTreeSet::new();
    let mut wildcard = false;
    for &node in &nodes {
        if node.kind() == "call"
            && node.child_by_field_name("function").is_some_and(|f| {
                matches!(value(f, raw), "__import__" | "exec" | "eval")
                    || attribute(f, raw).is_some_and(|(_, name)| name == "import_module")
            })
        {
            issues.insert("python_environment_dynamic_execution");
        }
        if matches!(node.kind(), "import_statement" | "import_from_statement") {
            let from = node.child_by_field_name("module_name");
            let module = from.map(|n| value(n, raw));
            for item in children(node) {
                if Some(item) == from {
                    continue;
                }
                let name = item
                    .child_by_field_name("name")
                    .map(|n| value(n, raw))
                    .unwrap_or_else(|| value(item, raw));
                if name == "*" {
                    wildcard = true;
                    continue;
                }
                let alias = item
                    .child_by_field_name("alias")
                    .map(|n| value(n, raw))
                    .unwrap_or_else(|| {
                        if from.is_none() {
                            name.split('.').next().unwrap_or(name)
                        } else {
                            name
                        }
                    });
                *counts.entry(alias.to_owned()).or_default() += 1;
                let kind = match (module, name) {
                    (None, "os") => Some(Binding::Os),
                    (Some("os"), "getenv") => Some(Binding::Getenv),
                    (Some("os"), "environ") => Some(Binding::Environ),
                    (Some("os"), "putenv" | "unsetenv") => Some(Binding::Mutation),
                    _ => None,
                };
                if kind.is_none()
                    && (module == Some("os") || (module.is_none() && name.starts_with("os.")))
                {
                    issues.insert("python_environment_unsupported_os_import");
                }
                if let Some(kind) = kind {
                    imports.insert(
                        alias.to_owned(),
                        Import {
                            kind,
                            end: node.end_byte(),
                        },
                    );
                    if node.parent().is_none_or(|p| p.kind() != "module") {
                        unsafe_names.insert(alias.to_owned());
                    }
                }
            }
        }
        if let Some(parameters) = node.child_by_field_name("parameters") {
            pattern_names(parameters, raw, &mut unsafe_names);
        }
        if matches!(node.kind(), "function_definition" | "class_definition") {
            if let Some(name) = node.child_by_field_name("name") {
                pattern_names(name, raw, &mut unsafe_names);
            }
        }
        if matches!(
            node.kind(),
            "assignment"
                | "augmented_assignment"
                | "named_expression"
                | "for_statement"
                | "for_in_clause"
        ) {
            if let Some(target) = node
                .child_by_field_name("left")
                .or_else(|| node.child_by_field_name("name"))
            {
                if target.kind() == "attribute" {
                    assignment_names(target, raw, &mut unsafe_names);
                } else if target.kind() != "subscript" {
                    pattern_names(target, raw, &mut unsafe_names);
                }
            }
        }
        if matches!(
            node.kind(),
            "as_pattern" | "case_pattern" | "global_statement" | "nonlocal_statement"
        ) {
            pattern_names(node, raw, &mut unsafe_names);
        }
        if node.kind() == "delete_statement" {
            for target in children(node) {
                if target.kind() != "subscript" {
                    assignment_names(target, raw, &mut unsafe_names);
                }
            }
        }
    }
    if wildcard {
        issues.insert("python_environment_binding_ambiguous");
    }
    if imports.is_empty() {
        return json!({"operations":[],"issues":issues});
    }
    for name in imports.keys() {
        if wildcard || counts.get(name) != Some(&1) {
            unsafe_names.insert(name.clone());
        }
    }
    if imports.keys().any(|name| unsafe_names.contains(name)) {
        issues.insert("python_environment_binding_ambiguous");
    }
    let mut operations = vec![];
    let mut consumed = BTreeSet::new();
    for &node in &nodes {
        let mut operation = None;
        if node.kind() == "call" {
            if let Some(function) = node.child_by_field_name("function") {
                let target = binding(function, raw, &imports, &unsafe_names, node.start_byte());
                let env_method = attribute(function, raw).filter(|(object, _)| {
                    binding(*object, raw, &imports, &unsafe_names, node.start_byte())
                        == Some(Binding::Environ)
                });
                let kind = match (target, env_method.map(|(_, member)| member)) {
                    (Some(Binding::Getenv), _) | (_, Some("get")) => Some("read"),
                    (Some(Binding::Mutation), _)
                    | (
                        _,
                        Some(
                            "update" | "clear" | "pop" | "popitem" | "setdefault" | "__setitem__"
                            | "__delitem__",
                        ),
                    ) => Some("write"),
                    (_, Some(_)) => Some("dynamic"),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let args = node
                        .child_by_field_name("arguments")
                        .map(children)
                        .unwrap_or_default();
                    let arg = args
                        .iter()
                        .copied()
                        .find(|a| a.kind() != "keyword_argument")
                        .or_else(|| {
                            args.iter()
                                .find(|a| {
                                    a.kind() == "keyword_argument"
                                        && a.child_by_field_name("name")
                                            .is_some_and(|n| value(n, raw) == "key")
                                })
                                .and_then(|a| a.child_by_field_name("value"))
                        });
                    operation = Some((
                        kind,
                        if kind == "dynamic" {
                            None
                        } else {
                            arg.and_then(|a| key(a, raw))
                        },
                    ));
                    consumed.insert((function.start_byte(), function.end_byte()));
                }
            }
        } else if node.kind() == "subscript" {
            if let Some(object) = node.child_by_field_name("value") {
                if binding(object, raw, &imports, &unsafe_names, node.start_byte())
                    == Some(Binding::Environ)
                {
                    operation = Some((
                        if written(node) { "write" } else { "read" },
                        node.child_by_field_name("subscript")
                            .and_then(|n| key(n, raw)),
                    ));
                    consumed.insert((object.start_byte(), object.end_byte()));
                }
            }
        }
        if let Some((kind, key)) = operation {
            if operations.len() >= limits.calls {
                issues.insert("python_environment_operation_limit");
                break;
            }
            operations.push(json!({"kind":kind,"key":key,"byte_start":node.start_byte(),"byte_end":node.end_byte(),"line":node.start_position().row+1,"evidence":"python_import_binding"}));
        }
    }
    // Passing/storing an os binding, monkey-patching it or reading it before
    // its import invalidates precise coverage. Strings/comments never reach here.
    for &node in &nodes {
        if node.kind() != "identifier" || !imports.contains_key(value(node, raw)) {
            continue;
        }
        let mut current = node;
        let mut in_import = false;
        while let Some(parent) = current.parent() {
            if matches!(parent.kind(), "import_statement" | "import_from_statement") {
                in_import = true;
                break;
            }
            if !matches!(parent.kind(), "aliased_import" | "dotted_name") {
                break;
            }
            current = parent;
        }
        if in_import {
            continue;
        }
        if consumed
            .range(..=(node.start_byte(), usize::MAX))
            .next_back()
            .is_some_and(|(_, end)| node.end_byte() <= *end)
        {
            continue;
        }
        if node.parent().is_some_and(|p| {
            p.kind() == "attribute" && p.child_by_field_name("attribute") == Some(node)
        }) {
            continue;
        }
        issues.insert("python_environment_binding_escape_or_unresolved_use");
    }
    json!({"operations":operations,"issues":issues})
}
