//! Precise TypeScript/JavaScript callers for basic review impact. The oxc
//! binder resolves modules over the pinned snapshot's own bytes (nothing on
//! disk, no `node_modules`) and binds calls to the review's declarations: a
//! method on a receiver typed by an annotation or made by `new`, and `new`
//! itself, which the name-based binder leaves unresolved.
use super::basic::{edge, line_span, script_source, symbol_key};
use super::*;
use graphicle::extractors::precise_js::{Nodes, bind_snapshot};
use std::path::PathBuf;
use std::sync::Arc;

/// Above this, a repository's sources are not bound precisely, and the review
/// says so. GitNexus's 3,752 sources (42 MiB) cost 0.4 s. A repository gets
/// here only within the import graph's file limit.
const BYTE_LIMIT: usize = 64 << 20;

/// The review declarations of one repository, as (file, symbol key).
struct Declared<'a> {
    repo: &'a str,
    keys: &'a BTreeSet<String>,
}

impl Declared<'_> {
    fn get(&self, file: &str, name: &str) -> Option<(String, String)> {
        let path = format!("{}/{file}", self.repo);
        let key = symbol_key(&path, name);
        self.keys.contains(&key).then_some((path, key))
    }
}

// ponytail: lines are ignored; a name declared twice in a file has no review
// node, so a name is unique per file. Match lines if nested names collide.
impl Nodes for Declared<'_> {
    type Id = (String, String);
    fn function(&self, file: &str, name: &str, _: usize) -> Option<Self::Id> {
        self.get(file, name)
    }
    fn class(&self, file: &str, name: &str, _: usize) -> Option<Self::Id> {
        self.get(file, name)
    }
    fn method(&self, (path, class): &Self::Id, name: &str) -> Option<Self::Id> {
        let key = format!("{class}.{name}");
        self.keys.contains(&key).then(|| (path.clone(), key))
    }
}

/// What module resolution reads besides sources.
fn resolution_config(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "package.json"
        || (name.ends_with(".json")
            && (name.starts_with("tsconfig") || name.starts_with("jsconfig")))
}

/// `may_call` edges for calls in `repo` the name-based binder did not find.
pub(super) fn calls(input: &Input, repo: &str, declared: &BTreeSet<String>, out: &mut Analysis) {
    let prefix = format!("{repo}/");
    // A virtual root: the resolver only sees these files.
    let root = Path::new("/snapshot").join(repo);
    let (mut sources, mut bytes) = (0, 0);
    let mut files: Vec<(PathBuf, Arc<str>)> = vec![];
    for (path, source) in input.sources.range(prefix.clone()..) {
        let Some(rel) = path.strip_prefix(&prefix) else {
            break;
        };
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let Some(text) = source.content.text() else {
            continue;
        };
        if script_source(ext) {
            sources += 1;
            bytes += text.len();
        } else if !resolution_config(rel) {
            continue;
        }
        files.push((root.join(rel), Arc::from(text)));
    }
    if sources == 0 {
        return;
    }
    if bytes > BYTE_LIMIT {
        out.issues.push(issue(
            "precise_js_budget",
            json!({"directory":repo,"files":sources,"bytes":bytes,"byte_limit":BYTE_LIMIT}),
        ));
        return;
    }
    let mut pairs: BTreeSet<(String, String)> = out
        .calls
        .iter()
        .filter(|(_, c)| c.relation == "may_call")
        .map(|(_, c)| (c.caller.clone(), c.target.clone()))
        .collect();
    for bound in bind_snapshot(
        &files,
        &root,
        &Declared {
            repo,
            keys: declared,
        },
    ) {
        let ((from, caller), (to, target)) = (bound.caller, bound.target);
        if !pairs.insert((caller.clone(), target.clone())) {
            continue;
        }
        let source = input.sources[&from].content.text().unwrap_or("");
        edge(
            out,
            (&from, &caller),
            (&to, &target),
            "may_call",
            bound.resolution,
            bound.line,
            line_span(source, bound.line),
        );
    }
}
