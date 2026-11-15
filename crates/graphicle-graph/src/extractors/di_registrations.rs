//! Awilix container registrations (`container.register('eventService',
//! asClass(require('../services/event-tracking-service')))`), resolved to the
//! repository classes they register. In CLASSIC injection mode the container
//! fills a registered class's constructor parameters by name, so a parameter
//! named like a registration key holds an instance of that key's class.
use super::receiver_bindings::{FileFacts, Registered};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[derive(Default)]
pub struct Registry {
    /// Registration key to the class ids registered under it.
    pub by_key: FxHashMap<String, Vec<String>>,
    /// Classes the container constructs.
    pub classes: FxHashSet<String>,
}

/// `a/b/../c` to `a/c`, without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Class ids declared in the file a relative specifier names, from the file `from`.
fn required<'a>(
    from: &str,
    spec: &str,
    classes_in: &'a FxHashMap<PathBuf, Vec<String>>,
) -> &'a [String] {
    if !spec.starts_with('.') {
        return &[];
    }
    let base = normalize(&Path::new(from).parent().unwrap_or(Path::new("")).join(spec));
    let ext = |e: &str| PathBuf::from(format!("{}{e}", base.display()));
    [
        base.clone(),
        ext(".js"),
        ext(".ts"),
        ext(".jsx"),
        ext(".tsx"),
        base.join("index.js"),
        base.join("index.ts"),
    ]
    .iter()
    .find_map(|p| classes_in.get(p))
    .map_or(&[], Vec::as_slice)
}

/// Resolve every file's registrations. `classes` maps a class id to its label
/// and source file. A key registered to different classes keeps them all, so
/// the call stays ambiguous.
pub fn build(
    facts: &BTreeMap<String, Option<FileFacts>>,
    classes: &[(String, String, String)],
) -> Registry {
    let mut classes_in: FxHashMap<PathBuf, Vec<String>> = FxHashMap::default();
    let mut label_in: FxHashMap<(PathBuf, &str), Vec<String>> = FxHashMap::default();
    for (id, label, file) in classes {
        let file = normalize(Path::new(file));
        classes_in.entry(file.clone()).or_default().push(id.clone());
        label_in
            .entry((file, label.as_str()))
            .or_default()
            .push(id.clone());
    }
    let mut registry = Registry::default();
    for (sf, f) in facts {
        let Some(f) = f else { continue };
        for (key, registered) in &f.registrations {
            let ids: Vec<String> = match registered {
                Registered::Require(spec) => required(sf, spec, &classes_in).to_vec(),
                Registered::Name(name) => match f.requires.get(name) {
                    Some(spec) => {
                        let ids = required(sf, spec, &classes_in);
                        // A file of several classes: the one the name says.
                        let named: Vec<String> = ids
                            .iter()
                            .filter(|id| classes.iter().any(|(i, l, _)| i == *id && l == name))
                            .cloned()
                            .collect();
                        if named.is_empty() {
                            ids.to_vec()
                        } else {
                            named
                        }
                    }
                    None => label_in
                        .get(&(normalize(Path::new(sf)), name.as_str()))
                        .cloned()
                        .unwrap_or_default(),
                },
            };
            for id in ids {
                registry.classes.insert(id.clone());
                let list = registry.by_key.entry(key.clone()).or_default();
                if !list.contains(&id) {
                    list.push(id);
                }
            }
        }
    }
    registry
}
