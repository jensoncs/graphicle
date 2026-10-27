//! An imported name is not a permanent binding. All resolution passes must
//! respect lexical rebinding, including the flow-insensitive precise pass.
use super::{
    engine_scope::PythonReceiverShadow, parse_cache, resolution::source_key,
    resolution_facts::python_imported_names,
};
use crate::resolver_registry::{Rec, rs};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub fn suppress<E: Rec>(paths: &[PathBuf], edges: &mut Vec<E>, root: &Path) -> Vec<Value> {
    let mut unsupported = BTreeMap::new();
    for path in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "py"))
    {
        let Some(parsed) = parse_cache::parse(path, "python", tree_sitter_python::LANGUAGE.into())
        else {
            continue;
        };
        let tree = parsed.tree.root_node();
        let mut imports = BTreeSet::new();
        let mut calls = Vec::new();
        let mut pending = vec![tree];
        while let Some(node) = pending.pop() {
            if node.kind() == "import_from_statement" {
                imports.extend(
                    python_imported_names(node, &parsed.source)
                        .into_iter()
                        .map(|(_, local)| local),
                );
            }
            if node.kind() == "call" {
                calls.push(node);
            }
            let mut cursor = node.walk();
            pending.extend(node.named_children(&mut cursor));
        }
        let mut shadow = PythonReceiverShadow::for_imports(tree, &parsed.source);
        for call in calls {
            let Some(func) = call
                .child_by_field_name("function")
                .filter(|n| n.kind() == "identifier")
            else {
                continue;
            };
            let name = func.utf8_text(&parsed.source).unwrap_or("");
            if imports.contains(name) && shadow.is_shadowed(call, name, &parsed.source) {
                unsupported.insert(
                    (
                        source_key(&path.to_string_lossy(), root),
                        format!("L{}", call.start_position().row + 1),
                    ),
                    name.to_string(),
                );
            }
        }
    }
    // A line can contain more than one call. The graph does not store columns;
    // suppressing all call edges on that line is conservative, never a guess.
    if unsupported.is_empty() {
        return vec![];
    }
    edges.retain(|e| {
        rs(e, "relation") != "calls"
            || !unsupported.contains_key(&(
                source_key(rs(e, "source_file"), root),
                rs(e, "source_location").into(),
            ))
    });
    unsupported
        .into_iter()
        .map(|((file, location), callee)| {
            json!({
                "source_file": file, "source_location": location, "callee": callee,
                "language": "python", "reason": "unsupported_rebinding"
            })
        })
        .collect()
}
