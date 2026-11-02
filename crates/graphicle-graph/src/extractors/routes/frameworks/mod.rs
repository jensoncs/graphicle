//! Framework declarations with proven import/project identity. These are source
//! facts, not deployment discovery: no hosts, projects or event wiring guessed.
use super::{Kind, Route, ScopeNames, lexical, parse_cache, text};
use crate::ids::make_id;
use rustc_hash::FxHashMap;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

mod java;
mod javascript;
mod rust;

pub(super) fn children(n: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (0..n.named_child_count()).filter_map(move |i| n.named_child(i as _))
}

fn walk<'t>(n: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(n);
    for child in children(n) {
        walk(child, f);
    }
}

/// No escape/interpolation interpretation, placeholders from external config,
/// or arbitrary expressions. Unknown syntax must not become a concrete path.
fn string(n: Node, src: &[u8]) -> Option<String> {
    if !matches!(n.kind(), "string" | "string_literal") {
        return None;
    }
    let raw = text(n, src);
    let quote = raw.chars().next()?;
    if !matches!(quote, '\'' | '"') || !raw.ends_with(quote) || raw.len() < 2 {
        return None;
    }
    let value = &raw[1..raw.len() - 1];
    (!value.contains(['\\', '\n', '\r']) && !value.contains("${") && !value.contains("#{"))
        .then(|| value.to_owned())
}

fn route(
    path: &Path,
    n: Node,
    framework: &str,
    method: &str,
    url: String,
    handler: Option<(String, usize)>,
) -> Route {
    Route {
        file: path.to_string_lossy().into_owned(),
        line: n.start_position().row + 1,
        end_line: n.end_position().row + 1,
        middleware: vec![],
        method: method.into(),
        path: url,
        router: framework.into(),
        kind: Kind::Declared,
        handler,
    }
}

/// Only admitted manifests are read. The nearest package boundary wins,
/// including non-Next packages nested in a Next workspace.
pub(super) struct Projects(FxHashMap<PathBuf, bool>);
impl Projects {
    pub(super) fn new(paths: &[PathBuf], nodes: &[Value], context: &[Value], root: &Path) -> Self {
        let mut manifests = paths.to_vec();
        manifests.extend(
            nodes
                .iter()
                .chain(context)
                .filter_map(|n| n["source_file"].as_str())
                .map(|p| root.join(p)),
        );
        let mut projects = FxHashMap::default();
        for p in manifests {
            if p.file_name().is_none_or(|n| n != "package.json") {
                continue;
            }
            let next = parse_cache::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .is_some_and(|v| {
                    ["dependencies", "devDependencies"]
                        .iter()
                        .any(|k| v[k]["next"].as_str().is_some_and(|v| !v.is_empty()))
                });
            if let Some(dir) = p.parent() {
                projects.insert(lexical(dir), next);
            }
        }
        Self(projects)
    }

    fn next_root(&self, path: &Path) -> Option<&Path> {
        let path = lexical(path);
        let (root, next) = self
            .0
            .iter()
            .filter(|(r, _)| path.starts_with(r))
            .max_by_key(|(r, _)| r.components().count())?;
        next.then_some(root.as_path())
    }
}

pub(super) fn scan(
    path: &Path,
    tag: &str,
    parsed: &crate::extractors::engine_parser::Parsed,
    projects: &Projects,
) -> (Vec<Route>, Vec<Value>) {
    if !matches!(tag, "java" | "rust" | "javascript" | "typescript" | "tsx") {
        return Default::default();
    }
    let root = parsed.tree.root_node();
    // Recovered syntax can turn dynamic/malformed declarations into false facts.
    if root.has_error() {
        return Default::default();
    }
    let src = parsed.source.as_slice();
    match tag {
        "java" => (java::scan(path, root, src), vec![]),
        "rust" => (rust::scan(path, root, src), vec![]),
        _ => javascript::scan(path, root, src, projects),
    }
}

fn boundary(
    path: &Path,
    n: Node,
    node_kind: &str,
    kind: &str,
    name: &str,
    resource: Option<String>,
) -> Value {
    let file = path.to_string_lossy();
    let line = (n.start_position().row + 1).to_string();
    let mut value = json!({"id":make_id([node_kind, file.as_ref(), &line, kind, name]),
        "label":format!("{kind} {name}"), "node_kind":node_kind,
        "file_type":"code", "source_file":file, "source_location":format!("L{line}"),
        "framework":"firebase", "boundary_kind":kind, "boundary_name":name,
        "deployment_identity":"unresolved", "_origin":"ast"});
    if let Some(resource) = resource {
        value["boundary_resource"] = json!(resource);
    }
    value
}
