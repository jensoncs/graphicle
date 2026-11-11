//! Call binding over any set of declarations ([`Nodes`]): graph nodes for the
//! extractor, review declarations for a pinned snapshot ([`bind_snapshot`]).
use super::{Caller, Export, MAX_HOPS, Ref, Summary, Target, is_js, resolver_with};
use crate::extractors::snapshot_fs::SnapshotFs;
use oxc_resolver::{FileSystem, ResolverGeneric};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The declarations calls bind to, found by file key, name and line.
pub trait Nodes {
    type Id: Clone + PartialEq;
    fn function(&self, file: &str, name: &str, line: usize) -> Option<Self::Id>;
    fn class(&self, file: &str, name: &str, line: usize) -> Option<Self::Id>;
    /// Method `name` declared on `class` itself (bases are walked here).
    fn method(&self, class: &Self::Id, name: &str) -> Option<Self::Id>;
}

/// A call from `caller` to `target` at `line` of the file keyed `file`.
#[derive(Debug)]
pub struct Bound<Id> {
    pub caller: Id,
    pub target: Id,
    pub file: String,
    pub line: usize,
    /// `oxc_binding` (scope and module resolution) or `oxc_type` (a receiver
    /// typed by an annotation or `new`).
    pub resolution: &'static str,
}

pub(super) struct Resolve<'a, Fs: FileSystem + 'static> {
    pub(super) summaries: &'a [Summary],
    pub(super) by_path: FxHashMap<PathBuf, usize>,
    pub(super) resolver: ResolverGeneric<Fs>,
    /// The scan root's tsconfig.json, for files whose nearest tsconfig does
    /// not carry the project's `paths` (what `tsc -p <root>` sees).
    pub(super) root_resolver: Option<ResolverGeneric<Fs>>,
    pub(super) modules: FxHashMap<(usize, String), Option<usize>>,
    /// How a resolved path is matched against `by_path`.
    pub(super) canonical: fn(&Path) -> PathBuf,
}

impl<Fs: FileSystem + 'static> Resolve<'_, Fs> {
    fn module(&mut self, from: usize, spec: &str) -> Option<usize> {
        let key = (from, spec.to_string());
        if let Some(hit) = self.modules.get(&key) {
            return *hit;
        }
        let dir = &self.summaries[from].dir;
        let found = std::iter::once(&self.resolver)
            .chain(self.root_resolver.as_ref())
            .find_map(|resolver| {
                let resolved = resolver.resolve(dir, spec).ok()?;
                self.by_path
                    .get(&(self.canonical)(resolved.path()))
                    .copied()
            });
        self.modules.insert(key, found);
        found
    }
    /// The local declaration `name` exported from `file` resolves to.
    fn export(&mut self, file: usize, name: &str, hops: usize) -> Option<(usize, Ref)> {
        if hops > MAX_HOPS {
            return None;
        }
        let summaries = self.summaries;
        match summaries[file].exports.get(name) {
            Some(Export::Local(r @ Ref::Local { .. })) => Some((file, r.clone())),
            Some(Export::Local(Ref::Import { spec, name })) | Some(Export::From { spec, name }) => {
                let target = self.module(file, spec)?;
                self.export(target, name, hops + 1)
            }
            None if name != "default" => {
                for star in &summaries[file].stars {
                    if let Some(target) = self.module(file, star)
                        && let Some(found) = self.export(target, name, hops + 1)
                    {
                        return Some(found);
                    }
                }
                None
            }
            None => None,
        }
    }
    fn declaration(&mut self, file: usize, r: &Ref) -> Option<(usize, Ref)> {
        match r {
            Ref::Local { .. } => Some((file, r.clone())),
            Ref::Import { spec, name } => {
                let target = self.module(file, spec)?;
                self.export(target, name, 0)
            }
        }
    }
}

/// Every call in a file `emit` accepts whose caller and callee are `nodes`.
pub(super) fn bind<Fs: FileSystem + 'static, N: Nodes>(
    r: &mut Resolve<Fs>,
    emit: impl Fn(&str) -> bool,
    nodes: &N,
) -> Vec<Bound<N::Id>> {
    let summaries = r.summaries;
    let mut out = vec![];
    for (fi, summary) in summaries.iter().enumerate() {
        if !emit(&summary.key) {
            continue;
        }
        for site in &summary.sites {
            let caller = site.callers.iter().find_map(|c| match c {
                Caller::Function { name, line } => nodes.function(&summary.key, name, *line),
                Caller::Method {
                    class,
                    class_line,
                    name,
                } => nodes
                    .class(&summary.key, class, *class_line)
                    .and_then(|c| nodes.method(&c, name)),
                Caller::Value { name, line } => nodes.class(&summary.key, name, *line),
            });
            let Some(caller) = caller else { continue };
            let (target, resolution) = match &site.target {
                Target::Function(reference) => {
                    let Some((file, Ref::Local { name, line })) = r.declaration(fi, reference)
                    else {
                        continue;
                    };
                    (
                        nodes.function(&summaries[file].key, &name, line),
                        "oxc_binding",
                    )
                }
                Target::Namespace { spec, name } => {
                    let Some(module) = r.module(fi, spec) else {
                        continue;
                    };
                    let Some((file, Ref::Local { name, line })) = r.export(module, name, 0) else {
                        continue;
                    };
                    (
                        nodes.function(&summaries[file].key, &name, line),
                        "oxc_binding",
                    )
                }
                Target::New(class) => {
                    let Some((file, Ref::Local { name, line })) = r.declaration(fi, class) else {
                        continue;
                    };
                    (
                        nodes.class(&summaries[file].key, &name, line),
                        "oxc_binding",
                    )
                }
                Target::Member { class, name, typed } => (
                    method(nodes, r, fi, class, name),
                    if *typed { "oxc_type" } else { "oxc_binding" },
                ),
                Target::Method { class, name } => {
                    (method(nodes, r, fi, class, name), "oxc_binding")
                }
            };
            let Some(target) = target else { continue };
            if target == caller {
                continue;
            }
            out.push(Bound {
                caller,
                target,
                file: summary.key.clone(),
                line: site.line,
                resolution,
            });
        }
    }
    out
}

/// Method `name` of the class `reference` (from file `file`) or its bases.
fn method<Fs: FileSystem + 'static, N: Nodes>(
    nodes: &N,
    r: &mut Resolve<Fs>,
    file: usize,
    reference: &Ref,
    name: &str,
) -> Option<N::Id> {
    let (mut file, mut class) = r.declaration(file, reference)?;
    for _ in 0..MAX_HOPS {
        let Ref::Local {
            name: class_name,
            line,
        } = &class
        else {
            return None;
        };
        let summary = &r.summaries[file];
        let node = nodes.class(&summary.key, class_name, *line)?;
        if let Some(m) = nodes.method(&node, name) {
            return Some(m);
        }
        let base = summary
            .classes
            .iter()
            .find(|c| &c.name == class_name && c.line == *line)?
            .base
            .clone()?;
        (file, class) = r.declaration(file, &base)?;
    }
    None
}

/// Calls among the JS/TS files of a pinned snapshot, keyed by path relative to
/// `root`. `files` is the whole snapshot: its tsconfig.json and package.json
/// files steer module resolution, and nothing outside it is read.
pub fn bind_snapshot<N: Nodes>(
    files: &[(PathBuf, Arc<str>)],
    root: &Path,
    nodes: &N,
) -> Vec<Bound<N::Id>> {
    let texts: FxHashMap<&Path, &str> = files.iter().map(|(p, t)| (p.as_path(), &**t)).collect();
    let keyed: Vec<(PathBuf, String)> = files
        .iter()
        .filter(|(p, _)| is_js(p))
        .filter_map(|(p, _)| {
            let key = crate::fsutil::rel_string(p.strip_prefix(root).ok()?);
            Some((p.clone(), key))
        })
        .collect();
    let summaries = super::summarize_all(&keyed, |path, key| {
        super::summarize_source(path, texts.get(path)?, key)
    });
    let index: FxHashMap<&str, usize> = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| (s.key.as_str(), i))
        .collect();
    let by_path = keyed
        .iter()
        .filter_map(|(p, key)| Some((p.clone(), *index.get(key.as_str())?)))
        .collect();
    let fs = SnapshotFs::from_files(files.iter().cloned());
    let root_tsconfig = root.join("tsconfig.json");
    let mut r = Resolve {
        summaries: &summaries,
        by_path,
        resolver: resolver_with(fs.clone(), None),
        root_resolver: texts
            .contains_key(root_tsconfig.as_path())
            .then(|| resolver_with(fs, Some(root_tsconfig))),
        modules: FxHashMap::default(),
        canonical: Path::to_path_buf,
    };
    bind(&mut r, |_| true, nodes)
}

#[cfg(test)]
mod tests {
    use super::{Nodes, bind_snapshot};
    use rustc_hash::FxHashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// The declared `file#name` ids; methods are `file#Class.name`.
    struct Names(FxHashSet<String>);
    impl Names {
        fn get(&self, id: String) -> Option<String> {
            self.0.contains(&id).then_some(id)
        }
    }
    impl Nodes for Names {
        type Id = String;
        fn function(&self, file: &str, name: &str, _: usize) -> Option<String> {
            self.get(format!("{file}#{name}"))
        }
        fn class(&self, file: &str, name: &str, _: usize) -> Option<String> {
            self.get(format!("{file}#{name}"))
        }
        fn method(&self, class: &String, name: &str) -> Option<String> {
            self.get(format!("{class}.{name}"))
        }
    }

    #[test]
    fn binds_snapshot_calls_through_imports_classes_and_types() {
        // A virtual root: nothing here exists on disk.
        let root = Path::new("/snapshot/repo");
        let files: Vec<(PathBuf, Arc<str>)> = [
            (
                "src/lib/math.ts",
                "export function add(a: number, b: number) { return a + b; }\nexport const twice = (x: number) => add(x, x);\n",
            ),
            (
                "src/lib/index.ts",
                "export * from './math';\nexport { add as plus } from './math';\n",
            ),
            (
                "src/shapes.ts",
                "export class Shape {\n  area(): number { return 0; }\n  describe() { return this.area(); }\n}\nexport class Square extends Shape {\n  area() { return super.area() + 1; }\n  static unit() { return new Square(); }\n}\n",
            ),
            (
                "src/main.ts",
                "import * as lib from './lib';\nimport { plus as sum } from './lib';\nimport { Square, Shape } from './shapes';\n\nexport function run(shape: Shape) {\n  const sq = new Square();\n  sq.describe();\n  shape.area();\n  Square.unit();\n  return lib.twice(sum(1, 2));\n}\n",
            ),
        ]
        .into_iter()
        .map(|(p, t)| (root.join(p), Arc::<str>::from(t)))
        .collect();
        let names = Names(
            [
                "src/lib/math.ts#add",
                "src/lib/math.ts#twice",
                "src/shapes.ts#Shape",
                "src/shapes.ts#Shape.area",
                "src/shapes.ts#Shape.describe",
                "src/shapes.ts#Square",
                "src/shapes.ts#Square.area",
                "src/shapes.ts#Square.unit",
                "src/main.ts#run",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        let bound = bind_snapshot(&files, root, &names);
        let has = |caller: &str, target: &str, resolution: &str| {
            bound
                .iter()
                .any(|b| b.caller == caller && b.target == target && b.resolution == resolution)
        };
        let main = "src/main.ts#run";
        for (target, resolution) in [
            ("src/lib/math.ts#twice", "oxc_binding"),
            ("src/lib/math.ts#add", "oxc_binding"),
            ("src/shapes.ts#Square", "oxc_binding"),
            // An instance from `new` is a binding; an annotation is a type.
            ("src/shapes.ts#Shape.describe", "oxc_binding"),
            ("src/shapes.ts#Shape.area", "oxc_type"),
            ("src/shapes.ts#Square.unit", "oxc_binding"),
        ] {
            assert!(has(main, target, resolution), "{target}: {bound:#?}");
        }
        assert!(has(
            "src/lib/math.ts#twice",
            "src/lib/math.ts#add",
            "oxc_binding"
        ));
        assert!(has(
            "src/shapes.ts#Shape.describe",
            "src/shapes.ts#Shape.area",
            "oxc_binding"
        ));
        assert!(has(
            "src/shapes.ts#Square.area",
            "src/shapes.ts#Shape.area",
            "oxc_binding"
        ));
        let call = bound
            .iter()
            .find(|b| b.target == "src/lib/math.ts#twice")
            .unwrap();
        assert_eq!((call.file.as_str(), call.line), ("src/main.ts", 10));
    }
}
