//! Bounded Kubernetes environment syntax over admitted bytes. No rendering,
//! alias expansion, external reads, secret resolution or value interpolation.
use super::yaml_frontmatter;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use yaml_rust2::{
    parser::{Event, MarkedEventReceiver, Parser},
    scanner::Marker,
};

#[derive(Default)]
struct Frame {
    mapping: bool,
    key: bool,
    keys: BTreeSet<String>,
    path: Vec<String>,
    pending: String,
    index: usize,
    start: Option<Marker>,
}
#[derive(Default)]
struct Guard {
    frames: Vec<Frame>,
    events: usize,
    documents: usize,
    invalid: bool,
    spans: BTreeMap<Vec<String>, (Marker, Marker)>,
}
impl MarkedEventReceiver for Guard {
    fn on_event(&mut self, event: Event, marker: Marker) {
        self.events += 1;
        if self.invalid || self.events > 20_000 || self.frames.len() > 64 {
            self.invalid = true;
            return;
        }
        let mapping = matches!(&event, Event::MappingStart(..));
        match event {
            Event::DocumentStart => {
                self.documents += 1;
                self.invalid |= self.documents > 1;
            }
            Event::MappingStart(anchor, tag) | Event::SequenceStart(anchor, tag) => {
                self.invalid |= anchor != 0 || tag.is_some();
                let mut path = vec![];
                if let Some(parent) = self.frames.last_mut() {
                    path = parent.path.clone();
                    if parent.mapping {
                        self.invalid |= parent.key;
                        parent.key = true;
                        path.push(parent.pending.clone());
                    } else {
                        path.push(parent.index.to_string());
                        parent.index += 1;
                    }
                }
                self.frames.push(Frame {
                    mapping,
                    key: true,
                    path,
                    start: Some(marker),
                    ..Default::default()
                });
            }
            Event::MappingEnd | Event::SequenceEnd => {
                if let Some(frame) = self.frames.pop() {
                    if frame.mapping
                        && frame.path.len() > 1
                        && frame.path[frame.path.len() - 2] == "env"
                    {
                        self.spans
                            .insert(frame.path, (frame.start.unwrap(), marker));
                    }
                }
            }
            Event::Scalar(text, _, anchor, tag) => {
                self.invalid |= anchor != 0 || tag.is_some();
                if let Some(parent) = self.frames.last_mut().filter(|f| f.mapping) {
                    if parent.key {
                        if parent.keys.is_empty() {
                            parent.start = Some(marker);
                        }
                        parent.pending = text.clone();
                        self.invalid |= text == "<<" || !parent.keys.insert(text);
                    }
                    parent.key = !parent.key;
                } else if let Some(parent) = self.frames.last_mut() {
                    parent.index += 1;
                }
            }
            Event::Alias(_) => self.invalid = true,
            _ => {}
        }
    }
}

pub struct DeploymentEnvironment {
    /// Used internally to fingerprint inputs; never emitted as telemetry.
    pub values: BTreeMap<String, String>,
    pub spans: BTreeMap<String, std::ops::Range<usize>>,
    /// Full document with just the selected container's literal values erased.
    /// Equality proves that a change did not alter image, command, env names,
    /// other containers, valueFrom, or any other deployment setting.
    pub masked: Value,
    pub workload: Value,
}

pub fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}

fn document(content: &str) -> Result<(Value, Guard), &'static str> {
    if content.len() > 1_048_576 {
        return Err("environment_source_limit");
    }
    let mut guard = Guard::default();
    if Parser::new_from_str(content)
        .load(&mut guard, true)
        .is_err()
        || guard.invalid
    {
        return Err("environment_yaml_unsupported");
    }
    let document = yaml_frontmatter::parse(content).map_err(|_| "environment_yaml_unsupported")?;
    Ok((document, guard))
}
fn pod_pointer(document: &Value) -> Result<&'static str, &'static str> {
    match (document["apiVersion"].as_str(), document["kind"].as_str()) {
        (Some("apps/v1"), Some("Deployment" | "StatefulSet" | "DaemonSet")) => {
            Ok("/spec/template/spec")
        }
        (Some("batch/v1"), Some("Job")) => Ok("/spec/template/spec"),
        (Some("batch/v1"), Some("CronJob")) => Ok("/spec/jobTemplate/spec/template/spec"),
        (Some("v1"), Some("Pod")) => Ok("/spec"),
        _ => Err("environment_workload_unsupported"),
    }
}
/// Discover container identities without interpreting ownership or environment.
/// The caller must refuse ambiguous repository/module/workload associations.
pub fn containers(content: &str) -> Result<Vec<String>, &'static str> {
    let (document, _) = document(content)?;
    let pointer = pod_pointer(&document)?;
    document
        .pointer(pointer)
        .and_then(|pod| pod["containers"].as_array())
        .ok_or("environment_container_missing")?
        .iter()
        .map(|c| {
            c["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .ok_or("environment_container_missing")
        })
        .collect()
}
pub fn deployment(content: &str, container: &str) -> Result<DeploymentEnvironment, &'static str> {
    let (mut document, guard) = document(content)?;
    let pointer = pod_pointer(&document)?;
    let kind = document["kind"].as_str().unwrap();
    let name = document["metadata"]["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("environment_workload_unsupported")?;
    let namespace = document["metadata"]["namespace"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("environment_namespace_missing")?;
    let workload =
        serde_json::json!({"kind":kind,"name":name,"namespace":namespace,"container":container});
    let pod = document
        .pointer_mut(pointer)
        .ok_or("environment_workload_unsupported")?;
    let containers = pod["containers"]
        .as_array_mut()
        .ok_or("environment_container_missing")?;
    let matches: Vec<_> = containers
        .iter()
        .enumerate()
        .filter(|(_, c)| c["name"] == container)
        .map(|(i, _)| i)
        .collect();
    if matches.len() != 1 {
        return Err("environment_container_ambiguous");
    }
    let c = &mut containers[matches[0]];
    if c.get("envFrom").is_some() {
        return Err("environment_env_from_unresolved");
    }
    let env = c["env"]
        .as_array_mut()
        .ok_or("environment_declarations_missing")?;
    if env.len() > 1024 {
        return Err("environment_declaration_limit");
    }
    let mut values = BTreeMap::new();
    let mut spans = BTreeMap::new();
    let offsets: Vec<_> = std::iter::once(0)
        .chain(
            content
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect();
    // yaml-rust2's absolute index mixes character and byte increments on
    // different scanner paths. Lines with ASCII prefixes give an unambiguous
    // byte column; decline ambiguous inline Unicode instead of mis-citing it.
    let offset = |marker: &Marker| -> Result<usize, &'static str> {
        // The scanner closes block collections after an implicit final newline.
        if !content.ends_with('\n') && marker.line() == offsets.len() + 1 && marker.col() == 0 {
            return Ok(content.len());
        }
        let line = *offsets
            .get(
                marker
                    .line()
                    .checked_sub(1)
                    .ok_or("environment_span_unavailable")?,
            )
            .ok_or("environment_span_unavailable")?;
        let at = line
            .checked_add(marker.col())
            .ok_or("environment_span_unavailable")?;
        let prefix = content
            .get(line..at)
            .ok_or("environment_span_unavailable")?;
        if !prefix.is_ascii() || prefix.contains('\n') {
            return Err("environment_span_unavailable");
        }
        Ok(at)
    };
    for (index, item) in env.iter_mut().enumerate() {
        let object = item
            .as_object_mut()
            .ok_or("environment_declaration_invalid")?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| key(s))
            .ok_or("environment_key_unresolved")?
            .to_owned();
        if object.keys().any(|k| k != "name" && k != "value") {
            return Err("environment_value_from_unresolved");
        }
        let value = object
            .get("value")
            .and_then(Value::as_str)
            .ok_or("environment_value_unresolved")?;
        if value.contains("$(") || value.contains("{{") {
            return Err("environment_value_unresolved");
        }
        let path: Vec<String> = pointer
            .split('/')
            .skip(1)
            .map(str::to_owned)
            .chain([
                "containers".into(),
                matches[0].to_string(),
                "env".into(),
                index.to_string(),
            ])
            .collect();
        let (start, end) = guard
            .spans
            .get(&path)
            .ok_or("environment_span_unavailable")?;
        let start = offset(start)?;
        let end = offset(end)?;
        if start >= end || content.get(start..end).is_none() {
            return Err("environment_span_unavailable");
        }
        spans.insert(name.clone(), start..end);
        if values.insert(name, value.to_owned()).is_some() {
            return Err("environment_key_ambiguous");
        }
        object.insert("value".into(), Value::Null);
    }
    // Retain exact non-env bytes as well as structure. YAML scalar coercions
    // or parser normalization must never erase another deployment change.
    let mut ranges: Vec<_> = spans.values().cloned().collect();
    ranges.sort_by_key(|span| span.start);
    let mut outside = String::new();
    let mut cursor = 0;
    for span in ranges {
        if span.start < cursor {
            return Err("environment_span_ambiguous");
        }
        outside.push_str(&content[cursor..span.start]);
        cursor = span.end;
    }
    outside.push_str(&content[cursor..]);
    Ok(DeploymentEnvironment {
        values,
        spans,
        masked: serde_json::json!({"document":document,"outside_environment":outside}),
        workload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const YAML: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: app\n  namespace: test\nspec:\n  template:\n    spec:\n      containers:\n        - name: app\n          image: app:v1\n          env:\n            - name: TIMEOUT\n              value: \"10\"\n";
    #[test]
    fn literal_values_are_the_only_erased_information() {
        let a = deployment(YAML, "app").unwrap();
        let b = deployment(&YAML.replace("\"10\"", "\"20\""), "app").unwrap();
        assert_eq!(a.masked, b.masked);
        assert_ne!(a.values, b.values);
        for (from, to) in [
            ("app:v1", "app:v2"),
            ("TIMEOUT", "OTHER"),
            ("namespace: test", "namespace: prod"),
        ] {
            assert_ne!(
                a.masked,
                deployment(&YAML.replace(from, to), "app").unwrap().masked
            );
        }
    }
    #[test]
    fn citations_use_byte_offsets_and_exclude_other_inputs() {
        let content = YAML.replace("kind: Deployment", "# 日本語\nkind: Deployment")
            + "            - name: OTHER\n              value: \"unrelated\"\n";
        let parsed = deployment(&content, "app").unwrap();
        let text = &content[parsed.spans["TIMEOUT"].clone()];
        assert!(
            text.contains("name: TIMEOUT"),
            "span={:?} text={text:?} expected_start={}",
            parsed.spans["TIMEOUT"],
            content.find("name: TIMEOUT").unwrap()
        );
        assert!(text.contains("value: \"10\""));
        assert!(!text.contains("OTHER"));
        assert!(!text.contains("image:"));
        let no_newline = content.trim_end();
        assert!(deployment(no_newline, "app").is_ok());
    }

    #[test]
    fn ambiguous_dynamic_or_external_values_never_resolve() {
        for content in [
            YAML.replace("value: \"10\"", "value: \"$(OTHER)\""),
            YAML.replace(
                "value: \"10\"",
                "valueFrom: {secretKeyRef: {name: secret, key: token}}",
            ),
            YAML.replace(
                "value: \"10\"",
                "value: \"10\"\n              value: \"20\"",
            ),
            YAML.replace("value: \"10\"", "value: &value \"10\""),
            YAML.replace("value: \"10\"", "value: 10"),
            YAML.replace(
                "image: app:v1",
                "envFrom: [{configMapRef: {name: app}}]\n          image: app:v1",
            ),
            format!("{YAML}---\n{YAML}"),
            YAML.replace("value: \"10\"", "value: {{ .Values.timeout }}"),
        ] {
            assert!(deployment(&content, "app").is_err(), "{content}");
        }
    }
}
