//! Select compiler working directories from admitted module files, not suffixes
//! found anywhere below an otherwise non-Go repository root.
use super::{Input, basic, issue};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) struct Scope {
    pub roots: Vec<Value>,
    pub issues: Vec<Value>,
}

fn within(path: &str, directory: &str) -> bool {
    path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|p| p.starts_with('/'))
}

// go list ./... does not traverse these directory names. An explicitly selected
// root is allowed; only descendants are filtered here.
fn ignored(relative: &str) -> bool {
    relative.split('/').any(|part| {
        part == "vendor" || part == "testdata" || part.starts_with('.') || part.starts_with('_')
    })
}

pub(super) fn scope(input: &Input) -> Result<Scope> {
    let mut out = Scope {
        roots: vec![],
        issues: vec![],
    };
    for pin in &input.pins {
        let modules: BTreeSet<_> = input
            .sources
            .keys()
            .filter_map(|path| {
                let directory = path.strip_suffix("/go.mod")?;
                (within(directory, &pin.directory) && basic::language(input, path) != Some("helm"))
                    .then_some(directory)
            })
            .collect();
        let mut selected = BTreeSet::new();
        for requested in &pin.modules {
            let directory = if requested == "." {
                pin.directory.clone()
            } else {
                format!("{}/{requested}", pin.directory)
            };
            if modules.iter().any(|module| within(&directory, module)) {
                selected.insert(directory.clone());
            }
            // A repository can have nested modules but no top-level module.
            // Each module needs its own invocation: ./... skips nested modules.
            for module in &modules {
                if let Some(relative) = module.strip_prefix(&format!("{directory}/"))
                    && !ignored(relative)
                {
                    selected.insert((*module).to_string());
                }
            }
        }
        if selected.len() > 64 {
            return Err(Error::bad(
                "Go module discovery exceeds 64 roots per repository; narrow modules",
            ));
        }
        for root in &selected {
            let module = root
                .strip_prefix(&format!("{}/", pin.directory))
                .unwrap_or(".");
            out.roots
                .push(json!({"directory":pin.directory,"module":module}));
        }
        let mut missing_module = vec![];
        let mut outside_scope = vec![];
        for path in input
            .sources
            .keys()
            .filter(|p| within(p, &pin.directory) && p.ends_with(".go"))
        {
            if basic::language(input, path) == Some("helm") {
                continue;
            }
            if selected.iter().any(|root| within(path, root)) {
                continue;
            }
            let directory = path.rsplit_once('/').map(|(p, _)| p).unwrap();
            let relative = directory
                .strip_prefix(&format!("{}/", pin.directory))
                .unwrap_or("");
            if ignored(relative) {
                continue;
            }
            if modules.iter().any(|module| within(path, module)) {
                outside_scope.push(path);
            } else {
                missing_module.push(path);
            }
        }
        for (reason, files) in [
            ("go_source_without_module", missing_module),
            ("go_source_outside_module_scope", outside_scope),
        ] {
            if !files.is_empty() {
                out.issues.push(issue(reason,json!({"directory":pin.directory,"files":files.len(),"examples":files.iter().take(8).collect::<Vec<_>>()})));
            }
        }
    }
    Ok(out)
}
