//! Conservative review equivalence for connected native package components.
//! Global graph completeness is unchanged. Unknown ownership blocks every proof;
//! package-local unresolved calls block only their connected component. Incoming
//! imports are included as well as dependencies, so changed consumers invalidate
//! a supplier's prior review too. No runtime target is inferred by this layer.
use super::*;

#[derive(Default)]
pub(super) struct Scopes {
    symbols: BTreeMap<String, String>,
    files: BTreeMap<String, BTreeSet<String>>,
    head_files: BTreeMap<String, String>,
    file_symbols: BTreeMap<String, usize>,
}
impl Scopes {
    fn keys<'a>(&'a self, collection: &str, entry: &Value) -> Vec<&'a String> {
        if collection == "entries" {
            self.symbols
                .get(s(entry, "symbol_id"))
                .into_iter()
                .collect()
        } else {
            self.files
                .get(s(entry, "file"))
                .into_iter()
                .flatten()
                .collect()
        }
    }
    pub(super) fn counts<'a>(
        &self,
        entries: impl IntoIterator<Item = (&'a str, &'a Value)>,
    ) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for (collection, entry) in entries {
            for scope in self.keys(collection, entry) {
                *counts.entry(scope.clone()).or_default() += 1;
            }
        }
        counts
    }
    pub(super) fn finish(
        &self,
        required: &BTreeMap<String, usize>,
        selected: &mut [(&str, Value)],
        report: &mut Value,
    ) {
        let actual = self.counts(
            selected
                .iter()
                .map(|(collection, entry)| (*collection, entry)),
        );
        let mut supplied_files: BTreeMap<&str, usize> = BTreeMap::new();
        for (collection, entry) in selected.iter().filter(|(_, e)| e["side"] == "head") {
            if *collection == "entries"
                && let Some(file) = self.head_files.get(s(entry, "symbol_id"))
            {
                *supplied_files.entry(file).or_default() += 1;
            }
        }
        let mut eligible = BTreeSet::new();
        for (collection, entry) in selected {
            if *collection != "entries" || entry["side"] != "head" {
                continue;
            }
            let scope = s(&entry["review_basis"], "scope");
            let full_file = self
                .head_files
                .get(s(entry, "symbol_id"))
                .is_some_and(|file| {
                    supplied_files.get(file.as_str()) == self.file_symbols.get(file)
                });
            if scope.is_empty() || required.get(scope) != actual.get(scope) || !full_file {
                entry["review_basis"]["complete"] = json!(false);
                entry["review_basis"]["reason"] = json!("context_omitted");
            }
            if entry["review_basis"]["complete"] == true {
                eligible.insert(s(entry, "id").to_owned());
            }
            // Packing may invalidate the proof without changing the bytes. Its
            // revision must change too, so a delta cannot retain a stale proof.
            entry["revision"] = json!(crate::digest(&json!([
                entry["revision"],
                entry["review_basis"]
            ])));
        }
        if let Some(ids) = report["review_reuse"]["eligible"].as_array_mut() {
            ids.retain(|id| id.as_str().is_some_and(|id| eligible.contains(id)));
        }
    }
}

pub(super) fn annotate(
    report: &mut Value,
    a: &Analysis,
    b: &Analysis,
    components: &components::Components,
) -> Scopes {
    let components::Components {
        component,
        files,
        hashes,
        ..
    } = components;
    let mut blocked: BTreeSet<usize> = BTreeSet::new();
    // Native package equivalence does not prove equivalent deployment wiring.
    // Boundary paths still participate in impact; review reuse stays disabled
    // until component proofs cover service inventories and runtime configuration.
    let mut global = a.services.active() || b.services.active();
    for issue in report["coverage"]["issues"]
        .as_array()
        .into_iter()
        .flatten()
    {
        if issue["reason"] == "unresolved_edge" {
            let detail = &issue["detail"];
            if let Some(owners) = files.get(&(
                s(detail, "consumer_build").to_owned(),
                s(detail, "file").to_owned(),
            )) {
                blocked.extend(owners.iter().copied());
            } else {
                global = true;
            }
        } else {
            global = true;
        }
    }
    // A non-package change may alter review assumptions outside compiler facts.
    // Until modeled, it is a global barrier rather than an inferred safe edit.
    let known_files: BTreeSet<_> = files.keys().map(|(_, file)| file.as_str()).collect();
    if report["file_changes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|f| !known_files.contains(s(f, "file")))
    {
        global = true;
    }
    let mut scopes = Scopes::default();
    for ((_, file), owners) in files {
        scopes
            .files
            .entry(file.clone())
            .or_default()
            .extend(owners.iter().map(|i| hashes[*i].clone()));
    }
    let mut file_symbols: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for analysis in [a, b] {
        for (id, (world, symbol)) in &analysis.symbols {
            if let Some(i) = component.get(&(world.clone(), symbol.package.clone())) {
                scopes.symbols.insert(id.clone(), hashes[*i].clone());
                scopes
                    .files
                    .entry(symbol.file.clone())
                    .or_default()
                    .insert(hashes[*i].clone());
            }
        }
    }
    for (id, (_, symbol)) in &b.symbols {
        scopes.head_files.insert(id.clone(), symbol.file.clone());
        *scopes.file_symbols.entry(symbol.file.clone()).or_default() += 1;
        file_symbols
            .entry(symbol.file.clone())
            .or_default()
            .insert(id.clone());
    }
    let supplied: BTreeSet<_> = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["side"] == "head")
        .map(|e| s(e, "symbol_id").to_owned())
        .collect();
    for entry in report["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|e| e["side"] == "head")
    {
        let id = s(entry, "symbol_id");
        let (world, symbol) = &b.symbols[id];
        let i = component.get(&(world.clone(), symbol.package.clone()));
        let reason = if global || i.is_none() {
            "unscoped_uncertainty"
        } else if i.is_some_and(|i| blocked.contains(i)) {
            "unresolved_component"
        } else if !file_symbols[&symbol.file].is_subset(&supplied) {
            "missing_file_declarations"
        } else {
            "complete_static_component"
        };
        entry["review_basis"] = json!({"scope":scopes.symbols.get(id),"complete":reason=="complete_static_component","reason":reason});
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_consumer_build_view_revokes_all_views_of_the_file() {
        let scopes = Scopes {
            symbols: BTreeMap::from([("one".into(), "a".into()), ("two".into(), "b".into())]),
            files: BTreeMap::new(),
            head_files: BTreeMap::from([
                ("one".into(), "file.go".into()),
                ("two".into(), "file.go".into()),
            ]),
            file_symbols: BTreeMap::from([("file.go".into(), 2)]),
        };
        let mut selected = vec![(
            "entries",
            json!({"id":"entry","side":"head","symbol_id":"one","revision":"old","review_basis":{"scope":"a","complete":true}}),
        )];
        let mut report = json!({"review_reuse":{"eligible":["entry"]}});
        scopes.finish(
            &BTreeMap::from([("a".into(), 1), ("b".into(), 1)]),
            &mut selected,
            &mut report,
        );
        assert_eq!(selected[0].1["review_basis"]["complete"], false);
        assert_eq!(report["review_reuse"]["eligible"], json!([]));
        assert_ne!(selected[0].1["revision"], "old");
    }
}
