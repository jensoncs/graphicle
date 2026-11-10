//! Shared base/head import components. Both incoming and outgoing dependencies
//! participate; removed edges on one side cannot hide an old consumer.
use super::*;

type Package = (String, String);

pub(super) struct Components {
    pub component: BTreeMap<Package, usize>,
    pub files: BTreeMap<(String, String), BTreeSet<usize>>,
    pub owned_files: BTreeMap<String, BTreeSet<usize>>,
    pub hashes: Vec<String>,
}

impl Components {
    pub fn new(a: &Analysis, b: &Analysis) -> Self {
        let mut neighbors: BTreeMap<Package, BTreeSet<Package>> = a
            .packages
            .iter()
            .chain(&b.packages)
            .map(|p| {
                (
                    (s(p, "consumer_build").to_owned(), s(p, "path").to_owned()),
                    BTreeSet::new(),
                )
            })
            .collect();
        for p in a.packages.iter().chain(&b.packages) {
            let owner = (s(p, "consumer_build").to_owned(), s(p, "path").to_owned());
            for dependency in p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let dep = (owner.0.clone(), dependency.to_owned());
                if neighbors.contains_key(&dep) {
                    neighbors.get_mut(&owner).unwrap().insert(dep.clone());
                    neighbors.get_mut(&dep).unwrap().insert(owner.clone());
                }
            }
        }
        let mut component = BTreeMap::new();
        let mut count = 0;
        for package in neighbors.keys() {
            if component.contains_key(package) {
                continue;
            }
            let mut pending = vec![package.clone()];
            while let Some(node) = pending.pop() {
                if component.contains_key(&node) {
                    continue;
                }
                component.insert(node.clone(), count);
                pending.extend(neighbors[&node].iter().cloned());
            }
            count += 1;
        }
        let mut owned_files: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        let mut fingerprints = vec![Vec::new(); count];
        let mut files: BTreeMap<(String, String), BTreeSet<usize>> = BTreeMap::new();
        for (side, analysis) in [("base", a), ("head", b)] {
            for p in &analysis.packages {
                let package = (s(p, "consumer_build").to_owned(), s(p, "path").to_owned());
                let i = component[&package];
                fingerprints[i].push(json!([side, package, p["content_instance"]]));
                for file in p["selected_sources"].as_array().into_iter().flatten() {
                    if p["analysis_role"] == "owned" {
                        owned_files
                            .entry(s(file, "file").to_owned())
                            .or_default()
                            .insert(i);
                    }
                    files
                        .entry((package.0.clone(), s(file, "file").to_owned()))
                        .or_default()
                        .insert(i);
                }
            }
        }
        let hashes: Vec<_> = fingerprints
            .into_iter()
            .map(|mut values| {
                values.sort_by_key(Value::to_string);
                crate::digest(&json!(["review-component-v1", values]))
            })
            .collect();
        Self {
            component,
            files,
            owned_files,
            hashes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis(packages: Vec<Value>) -> Analysis {
        Analysis {
            package_diagnostics: BTreeMap::new(),
            diagnostics: vec![],
            diagnostic_count: 0,
            symbols: BTreeMap::new(),
            calls: vec![],
            packages,
            issues: vec![],
            analyzer: String::new(),
            toolchain: String::new(),
            inputs: vec![],
            raw: None,
            package_cache: crate::review_cache::PackageResult::default(),
            services: services::Links::default(),
        }
    }
    fn package(world: &str, path: &str, dependencies: &[&str], role: &str) -> Value {
        json!({"consumer_build":world,"path":path,"dependencies":dependencies,
            "analysis_role":role,"content_instance":path,
            "selected_sources":[{"file":format!("{world}/{path}.go")}]})
    }

    #[test]
    fn components_preserve_old_edges_build_worlds_and_owned_sources() {
        let base = analysis(vec![
            package("app", "consumer", &["supplier"], "owned"),
            package("app", "supplier", &[], "owned"),
            package("app", "vendor", &[], "dependency"),
            package("other", "supplier", &[], "owned"),
        ]);
        let mut head = analysis(base.packages.clone());
        head.packages[0]["dependencies"] = json!([]);
        // The removed file remains owned via the base side.
        head.packages[1]["selected_sources"] = json!([]);
        let c = Components::new(&base, &head);
        assert_eq!(
            c.owned_files["app/consumer.go"],
            c.owned_files["app/supplier.go"]
        );
        assert!(c.owned_files["app/supplier.go"].is_disjoint(&c.owned_files["other/supplier.go"]));
        assert!(!c.owned_files.contains_key("app/vendor.go"));
        assert!(
            c.files
                .contains_key(&("app".into(), "app/vendor.go".into()))
        );
        head.packages.reverse();
        assert_eq!(c.hashes, Components::new(&base, &head).hashes);
    }
}
