//! Package-level Go constants and variables (`const EndpointType = ...`,
//! enum values like `SocketAddress_TCP`, `var ErrNotFound = errors.New(..)`).
//! Upstream emits no node for them, so an importer's `pkg.EndpointType` has
//! no definition to land on. Precise mode gives each exported one a node in
//! its file and a `contains` edge from the file; unexported and
//! function-local values stay out (no importer can name them).
use super::base::{make_id, read_text};
use super::parse_cache;
use rustc_hash::FxHashSet;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn absolute(sf: &str, root: &Path) -> PathBuf {
    let p = Path::new(sf);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// Every `const_spec` / `var_spec` of a top-level declaration, through
/// `( ... )` groups and `var_spec_list`.
fn specs<'t>(declaration: Node<'t>, out: &mut Vec<Node<'t>>) {
    let mut cursor = declaration.walk();
    for child in declaration.named_children(&mut cursor) {
        match child.kind() {
            "const_spec" | "var_spec" => out.push(child),
            "var_spec_list" => specs(child, out),
            _ => {}
        }
    }
}

/// The exported names a Go file declares at package level, with their lines.
fn declared(path: &Path) -> Vec<(String, usize)> {
    let Some(parsed) = parse_cache::parse(path, "go", tree_sitter_go::LANGUAGE.into()) else {
        return vec![];
    };
    let source = parsed.source.as_slice();
    let root = parsed.tree.root_node();
    let mut found = vec![];
    let mut cursor = root.walk();
    for declaration in root.named_children(&mut cursor) {
        if !matches!(declaration.kind(), "const_declaration" | "var_declaration") {
            continue;
        }
        let mut list = vec![];
        specs(declaration, &mut list);
        for spec in list {
            let mut names = spec.walk();
            for name in spec.children_by_field_name("name", &mut names) {
                let text = read_text(name, source);
                if text.starts_with(|c: char| c.is_ascii_uppercase()) {
                    found.push((text, name.start_position().row + 1));
                }
            }
        }
    }
    found
}

pub fn declare(nodes: &mut Vec<Value>, edges: &mut Vec<Value>, root: &Path) {
    let files: Vec<(String, String)> = nodes
        .iter()
        .filter(|n| {
            let sf = s(n, "source_file");
            sf.ends_with(".go")
                && !sf.ends_with("_test.go")
                && Path::new(sf).file_name() == Path::new(s(n, "label")).file_name()
        })
        .map(|n| (s(n, "id").to_string(), s(n, "source_file").to_string()))
        .collect();
    if files.is_empty() {
        return;
    }
    let mut seen: FxHashSet<String> = nodes.iter().map(|n| s(n, "id").to_string()).collect();
    let found = parse_cache::par_map(&files, |(_, sf)| declared(&absolute(sf, root)));
    for ((file, sf), names) in files.iter().zip(found) {
        for (name, line) in names {
            let id = make_id([file.as_str(), name.as_str()]);
            if !seen.insert(id.clone()) {
                continue;
            }
            let location = format!("L{line}");
            nodes.push(
                json!({"id":id,"label":name,"file_type":"code","source_file":sf,
                "source_location":location}),
            );
            edges.push(
                json!({"source":file,"target":id,"relation":"contains","confidence":"EXTRACTED",
                "source_file":sf,"source_location":location,"weight":1.0}),
            );
        }
    }
}
