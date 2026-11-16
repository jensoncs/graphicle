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
    /// A top-level `data` entry being read: key, start, value seen.
    data_open: Option<(String, Marker, bool)>,
}
impl MarkedEventReceiver for Guard {
    fn on_event(&mut self, event: Event, marker: Marker) {
        self.events += 1;
        if self.invalid || self.events > 20_000 || self.frames.len() > 64 {
            self.invalid = true;
            return;
        }
        // A `data` entry ends where the next event starts.
        if self.data_open.as_ref().is_some_and(|open| open.2)
            && let Some((key, start, _)) = self.data_open.take()
        {
            self.spans.insert(vec!["data".into(), key], (start, marker));
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
                if let Some(frame) = self.frames.pop()
                    && frame.mapping
                    && frame.path.len() > 1
                    && frame.path[frame.path.len() - 2] == "env"
                {
                    self.spans
                        .insert(frame.path, (frame.start.unwrap(), marker));
                }
            }
            Event::Scalar(text, _, anchor, tag) => {
                self.invalid |= anchor != 0 || tag.is_some();
                if let Some(parent) = self.frames.last_mut().filter(|f| f.mapping) {
                    if parent.path.len() == 1 && parent.path[0] == "data" {
                        if parent.key {
                            self.data_open = Some((text.clone(), marker, false));
                        } else if let Some(open) = self.data_open.as_mut() {
                            open.2 = true;
                        }
                    }
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
    /// Env names whose value is a ConfigMap entry: (ConfigMap, data key).
    pub references: BTreeMap<String, (String, String)>,
    /// Env names set outside the repository (`secretKeyRef`, `fieldRef`,
    /// `resourceFieldRef`): a canonical description of the reference and the
    /// env entry's span. There is no value; the entry stays in `masked`, so a
    /// changed reference is a changed document.
    pub opaque: BTreeMap<String, (String, std::ops::Range<usize>)>,
    /// ConfigMaps the workload uses other than through these references
    /// (volumes, `envFrom`, other containers): their other keys have readers
    /// this model does not see.
    pub config_maps_elsewhere: BTreeSet<String>,
}

/// A ConfigMap's literal `data`, cited like deployment values.
pub struct ConfigMapData {
    pub name: String,
    pub namespace: String,
    pub values: BTreeMap<String, String>,
    pub spans: BTreeMap<String, std::ops::Range<usize>>,
    /// Full document with just the `data` values erased.
    pub masked: Value,
}

/// A Kubernetes object name (DNS-1123 subdomain).
fn resource_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
}
/// A ConfigMap data key.
fn data_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}
fn config_map_names(value: &Value, out: &mut BTreeMap<String, usize>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if matches!(
                    key.as_str(),
                    "configMap" | "configMapRef" | "configMapKeyRef"
                ) && let Some(name) = child["name"].as_str()
                {
                    *out.entry(name.to_owned()).or_default() += 1;
                }
                config_map_names(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| config_map_names(item, out)),
        _ => {}
    }
}
fn line_starts(content: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            content
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect()
}
// yaml-rust2's absolute index mixes character and byte increments on
// different scanner paths. Lines with ASCII prefixes give an unambiguous
// byte column; decline ambiguous inline Unicode instead of mis-citing it.
fn byte_offset(content: &str, offsets: &[usize], marker: &Marker) -> Result<usize, &'static str> {
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
}
fn span(
    content: &str,
    offsets: &[usize],
    guard: &Guard,
    path: &[String],
) -> Result<std::ops::Range<usize>, &'static str> {
    let (start, end) = guard
        .spans
        .get(path)
        .ok_or("environment_span_unavailable")?;
    let start = byte_offset(content, offsets, start)?;
    let end = byte_offset(content, offsets, end)?;
    if start >= end || content.get(start..end).is_none() {
        return Err("environment_span_unavailable");
    }
    Ok(start..end)
}
/// The bytes outside the erased spans. Retain exact non-value bytes as well as
/// structure: YAML scalar coercions or parser normalization must never erase
/// another change.
fn outside(
    content: &str,
    spans: &BTreeMap<String, std::ops::Range<usize>>,
) -> Result<String, &'static str> {
    let mut ranges: Vec<_> = spans.values().cloned().collect();
    ranges.sort_by_key(|span| span.start);
    let mut out = String::new();
    let mut cursor = 0;
    for span in ranges {
        if span.start < cursor {
            return Err("environment_span_ambiguous");
        }
        out.push_str(&content[cursor..span.start]);
        cursor = span.end;
    }
    out.push_str(&content[cursor..]);
    Ok(out)
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
    let mut config_map_uses = BTreeMap::new();
    config_map_names(&document, &mut config_map_uses);
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
    let mut references = BTreeMap::<String, (String, String)>::new();
    let mut opaque = BTreeMap::new();
    let mut spans = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let offsets = line_starts(content);
    for (index, item) in env.iter_mut().enumerate() {
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
        let object = item
            .as_object_mut()
            .ok_or("environment_declaration_invalid")?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| key(s))
            .ok_or("environment_key_unresolved")?
            .to_owned();
        if !seen.insert(name.clone()) {
            return Err("environment_key_ambiguous");
        }
        if object.len() == 2
            && let Some(source) = object.get("valueFrom").and_then(external_source)
        {
            opaque.insert(name, (source, span(content, &offsets, &guard, &path)?));
            continue;
        }
        if object.len() == 2 && object.contains_key("valueFrom") {
            // Only an exact, required `configMapKeyRef` is a reference;
            // `optional`, secrets and field refs stay unresolved.
            let reference = object["valueFrom"]
                .as_object()
                .filter(|from| from.len() == 1)
                .and_then(|from| from.get("configMapKeyRef"))
                .and_then(Value::as_object)
                .filter(|r| r.len() == 2);
            let map = reference
                .and_then(|r| r.get("name"))
                .and_then(Value::as_str)
                .filter(|s| resource_name(s));
            let entry = reference
                .and_then(|r| r.get("key"))
                .and_then(Value::as_str)
                .filter(|s| data_key(s));
            let (Some(map), Some(entry)) = (map, entry) else {
                return Err("environment_value_from_unresolved");
            };
            references.insert(name, (map.to_owned(), entry.to_owned()));
            continue;
        }
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
        spans.insert(name.clone(), span(content, &offsets, &guard, &path)?);
        values.insert(name, value.to_owned());
        object.insert("value".into(), Value::Null);
    }
    let mut own = BTreeMap::<&str, usize>::new();
    for (map, _) in references.values() {
        *own.entry(map.as_str()).or_default() += 1;
    }
    let config_maps_elsewhere = config_map_uses
        .into_iter()
        .filter(|(map, uses)| own.get(map.as_str()) != Some(uses))
        .map(|(map, _)| map)
        .collect();
    let outside = outside(content, &spans)?;
    Ok(DeploymentEnvironment {
        values,
        spans,
        masked: serde_json::json!({"document":document,"outside_environment":outside}),
        workload,
        references,
        opaque,
        config_maps_elsewhere,
    })
}

/// A `valueFrom` that Kubernetes fills from outside the repository, as a
/// canonical description; `None` for anything else or a malformed reference.
fn external_source(value_from: &Value) -> Option<String> {
    let (kind, reference) = value_from.as_object()?.iter().next()?;
    let reference = reference.as_object()?;
    let required: &[&str] = match kind.as_str() {
        "secretKeyRef" => &["name", "key"],
        "fieldRef" => &["fieldPath"],
        "resourceFieldRef" => &["resource"],
        _ => return None,
    };
    let fixed = required.iter().all(|field| {
        reference
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    });
    let known = reference.keys().all(|field| {
        required.contains(&field.as_str())
            || matches!(
                field.as_str(),
                "optional" | "apiVersion" | "containerName" | "divisor"
            )
    });
    (value_from.as_object()?.len() == 1 && fixed && known)
        .then(|| serde_json::to_string(&serde_json::json!({ kind: reference })).unwrap())
}

pub fn config_map(content: &str) -> Result<ConfigMapData, &'static str> {
    let (mut document, guard) = document(content)?;
    if document["apiVersion"] != "v1"
        || document["kind"] != "ConfigMap"
        || document.get("binaryData").is_some()
    {
        return Err("environment_config_map_unsupported");
    }
    let name = document["metadata"]["name"]
        .as_str()
        .filter(|s| resource_name(s))
        .ok_or("environment_config_map_unsupported")?
        .to_owned();
    let namespace = document["metadata"]["namespace"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("environment_namespace_missing")?
        .to_owned();
    let data = document["data"]
        .as_object_mut()
        .ok_or("environment_config_map_unsupported")?;
    if data.len() > 1024 {
        return Err("environment_declaration_limit");
    }
    let offsets = line_starts(content);
    let mut values = BTreeMap::new();
    let mut spans = BTreeMap::new();
    for (entry, value) in data.iter_mut() {
        if !data_key(entry) {
            return Err("environment_config_map_unsupported");
        }
        let text = value.as_str().ok_or("environment_value_unresolved")?;
        if text.contains("{{") {
            return Err("environment_value_unresolved");
        }
        let path = ["data".to_owned(), entry.clone()];
        spans.insert(entry.clone(), span(content, &offsets, &guard, &path)?);
        values.insert(entry.clone(), text.to_owned());
        *value = Value::Null;
    }
    let outside = outside(content, &spans)?;
    Ok(ConfigMapData {
        name,
        namespace,
        values,
        spans,
        masked: serde_json::json!({"document":document,"outside_environment":outside}),
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

    const MAP: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n  namespace: test\ndata:\n  timeout: \"10\"\n  retries: \"3\"\n";
    const REF: &str = "valueFrom:\n                configMapKeyRef:\n                  name: settings\n                  key: timeout";

    #[test]
    fn config_map_key_refs_are_references_kept_in_the_masked_document() {
        let parsed = deployment(&YAML.replace("value: \"10\"", REF), "app").unwrap();
        assert!(parsed.values.is_empty());
        assert_eq!(
            parsed.references["TIMEOUT"],
            ("settings".to_owned(), "timeout".to_owned())
        );
        assert!(parsed.config_maps_elsewhere.is_empty());
        let other_key = deployment(
            &YAML.replace(
                "value: \"10\"",
                &REF.replace("key: timeout", "key: retries"),
            ),
            "app",
        )
        .unwrap();
        assert_ne!(parsed.masked, other_key.masked);
        let mounted = YAML.replace("value: \"10\"", REF).replace(
            "      containers:\n",
            "      volumes:\n        - name: conf\n          configMap:\n            name: settings\n      containers:\n",
        );
        let mounted = deployment(&mounted, "app").unwrap();
        assert!(mounted.config_maps_elsewhere.contains("settings"));
    }

    #[test]
    fn config_map_data_values_are_the_only_erased_information() {
        let a = config_map(MAP).unwrap();
        assert_eq!(
            (a.name.as_str(), a.namespace.as_str()),
            ("settings", "test")
        );
        assert_eq!(a.values["timeout"], "10");
        let text = &MAP[a.spans["timeout"].clone()];
        assert!(
            text.contains("timeout: \"10\"") && !text.contains("retries"),
            "{text:?}"
        );
        let b = config_map(&MAP.replace("\"10\"", "\"20\"")).unwrap();
        assert_eq!(a.masked, b.masked);
        assert_ne!(a.values, b.values);
        for (from, to) in [("retries", "attempts"), ("name: settings", "name: other")] {
            assert_ne!(a.masked, config_map(&MAP.replace(from, to)).unwrap().masked);
        }
        for content in [
            MAP.replace("\"3\"", "3"),
            MAP.replace("data:", "binaryData:\n  blob: AA==\ndata:"),
            MAP.replace("  namespace: test\n", ""),
            MAP.replace("kind: ConfigMap", "kind: Secret"),
            MAP.replace("\"10\"", "\"{{ .Values.timeout }}\""),
        ] {
            assert!(config_map(&content).is_err(), "{content}");
        }
    }

    #[test]
    fn secret_and_field_refs_are_opaque_inputs_cited_at_their_entry() {
        let content = YAML.to_owned()
            + "            - name: TOKEN\n              valueFrom: {secretKeyRef: {name: secret, key: token}}\n"
            + "            - name: HOST\n              valueFrom:\n                fieldRef:\n                  fieldPath: status.hostIP\n";
        let parsed = deployment(&content, "app").unwrap();
        assert_eq!(parsed.values.keys().collect::<Vec<_>>(), ["TIMEOUT"]);
        assert_eq!(parsed.opaque.keys().collect::<Vec<_>>(), ["HOST", "TOKEN"]);
        let token = &content[parsed.opaque["TOKEN"].1.clone()];
        assert!(
            token.contains("name: TOKEN") && token.contains("key: token"),
            "{token:?}"
        );
        assert!(
            !token.contains("HOST") && !token.contains("TIMEOUT"),
            "{token:?}"
        );
        assert!(parsed.opaque["TOKEN"].0.contains("secretKeyRef"));
        // The reference is configuration, so changing it is a change.
        let moved = deployment(&content.replace("key: token", "key: other"), "app").unwrap();
        assert_ne!(parsed.masked, moved.masked);
        assert_ne!(parsed.opaque["TOKEN"].0, moved.opaque["TOKEN"].0);
        let literal = deployment(&content.replace("\"10\"", "\"20\""), "app").unwrap();
        assert_eq!(parsed.masked, literal.masked);
    }

    #[test]
    fn ambiguous_dynamic_or_external_values_never_resolve() {
        for content in [
            YAML.replace("value: \"10\"", "value: \"$(OTHER)\""),
            YAML.replace("value: \"10\"", "valueFrom: {secretKeyRef: {name: secret}}"),
            YAML.replace("value: \"10\"", "valueFrom: {}"),
            YAML.replace(
                "value: \"10\"",
                "valueFrom: {secretKeyRef: {name: secret, key: token}, fieldRef: {fieldPath: x}}",
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
            YAML.replace(
                "value: \"10\"",
                &format!("{REF}\n                  optional: true"),
            ),
            YAML.replace(
                "value: \"10\"",
                &format!("value: \"10\"\n              {REF}"),
            ),
        ] {
            assert!(deployment(&content, "app").is_err(), "{content}");
        }
    }
}
