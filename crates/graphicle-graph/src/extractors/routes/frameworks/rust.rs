use super::*;
use rustc_hash::FxHashSet;

struct Imports {
    names: FxHashMap<String, String>,
    shadowed: FxHashSet<String>,
}
impl Imports {
    fn identity(&self, n: Node, src: &[u8]) -> Option<String> {
        if !matches!(n.kind(), "identifier" | "scoped_identifier") {
            return None;
        }
        let raw = text(n, src);
        let (first, rest) = raw.split_once("::").unwrap_or((raw, ""));
        if self.shadowed.contains(first) {
            return None;
        }
        let first = self.names.get(first).map(String::as_str).unwrap_or(first);
        Some(if rest.is_empty() {
            first.into()
        } else {
            format!("{first}::{rest}")
        })
    }
}

fn args(n: Node<'_>) -> Vec<Node<'_>> {
    n.child_by_field_name("arguments")
        .map(|a| children(a).collect())
        .unwrap_or_default()
}

fn member(n: Node<'_>) -> Option<(Node<'_>, Node<'_>)> {
    let f = n.child_by_field_name("function")?;
    (f.kind() == "field_expression").then_some((
        f.child_by_field_name("value")?,
        f.child_by_field_name("field")?,
    ))
}

fn router(n: Node, src: &[u8], imports: &Imports, depth: usize) -> bool {
    if depth > 32 || n.kind() != "call_expression" {
        return false;
    }
    if n.child_by_field_name("function")
        .and_then(|f| imports.identity(f, src))
        .as_deref()
        == Some("axum::Router::new")
    {
        return args(n).is_empty();
    }
    member(n).is_some_and(|(value, field)| {
        matches!(
            text(field, src),
            "route" | "with_state" | "layer" | "route_layer"
        ) && router(value, src, imports, depth + 1)
    })
}

/// Return each verb's actual handler, not one handler for a whole method chain.
fn methods<'t>(
    n: Node<'t>,
    src: &[u8],
    imports: &Imports,
    depth: usize,
) -> Option<Vec<(String, Node<'t>)>> {
    if depth > 16 || n.kind() != "call_expression" {
        return None;
    }
    let arguments = args(n);
    let [handler] = arguments.as_slice() else {
        return None;
    };
    // Names with uncertain/qualified ownership stay unresolved at the symbol layer.
    if handler.kind() != "identifier" {
        return None;
    }
    if let Some((value, field)) = member(n) {
        let verb = text(field, src);
        if !super::super::VERBS.contains(&verb) {
            return None;
        }
        let mut out = methods(value, src, imports, depth + 1)?;
        if out.iter().any(|(v, _)| v == &verb.to_uppercase()) {
            return None;
        }
        out.push((verb.to_uppercase(), *handler));
        return Some(out);
    }
    let identity = imports.identity(n.child_by_field_name("function")?, src)?;
    let verb = identity.strip_prefix("axum::routing::")?;
    super::super::VERBS
        .contains(&verb)
        .then(|| vec![(verb.to_uppercase(), *handler)])
}

pub(super) fn scan(path: &Path, root: Node, src: &[u8]) -> Vec<Route> {
    let mut imports = Imports {
        names: FxHashMap::default(),
        shadowed: FxHashSet::default(),
    };
    for n in children(root).filter(|n| n.kind() == "use_declaration") {
        if let Some(argument) = n.child_by_field_name("argument") {
            for (binding, _) in crate::extractors::rust::use_bindings(argument, src, "", "") {
                let parts = binding.split_whitespace().collect::<Vec<_>>();
                let (value, alias) = match parts.as_slice() {
                    [value, "as", alias] => (*value, *alias),
                    [value] => (*value, value.rsplit("::").next().unwrap_or(value)),
                    _ => continue,
                };
                if alias == "*" {
                    continue;
                }
                if imports.names.insert(alias.into(), value.into()).is_some() {
                    imports.shadowed.insert(alias.into());
                }
            }
        }
    }
    let mut definitions: FxHashMap<String, Vec<usize>> = FxHashMap::default();
    walk(root, &mut |n| {
        if matches!(
            n.kind(),
            "function_item"
                | "struct_item"
                | "enum_item"
                | "type_item"
                | "mod_item"
                | "const_item"
                | "static_item"
        ) && let Some(name) = n.child_by_field_name("name")
        {
            imports.shadowed.insert(text(name, src).into());
            if n.kind() == "function_item" && n.parent() == Some(root) {
                definitions
                    .entry(text(name, src).into())
                    .or_default()
                    .push(name.start_position().row + 1);
            }
        }
        if matches!(
            n.kind(),
            "parameter" | "let_declaration" | "closure_parameters" | "for_expression"
        ) {
            let pattern = n.child_by_field_name("pattern").unwrap_or(n);
            walk(pattern, &mut |p| {
                if p.kind() == "identifier" {
                    imports.shadowed.insert(text(p, src).into());
                }
            });
        }
        if n.kind() == "use_declaration"
            && n.parent() != Some(root)
            && let Some(argument) = n.child_by_field_name("argument")
        {
            for (binding, _) in crate::extractors::rust::use_bindings(argument, src, "", "") {
                let name = binding
                    .split_whitespace()
                    .last()
                    .unwrap_or("")
                    .rsplit("::")
                    .next()
                    .unwrap_or("");
                imports.shadowed.insert(name.into());
            }
        }
    });
    let handler = |name: &str| -> Option<(String, usize)> {
        let lines = definitions.get(name)?;
        (lines.len() == 1).then(|| (name.into(), lines[0]))
    };
    let mut out = vec![];
    let mut scoped_actix = false;
    walk(root, &mut |n| {
        if n.kind() == "call_expression"
            && n.child_by_field_name("function")
                .and_then(|f| imports.identity(f, src))
                .as_deref()
                == Some("actix_web::web::scope")
        {
            scoped_actix = true;
        }
    });
    // Attributes belong to the following item, not to arbitrary nearby names.
    let mut attributes = vec![];
    for item in children(root) {
        if item.kind() == "attribute_item" {
            attributes.push(item);
            continue;
        }
        if item.kind() == "line_comment" || item.kind() == "block_comment" {
            continue;
        }
        if !scoped_actix && item.kind() == "function_item" && attributes.len() == 1 {
            let attribute = attributes[0].named_child(0);
            if let Some(attribute) = attribute {
                let identity = children(attribute)
                    .next()
                    .and_then(|n| imports.identity(n, src));
                if let Some(verb) = identity
                    .as_deref()
                    .and_then(|s| s.strip_prefix("actix_web::"))
                    && super::super::VERBS.contains(&verb)
                    && let Some(arguments) = attribute.child_by_field_name("arguments")
                {
                    let values = children(arguments).collect::<Vec<_>>();
                    if let [value] = values.as_slice()
                        && let Some(url) = string(*value, src)
                        && let Some(name) = item.child_by_field_name("name")
                    {
                        out.push(route(
                            path,
                            item,
                            "actix",
                            &verb.to_uppercase(),
                            url,
                            handler(text(name, src)),
                        ));
                    }
                }
            }
        }
        attributes.clear();
    }
    walk(root, &mut |n| {
        if n.kind() != "call_expression" {
            return;
        }
        let Some((value, field)) = member(n) else {
            return;
        };
        if text(field, src) != "route" || !router(value, src, &imports, 0) {
            return;
        }
        // Prefix-changing or unknown surrounding builders require dedicated
        // composition; do not expose the inner path as a served endpoint.
        let mut ancestor = n.parent();
        while let Some(parent) = ancestor {
            if parent.kind() == "call_expression" {
                let Some((_, field)) = member(parent) else {
                    return;
                };
                if !matches!(
                    text(field, src),
                    "route" | "with_state" | "layer" | "route_layer"
                ) {
                    return;
                }
            }
            ancestor = parent.parent();
        }
        let arguments = args(n);
        let [url, routing] = arguments.as_slice() else {
            return;
        };
        let Some(url) = string(*url, src) else {
            return;
        };
        // Axum 0.8 catchalls use {*name}; legacy *name is also normalized.
        let url = url
            .split('/')
            .map(|part| {
                if let Some(name) = part
                    .strip_prefix("{*")
                    .and_then(|p| p.strip_suffix('}'))
                    .or_else(|| part.strip_prefix('*'))
                {
                    format!("{{{name}:path}}")
                } else {
                    part.into()
                }
            })
            .collect::<Vec<String>>()
            .join("/");
        if let Some(methods) = methods(*routing, src, &imports, 0) {
            for (verb, name) in methods {
                out.push(route(
                    path,
                    n,
                    "axum",
                    &verb,
                    url.clone(),
                    handler(text(name, src)),
                ));
            }
        }
    });
    out
}
