//! Bounded, static Helm references. No rendering, subprocesses, cluster reads,
//! values merging or execution of template functions. Chart boundaries keep
//! unrelated charts apart; finite expressions resolve while runtime-dependent
//! names and unsupported scopes stay explicitly unresolved.
use super::base::make_id;
use crate::{Result, fsutil};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use yaml_rust2::{
    parser::{Event, MarkedEventReceiver, Parser},
    scanner::Marker,
};

mod expressions;

const MAX_BYTES: usize = 1_048_576;
const MAX_ITEMS: usize = 4096;

/// Only chart values and files beneath a chart's templates directory qualify.
/// A generic YAML config or a Go template outside a chart keeps its extractor.
pub fn chart_root(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if !matches!(
        path.extension()?.to_str()?,
        "yaml" | "yml" | "tpl" | "gotmpl"
    ) {
        return None;
    }
    let parent = path.parent()?;
    let chart = |dir: &Path| dir.join("Chart.yaml").is_file() || dir.join("Chart.yml").is_file();
    if (name == "values.yaml" || name == "values.yml" || name.starts_with("values-"))
        && chart(parent)
    {
        return Some(parent.to_path_buf());
    }
    for dir in parent.ancestors().take(32) {
        if dir.file_name().is_some_and(|name| name == "templates") {
            let root = dir.parent()?;
            if chart(root) {
                return Some(root.to_path_buf());
            }
        }
    }
    None
}

fn identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Clone, Debug)]
struct Token {
    value: String,
    quoted: bool,
}

/// Find actions without interpreting delimiters in strings or Go comments.
fn actions(source: &str) -> Result<Vec<(usize, Vec<Token>)>> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut line = 1;
    while let Some(start) = source[cursor..].find("{{").map(|n| n + cursor) {
        line += source[cursor..start]
            .bytes()
            .filter(|&b| b == b'\n')
            .count();
        let action_line = line;
        if out.len() >= MAX_ITEMS {
            return Err("Helm action limit exceeded".into());
        }
        let mut i = start + 2;
        let mut tokens = Vec::new();
        loop {
            if i >= bytes.len() {
                return Err("Unclosed Helm action".into());
            }
            if source[i..].starts_with("}}") {
                i += 2;
                break;
            }
            if bytes[i].is_ascii_whitespace() || bytes[i] == b'-' {
                i += 1;
                continue;
            }
            if source[i..].starts_with("/*") {
                i += 2;
                i += source[i..].find("*/").ok_or("Unclosed Helm comment")? + 2;
                continue;
            }
            let begin = i;
            if matches!(bytes[i], b'"' | b'\'' | b'`') {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 1;
                    }
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err("Unclosed Helm string".into());
                }
                i += 1;
                let raw = &source[begin..i];
                let value = if quote == b'"' {
                    serde_json::from_str::<String>(raw).unwrap_or_default()
                } else if quote == b'`' {
                    raw[1..raw.len() - 1].to_string()
                } else {
                    String::new()
                };
                tokens.push(Token {
                    value,
                    quoted: true,
                });
            } else if b"()|,:=".contains(&bytes[i]) {
                tokens.push(Token {
                    value: source[i..i + 1].into(),
                    quoted: false,
                });
                i += 1;
            } else {
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !b"()|,:=\"'`".contains(&bytes[i])
                    && !source[i..].starts_with("}}")
                {
                    i += source[i..].chars().next().unwrap().len_utf8();
                }
                tokens.push(Token {
                    value: source[begin..i].trim_end_matches('-').into(),
                    quoted: false,
                });
            }
            if tokens.len() >= MAX_ITEMS {
                return Err("Helm token limit exceeded".into());
            }
        }
        out.push((action_line, tokens));
        line += source[start..i].bytes().filter(|&b| b == b'\n').count();
        cursor = i;
    }
    Ok(out)
}

struct Output {
    path: String,
    chart: String,
    nodes: Vec<Value>,
    refs: Vec<Value>,
    issues: BTreeSet<String>,
}
impl Output {
    fn node(&mut self, name: &str, kind: &str, line: usize) -> String {
        let id = make_id([self.path.as_str(), kind, name, &line.to_string()]);
        self.nodes.push(
            json!({"id":id,"label":name,"file_type":"code","source_file":self.path,
            "source_location":format!("L{line}"),"_helm_chart":self.chart,"_helm_kind":kind}),
        );
        id
    }
    fn reference(&mut self, owner: &str, name: &str, kind: &str, line: usize) {
        self.refs
            .push(json!({"owner":owner,"name":name,"kind":kind,"line":line}));
    }
    fn template(&mut self, source: &str) -> Result<()> {
        let relative = Path::new(&self.path)
            .strip_prefix(&self.chart)
            .unwrap_or(Path::new(&self.path));
        let render = self.node(
            &format!("template.{}", fsutil::path_string(relative)),
            "render",
            1,
        );
        use expressions::{Context, Value as StaticValue};
        // Lexical scopes save bindings and dot independently of the template
        // owner. A helper receives its own root; a with-body receives its map.
        let mut blocks: Vec<(String, Context)> = Vec::new();
        let mut owner = render;
        let mut context = Context::default();
        for (line, tokens) in actions(source)? {
            let Some(first) = tokens.first() else {
                continue;
            };
            let command = if first.quoted {
                ""
            } else {
                first.value.as_str()
            };
            match command {
                "define" | "block" => {
                    let name = tokens.get(1).filter(|t| t.quoted && !t.value.is_empty());
                    if command == "block"
                        && let Some(name) = name
                    {
                        self.reference(&owner, &name.value, "helper", line);
                        if !tokens.get(2).is_some_and(|t| {
                            !t.quoted && t.value == "." && context.dot == StaticValue::Root
                        }) {
                            self.issues.insert("custom_helper_context".into());
                        }
                    }
                    blocks.push((owner.clone(), context.clone()));
                    if let Some(name) = name {
                        owner = self.node(&name.value, "helper", line);
                    } else {
                        self.issues.insert("dynamic_definition".into());
                    }
                    context = Context::default();
                }
                "end" => {
                    let Some((previous, saved)) = blocks.pop() else {
                        return Err("Unmatched Helm end".into());
                    };
                    owner = previous;
                    context = saved;
                }
                "else" => {
                    let Some((_, saved)) = blocks.last() else {
                        return Err("Unmatched Helm else".into());
                    };
                    context = saved.clone();
                    if tokens.len() > 1 {
                        self.issues.insert("chained_template_branch".into());
                    }
                }
                "if" | "with" | "range" => {
                    let mut evaluated = expressions::analyze(&tokens[1..], &context);
                    evaluated.read();
                    for (kind, name) in evaluated.references {
                        self.reference(&owner, &name, &kind, line);
                    }
                    self.issues.extend(evaluated.issues);
                    blocks.push((owner.clone(), context.clone()));
                    if command == "with" {
                        context.dot = evaluated.value;
                    } else if command == "range" {
                        context.dot = StaticValue::Unknown;
                    }
                    if tokens.iter().any(|t| !t.quoted && t.value == "=") {
                        self.issues.insert("control_variable_binding".into());
                    }
                }
                _ => {
                    let declaration = tokens.len() >= 4
                        && command.starts_with('$')
                        && identifier(&command[1..])
                        && tokens[1].value == ":"
                        && tokens[2].value == "=";
                    if !declaration && tokens.iter().any(|t| !t.quoted && t.value == "=") {
                        // Reassignment can escape a conditional/loop. Do not
                        // treat the last syntactic assignment as a known value.
                        self.issues.insert("mutable_template_variable".into());
                        if command.starts_with('$') && identifier(&command[1..]) {
                            context
                                .variables
                                .insert(command.into(), StaticValue::Unknown);
                            for (_, saved) in &mut blocks {
                                if saved.variables.contains_key(command) {
                                    saved.variables.insert(command.into(), StaticValue::Unknown);
                                }
                            }
                        }
                    }
                    let expression = if declaration {
                        &tokens[3..]
                    } else {
                        &tokens[..]
                    };
                    let mut evaluated = expressions::analyze(expression, &context);
                    if declaration {
                        context
                            .variables
                            .insert(command.into(), evaluated.value.clone());
                    } else {
                        evaluated.read();
                    }
                    for (kind, name) in evaluated.references {
                        self.reference(&owner, &name, &kind, line);
                    }
                    self.issues.extend(evaluated.issues);
                }
            }
            if blocks.len() > 128 || self.nodes.len() + self.refs.len() > MAX_ITEMS {
                return Err("Helm structure limit exceeded".into());
            }
        }
        if !blocks.is_empty() {
            return Err("Unclosed Helm control block".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart(files: &[(&str, &str)]) -> (tempfile::TempDir, Vec<Value>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Chart.yaml"),
            "apiVersion: v2\nname: demo\nversion: 0.1.0\n",
        )
        .unwrap();
        let mut nodes = vec![];
        for (name, source) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, source).unwrap();
            nodes.extend(
                extract(&path).unwrap()["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
        }
        (dir, nodes)
    }

    #[test]
    fn helpers_and_nested_values_have_source_locations_and_directed_references() {
        let (_dir, mut nodes) = chart(&[
            ("values.yaml", "replicas: 2\nservice:\n  port: 8080\n"),
            (
                "templates/_helpers.tpl",
                "{{/* héllo }} is inside a comment */}}\n{{ define \"demo.labels\" }}\napp: demo\n{{ end }}\n",
            ),
            (
                "templates/service.yaml",
                "{{ include \"demo.labels\" . }}\nport: {{ .Values.service.port }}\n",
            ),
        ]);
        let mut edges = vec![];
        resolve(&[], &mut nodes, &mut edges);
        let target = |label| nodes.iter().find(|n| n["label"] == label).unwrap()["id"].clone();
        let render = target("template.templates/service.yaml");
        for label in ["demo.labels", "Values.service.port"] {
            assert!(
                edges
                    .iter()
                    .any(|e| e["source"] == render && e["target"] == target(label)),
                "{edges:#?}"
            );
        }
        assert!(
            !edges
                .iter()
                .any(|e| e["target"] == target("Values.replicas"))
        );
        assert_eq!(
            nodes
                .iter()
                .find(|n| n["label"] == "Values.service.port")
                .unwrap()["source_location"],
            "L3"
        );
    }

    #[test]
    fn parent_value_references_reach_children_but_not_siblings() {
        let (_dir, mut nodes) = chart(&[
            ("values.yaml", "service:\n  port: 80\nreplicas: 2\n"),
            ("templates/service.yaml", "{{ toYaml .Values.service }}"),
        ]);
        let mut edges = vec![];
        resolve(&[], &mut nodes, &mut edges);
        let id = |name| nodes.iter().find(|n| n["label"] == name).unwrap()["id"].clone();
        assert!(edges.iter().any(
            |e| e["source"] == id("Values.service") && e["target"] == id("Values.service.port")
        ));
        assert!(!edges.iter().any(|e| e["target"] == id("Values.replicas")));
        let (_dir, nodes) = chart(&[("templates/all.yaml", "{{ toYaml .Values }}")]);
        assert!(!nodes[0]["_helm_issues"].as_array().unwrap().is_empty());
    }

    #[test]
    fn strings_comments_and_changed_dot_do_not_invent_references() {
        let (_dir, nodes) = chart(&[(
            "templates/test.yaml",
            "{{/* include \"fake\" . .Values.fake */}}\n{{ printf \"}} .Values.fake\" }}\n{{ with .Values.service }}\n{{ .Values.wrong }}\n{{ $.Values.real }}\n{{ end }}\n{{ include $helper . }}\n",
        )]);
        let refs = nodes[0]["_helm_references"].as_array().unwrap();
        assert_eq!(
            refs.iter()
                .map(|r| r["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "Values.service",
                "Values.service.Values.wrong",
                "Values.real"
            ]
        );
        assert!(
            nodes[0]["_helm_issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x == "dynamic_helper")
        );
    }

    #[test]
    fn computed_helpers_aliases_and_indirect_value_access_have_precise_references() {
        let (_dir, mut nodes) = chart(&[
            (
                "values.yaml",
                "service:\n  port: 80\nreplicas: 2\nmodern: true\n",
            ),
            (
                "templates/_helpers.tpl",
                "{{ define \"demo.labels\" }}old{{ end }}\n{{ define \"demo.modern\" }}new{{ end }}",
            ),
            (
                "templates/service.yaml",
                r#"{{ $name := printf "%s.labels" "demo" }}
{{ $root := . }}{{ $values := .Values }}
{{ include $name $root | nindent 2 }}
{{ index $values "service" "port" }}
{{ get .Values.service "port" }}
{{ .Values | dig "service" "port" 80 }}
{{ with .Values.service }}{{ .port }}{{ end }}
{{ include (ternary "demo.labels" "demo.modern" .Values.modern) . }}
{{ tpl "{{ include \"demo.labels\" . }}" . }}"#,
            ),
        ]);
        let mut edges = vec![];
        resolve(&[], &mut nodes, &mut edges);
        let render = nodes
            .iter()
            .find(|n| n["label"] == "template.templates/service.yaml")
            .unwrap();
        assert_eq!(render["_helm_issues"], json!([]), "{render:#}");
        assert_eq!(render["_helm_unresolved"], json!([]), "{render:#}");
        let refs: BTreeSet<_> = render["_helm_references"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            refs,
            BTreeSet::from([
                "demo.labels",
                "demo.modern",
                "Values.service.port",
                "Values.service",
                "Values.modern"
            ])
        );
        assert!(
            !edges
                .iter()
                .any(|e| e["source"] == render["id"] && e["_helm_reference"] == "Values.replicas")
        );
    }

    #[test]
    fn runtime_names_reassignments_and_nonliteral_tpl_never_choose_guessed_targets() {
        for source in [
            r#"{{ include .Values.helper . }}"#,
            r#"{{ tpl .Values.config . }}"#,
            r#"{{ $key := .Values.key }}{{ index .Values $key }}"#,
            r#"{{ $name := "demo.labels" }}{{ if .Values.flag }}{{ $name = "demo.other" }}{{ end }}{{ include $name . }}"#,
            r#"{{ $name := "demo.labels" }}{{ with .Values.service }}{{ $name := "demo.other" }}{{ end }}{{ include $name (dict "Values" .Values) }}"#,
        ] {
            let (_dir, nodes) = chart(&[("templates/use.yaml", source)]);
            let node = &nodes[0];
            assert!(
                !node["_helm_issues"].as_array().unwrap().is_empty(),
                "{source}: {node:#}"
            );
            if source.contains("$name =") {
                assert!(
                    !node["_helm_references"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["kind"] == "helper")
                );
            }
        }
        let (_dir, nodes) = chart(&[(
            "templates/use.yaml",
            r#"{{ $name := "demo.labels" }}{{ with .Values.service }}{{ $name := "demo.other" }}{{ end }}{{ include $name . }}"#,
        )]);
        assert!(
            nodes[0]["_helm_references"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "demo.labels")
        );
        assert!(
            !nodes[0]["_helm_references"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "demo.other")
        );
    }

    #[test]
    fn duplicate_helpers_and_missing_values_stay_unresolved() {
        let (_dir, mut nodes) = chart(&[
            ("templates/a.tpl", "{{ define \"demo.labels\" }}a{{ end }}"),
            ("templates/b.tpl", "{{ define \"demo.labels\" }}b{{ end }}"),
            (
                "templates/use.yaml",
                "{{ template \"demo.labels\" . }} {{ .Values.absent }}",
            ),
        ]);
        let mut edges = vec![];
        resolve(&[], &mut nodes, &mut edges);
        assert!(edges.is_empty());
        assert_eq!(
            nodes
                .iter()
                .find(|n| n["label"] == "template.templates/use.yaml")
                .unwrap()["_helm_unresolved"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn overrides_and_other_charts_are_not_effective_defaults() {
        let (_first, mut nodes) = chart(&[
            ("values.yaml", "port: 80\n"),
            ("values-prod.yaml", "port: 443\n"),
            ("templates/service.yaml", "port: {{ .Values.port }}"),
        ]);
        let (_second, others) = chart(&[("values.yaml", "port: 1234\n")]);
        nodes.extend(others);
        let mut edges = vec![];
        resolve(&[], &mut nodes, &mut edges);
        assert_eq!(edges.len(), 1);
        let target = nodes
            .iter()
            .find(|n| n["id"] == edges[0]["target"])
            .unwrap();
        assert_eq!(
            target["source_file"],
            json!(fsutil::path_string(&_first.path().join("values.yaml")))
        );
    }

    #[test]
    fn plain_yaml_and_unsafe_or_malformed_templates_are_not_guessed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("values.yaml");
        std::fs::write(&path, "replicas: 2\n").unwrap();
        assert!(chart_root(&path).is_none());
        std::fs::write(dir.path().join("Chart.yaml"), "name: test\n").unwrap();
        std::fs::write(&path, "foo: !!python/object/apply:os.system ['bad']\n").unwrap();
        assert_eq!(extract(&path).unwrap()["nodes"], json!([]));
        std::fs::create_dir(dir.path().join("templates")).unwrap();
        let path = dir.path().join("templates/x.tpl");
        for source in ["{{ define \"x\" }}", "{{ end }}", "{{ printf \"broken }}"] {
            std::fs::write(&path, source).unwrap();
            assert_eq!(extract(&path).unwrap()["nodes"], json!([]));
        }
    }

    #[test]
    fn aliases_duplicates_and_multiple_documents_do_not_claim_complete_values() {
        let (_dir, nodes) = chart(&[("values.yaml", "a: &a [x]\nb: &b [*a, *a]\nc: [*b, *b]\n")]);
        assert!(nodes.iter().all(|n| {
            n["_helm_issues"]
                .as_array()
                .unwrap()
                .contains(&json!("aliased_value"))
        }));
        let (_dir, nodes) = chart(&[(
            "values.yaml",
            "service:\n  port: 80\nservice:\n  target: 90\n",
        )]);
        assert!(nodes.iter().all(|n| {
            n["_helm_issues"]
                .as_array()
                .unwrap()
                .contains(&json!("duplicate_value"))
        }));
        let (_dir, nodes) = chart(&[("values.yaml", "port: 80\n---\nreplicas: 2\n")]);
        assert!(nodes.is_empty());
    }

    #[test]
    fn dotted_literal_keys_and_sequence_members_are_not_value_paths() {
        let (_dir, nodes) = chart(&[(
            "values.yaml",
            "'a.b': 1\nitems:\n  - port: 80\nservice:\n  port: 81\n",
        )]);
        let names: Vec<_> = nodes.iter().map(|n| n["label"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["Values.items", "Values.service", "Values.service.port"]
        );
    }
}

struct Frame {
    path: Option<String>,
    mapping: bool,
    key: Option<(String, usize)>,
}
struct Values<'a> {
    output: &'a mut Output,
    stack: Vec<Frame>,
    events: usize,
    documents: usize,
    incomplete: bool,
}
impl Values<'_> {
    fn value(&mut self) -> Option<(String, usize)> {
        let frame = self.stack.last_mut()?;
        let (key, line) = frame.key.take()?;
        if !identifier(&key) {
            return None;
        }
        Some((format!("{}.{}", frame.path.as_ref()?, key), line))
    }
}
impl MarkedEventReceiver for Values<'_> {
    fn on_event(&mut self, event: Event, marker: Marker) {
        self.events += 1;
        if self.events > MAX_ITEMS || self.stack.len() > 64 {
            self.incomplete = true;
            return;
        }
        // Inspect syntax only: never construct values or expand YAML aliases.
        // Explicit tags can change collection/key semantics, so leave them to
        // textual retrieval rather than interpreting them here.
        if matches!(
            &event,
            Event::Scalar(_, _, _, Some(_))
                | Event::MappingStart(_, Some(_))
                | Event::SequenceStart(_, Some(_))
        ) {
            self.incomplete = true;
            return;
        }
        match event {
            Event::DocumentStart => {
                self.documents += 1;
                self.incomplete |= self.documents > 1;
            }
            Event::MappingStart(_, _) | Event::SequenceStart(_, _) => {
                if self
                    .stack
                    .last()
                    .is_some_and(|frame| frame.mapping && frame.key.is_none())
                {
                    self.incomplete = true; // A complex mapping key is not a dot-addressable field.
                }
                let mapping = matches!(event, Event::MappingStart(_, _));
                let path = if self.stack.is_empty() {
                    Some("Values".into())
                } else {
                    self.value().map(|(path, line)| {
                        self.output.node(&path, "value", line);
                        path
                    })
                };
                self.stack.push(Frame {
                    path: if mapping { path } else { None },
                    mapping,
                    key: None,
                });
            }
            Event::MappingEnd | Event::SequenceEnd => {
                self.stack.pop();
            }
            Event::Scalar(key, _, _, _) => {
                if let Some(frame) = self.stack.last_mut().filter(|f| f.mapping) {
                    if frame.key.is_none() {
                        // Non-identifier keys cannot be accessed by `.Values.x`.
                        frame.key = Some((key, marker.line()));
                    } else if let Some((path, line)) = self.value() {
                        if path.split('.').all(identifier) {
                            self.output.node(&path, "value", line);
                        }
                    }
                }
            }
            Event::Alias(_) => {
                if self
                    .stack
                    .last()
                    .is_some_and(|frame| frame.mapping && frame.key.is_none())
                {
                    self.incomplete = true;
                }
                self.value();
                self.output.issues.insert("aliased_value".into());
            }
            _ => {}
        }
    }
}

pub fn extract(path: &Path) -> Result<Value> {
    let Some(chart) = chart_root(path) else {
        return Ok(json!({"nodes":[],"edges":[]}));
    };
    let bytes = super::base::read_source_bytes(path, true)?;
    let mut output = Output {
        path: fsutil::path_string(path),
        chart: fsutil::path_string(&chart),
        nodes: vec![],
        refs: vec![],
        issues: BTreeSet::new(),
    };
    let parsed = (|| -> Result<()> {
        if bytes.len() > MAX_BYTES {
            return Err("Helm file size limit exceeded".into());
        }
        let source = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        if path.parent() == Some(chart.as_path()) {
            // Event positions identify keys without constructing values or
            // expanding aliases; malformed streams publish no partial graph.
            let mut values = Values {
                output: &mut output,
                stack: vec![],
                events: 0,
                documents: 0,
                incomplete: false,
            };
            Parser::new_from_str(source).load(&mut values, true)?;
            if values.incomplete {
                return Err("Helm values limit or unsupported key/tag".into());
            }
        } else {
            output.template(source)?;
        }
        Ok(())
    })();
    if let Err(error) = parsed {
        // Unsupported static analysis must not fail a project sync or publish
        // partial declarations. Text retrieval remains available for this file.
        return Ok(
            json!({"nodes":[],"edges":[],"skipped":"unsupported Helm structure",
            "issues":[error.to_string()]}),
        );
    }
    let mut names = BTreeSet::new();
    if output
        .nodes
        .iter()
        .filter(|node| node["_helm_kind"] == "value")
        .any(|node| !names.insert(node["label"].clone().to_string()))
    {
        output.issues.insert("duplicate_value".into());
    }
    // References belong to the source owner, so incremental assembly can resolve
    // them from fresh and unchanged nodes without consulting the host filesystem.
    for node in &mut output.nodes {
        node["_helm_references"] = json!(
            output
                .refs
                .iter()
                .filter(|r| r["owner"] == node["id"])
                .collect::<Vec<_>>()
        );
        node["_helm_issues"] = json!(output.issues);
    }
    Ok(json!({"nodes":output.nodes,"edges":[],"issues":output.issues}))
}

pub fn resolve(_per_file: &[Value], nodes: &mut Vec<Value>, edges: &mut Vec<Value>) {
    let mut definitions: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for node in nodes.iter() {
        let (Some(chart), Some(kind), Some(name), Some(id)) = (
            node["_helm_chart"].as_str(),
            node["_helm_kind"].as_str(),
            node["label"].as_str(),
            node["id"].as_str(),
        ) else {
            continue;
        };
        // Override files are declarations, not unconditional effective defaults.
        let file = Path::new(node["source_file"].as_str().unwrap_or(""))
            .file_name()
            .and_then(|p| p.to_str());
        if kind == "value" && !matches!(file, Some("values.yaml" | "values.yml")) {
            continue;
        }
        definitions
            .entry((chart.into(), kind.into(), name.into()))
            .or_default()
            .push(id.into());
    }
    // A map-valued reference (for example toYaml .Values.service) depends on
    // its addressable descendants, while a leaf never pulls in sibling fields.
    let mut fields: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for node in nodes.iter().filter(|node| node["_helm_kind"] == "value") {
        fields
            .entry((
                node["source_file"].as_str().unwrap_or("").into(),
                node["label"].as_str().unwrap_or("").into(),
            ))
            .or_default()
            .push(node["id"].as_str().unwrap_or("").into());
    }
    let mut seen = BTreeSet::new();
    for node in nodes.iter().filter(|node| node["_helm_kind"] == "value") {
        let name = node["label"].as_str().unwrap_or("");
        let Some((parent, _)) = name.rsplit_once('.') else {
            continue;
        };
        let file = node["source_file"].as_str().unwrap_or("");
        if let Some(parents) = fields
            .get(&(file.into(), parent.into()))
            .filter(|ids| ids.len() == 1)
        {
            let target = node["id"].as_str().unwrap_or("");
            let source = &parents[0];
            if seen.insert((source.clone(), target.to_string())) {
                edges.push(
                    json!({"source":source,"target":target,"relation":"references",
                    "confidence":"EXTRACTED","weight":1.0,"source_file":node["source_file"],
                    "source_location":node["source_location"],"_helm_reference":name}),
                );
            }
        }
    }
    for node in nodes.iter_mut() {
        let Some(chart) = node["_helm_chart"].as_str().map(str::to_string) else {
            continue;
        };
        let refs = node["_helm_references"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut unresolved = Vec::new();
        for reference in refs {
            let key = (
                chart.clone(),
                reference["kind"].as_str().unwrap_or("").into(),
                reference["name"].as_str().unwrap_or("").into(),
            );
            if let Some(targets) = definitions.get(&key).filter(|targets| targets.len() == 1) {
                let target = &targets[0];
                let source = node["id"].as_str().unwrap_or("");
                if source != target && seen.insert((source.to_string(), target.clone())) {
                    edges.push(json!({"source":source,"target":target,"relation":"references",
                        "confidence":"EXTRACTED","weight":1.0,"source_file":node["source_file"],
                        "source_location":format!("L{}", reference["line"]),"_helm_reference":reference["name"]}));
                }
            } else {
                unresolved.push(reference);
            }
        }
        node["_helm_unresolved"] = json!(unresolved);
    }
}
