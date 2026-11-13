//! Bounded, source-only dependency declarations. Never execute shell commands,
//! evaluate HCL, resolve host paths, or read outside the supplied source bytes.
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use tree_sitter::Node;

/// The running script's own directory, written the usual ways (double quotes
/// removed first): `$(dirname "$0")`, `$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)`,
/// `${0%/*}`. Group `up` is a literal path inside the `cd`.
fn script_dir() -> String {
    let me = r"\$\{?(?:0|BASH_SOURCE(?:\[0\])?)\}?";
    let dirname = format!(r"(?:\$\(\s*dirname\s+(?:--\s+)?{me}\s*\)|`dirname\s+(?:--\s+)?{me}`)");
    let quiet = r"(?:>\s*/dev/null\s*(?:2>&1\s*)?)?";
    let cd = format!(
        r"\$\(\s*(?:builtin\s+)?cd\s+(?:-P\s+)?(?:--\s+)?{dirname}(?P<up>/[^\s$`()&;|]*)?\s*{quiet}(?:&&|;)\s*pwd(?:\s+-P)?\s*\)"
    );
    format!(r"{cd}|{dirname}|\$\{{(?:0|BASH_SOURCE(?:\[0\])?)%/\*\}}")
}
static IDIOM: LazyLock<Regex> = LazyLock::new(|| Regex::new(&script_dir()).unwrap());
static WHOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("^(?:{})$", script_dir())).unwrap());
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap()
});

/// `.` plus the literal path an idiom adds to the script's directory.
fn here(found: &regex::Captures<'_>) -> String {
    format!(".{}", found.name("up").map_or("", |m| m.as_str()))
}

/// Variables every assignment in the file sets to the script's directory (as
/// a path relative to it). One other assignment makes a variable dynamic.
fn script_dirs(root: Node<'_>, source: &str) -> HashMap<String, Option<String>> {
    let mut dirs = HashMap::<String, Option<String>>::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.kind() == "variable_assignment"
            && let Some(name) = node.child_by_field_name("name")
        {
            let value = node
                .child_by_field_name("value")
                .map_or(String::new(), |v| text(v, source).replace('"', ""));
            let dir = WHOLE.captures(&value).map(|c| here(&c));
            let entry = dirs
                .entry(text(name, source).to_owned())
                .or_insert(dir.clone());
            if *entry != dir {
                *entry = None;
            }
        }
        pending.extend(children(node));
    }
    dirs
}

/// A shell word with the script-directory idiom and its variables replaced by
/// the relative path they stand for; other expansions are left for the
/// literal check to reject.
fn expand(raw: &str, dirs: &HashMap<String, Option<String>>) -> String {
    if raw.starts_with('\'') || !raw.contains(['$', '`']) {
        return raw.to_owned();
    }
    let word = raw.replace('"', "");
    let word = IDIOM.replace_all(&word, |c: &regex::Captures<'_>| here(c));
    VARIABLE
        .replace_all(&word, |c: &regex::Captures<'_>| {
            let name = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
            match dirs.get(name) {
                Some(Some(dir)) => dir.clone(),
                _ => c[0].to_owned(),
            }
        })
        .into_owned()
}

/// The script a `bash`/`sh` command runs: its first operand. `-c` runs a
/// string, not a file; `-o`/`-O` take a value.
fn shell_script<'t>(node: Node<'t>, source: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    let mut arguments = node.children_by_field_name("argument", &mut cursor);
    while let Some(argument) = arguments.next() {
        let word = text(argument, source);
        if word.starts_with("--") {
            continue;
        }
        if let Some(flags) = word.strip_prefix(['-', '+']) {
            if flags.contains('c') {
                return None;
            }
            if flags.contains(['o', 'O']) {
                arguments.next();
            }
            continue;
        }
        return Some(argument);
    }
    None
}

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
    let dirs = if language == "bash" {
        script_dirs(tree.root_node(), source)
    } else {
        HashMap::new()
    };
    let mut pending = vec![tree.root_node()];
    let mut visited = 0;
    while let Some(node) = pending.pop() {
        visited += 1;
        if visited > 100_000 || dependencies.len() >= 4096 {
            issues.push("dependency_analysis_limit");
            break;
        }
        let candidate = if language == "bash" && node.kind() == "command" {
            let name = node.child_by_field_name("name");
            let command = name.map(|n| text(n, source)).unwrap_or("");
            let shell = matches!(command.rsplit('/').next(), Some("bash" | "sh"))
                && (!command.contains('/') || command.starts_with('/'));
            if matches!(command, "source" | ".") {
                node.child_by_field_name("argument")
                    .map(|n| (n, "shell_path"))
            } else if shell {
                shell_script(node, source).map(|n| (n, "shell_path"))
            } else if command.contains('/') && !command.starts_with(['/', '~']) {
                // A command run by its path, never one found through PATH.
                name.map(|n| (n, "shell_command"))
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
            let expanded = if language == "bash" {
                expand(text(value, source), &dirs)
            } else {
                text(value, source).to_owned()
            };
            let raw = expanded.as_str();
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
