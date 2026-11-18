//! Call binding over any set of declarations ([`Nodes`]): graph nodes for the
//! extractor, review declarations for a pinned snapshot ([`bind_snapshot`]).
use super::{
    Caller, ClassInfo, Export, MAX_HOPS, Recv, Ref, Summary, Target, is_js,
};
use crate::extractors::snapshot_fs::SnapshotFs;
use oxc_resolver::{Alias, AliasValue, FileSystem, ResolveOptions, ResolverGeneric};
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
        let (file, declaration) = self.located(file, name, hops)?;
        Some((file, declaration?))
    }
    /// The file `name` exported from `file` is defined in, with its local
    /// declaration when it has one: a whole module (`k: require(m)`) is the
    /// module, its default declaration when that resolves, and a value
    /// written in place has none.
    fn located(&mut self, file: usize, name: &str, hops: usize) -> Option<(usize, Option<Ref>)> {
        if hops > MAX_HOPS {
            return None;
        }
        let summaries = self.summaries;
        match summaries[file].exports.get(name) {
            Some(Export::Local(r @ Ref::Local { .. })) => Some((file, Some(r.clone()))),
            Some(Export::Inline) => Some((file, None)),
            Some(Export::Module(spec)) => {
                let target = self.module(file, spec)?;
                Some(self.whole(target, hops))
            }
            Some(Export::Local(Ref::Import { spec, name })) | Some(Export::From { spec, name }) => {
                let target = self.module(file, spec)?;
                if name == "default" {
                    return Some(self.whole(target, hops));
                }
                self.located(target, name, hops + 1)
            }
            None if name != "default" => {
                for star in &summaries[file].stars {
                    if let Some(target) = self.module(file, star)
                        && let Some(found) = self.located(target, name, hops + 1)
                    {
                        return Some(found);
                    }
                }
                None
            }
            None => None,
        }
    }
    /// The export keys of `file` but `default`, with those of the modules it
    /// re-exports whole (`export *`, `...require(m)`), sorted.
    fn keys(&mut self, file: usize, hops: usize) -> std::collections::BTreeSet<String> {
        let summaries = self.summaries;
        let mut out: std::collections::BTreeSet<String> = summaries[file]
            .exports
            .keys()
            .filter(|k| *k != "default")
            .cloned()
            .collect();
        if hops < MAX_HOPS {
            for star in &summaries[file].stars {
                if let Some(target) = self.module(file, star) {
                    out.extend(self.keys(target, hops + 1));
                }
            }
        }
        out
    }
    /// Module `file` as one value: its default declaration, else the module.
    fn whole(&mut self, file: usize, hops: usize) -> (usize, Option<Ref>) {
        self.located(file, "default", hops + 1)
            .filter(|(_, declaration)| declaration.is_some())
            .unwrap_or((file, None))
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

/// What an export key resolves to: a declaration's node, or the file (key)
/// that defines it when it has no declaration of its own.
#[derive(Debug, PartialEq)]
pub(super) enum Exported<Id> {
    Node(Id),
    File(String),
}

/// The declaration each export key of a file `emit` accepts resolves to,
/// through `require` and re-export hops (`module.exports = { k: v, ...m }`,
/// `export { a as b } from`): `(file key, export key, target)`. Only
/// keys a declaration's own node does not already carry: renamed, or
/// declared in another file. A key with no declaration node (a whole
/// module, a value written in place) targets the file that defines it.
pub(super) fn exported<Fs: FileSystem + 'static, N: Nodes>(
    r: &mut Resolve<Fs>,
    emit: impl Fn(&str) -> bool,
    nodes: &N,
) -> Vec<(String, String, Exported<N::Id>)> {
    let summaries = r.summaries;
    let mut out = vec![];
    for (fi, summary) in summaries.iter().enumerate() {
        if !emit(&summary.key) {
            continue;
        }
        for key in r.keys(fi, 0) {
            let key = &key;
            let Some((file, declaration)) = r.located(fi, key, 0) else {
                continue;
            };
            let owner = &summaries[file].key;
            let target = match &declaration {
                Some(Ref::Local { name, .. }) if file == fi && name == key => continue,
                Some(Ref::Local { name, line }) => nodes
                    .function(owner, name, *line)
                    .or_else(|| nodes.class(owner, name, *line))
                    .map(Exported::Node),
                _ => None,
            };
            out.push((
                summary.key.clone(),
                key.clone(),
                target.unwrap_or_else(|| Exported::File(owner.clone())),
            ));
        }
    }
    out
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
                Target::Function(reference) | Target::Render(reference) => {
                    let Some((file, Ref::Local { name, line })) = r.declaration(fi, reference)
                    else {
                        continue;
                    };
                    (
                        nodes.function(&summaries[file].key, &name, line),
                        if matches!(site.target, Target::Render(_)) { "oxc_jsx" } else { "oxc_binding" },
                    )
                }
                Target::Namespace { spec, name } => {
                    let Some(module) = r.module(fi, spec) else {
                        continue;
                    };
                    match r.export(module, name, 0) {
                        Some((file, Ref::Local { name, line })) => (
                            nodes.function(&summaries[file].key, &name, line),
                            "oxc_binding",
                        ),
                        // CommonJS `module.exports = C`: `m.f()` is C's static method
                        _ => {
                            let default = Ref::Import {
                                spec: spec.clone(),
                                name: "default".into(),
                            };
                            (method(nodes, r, fi, &default, name), "oxc_binding")
                        }
                    }
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
                Target::Chain { receiver, name } => {
                    let Some((file, class)) = class_of(r, fi, receiver) else {
                        continue;
                    };
                    (method(nodes, r, file, &class, name), "oxc_type")
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

/// The declaration (file, local class) of the class `recv`, read in file
/// `file`, holds.
fn class_of<Fs: FileSystem + 'static>(
    r: &mut Resolve<Fs>,
    file: usize,
    recv: &Recv,
) -> Option<(usize, Ref)> {
    let (file, class) = match recv {
        Recv::Class(class) => (file, class.clone()),
        Recv::Call(target) => returned(r, file, target)?,
        Recv::Field(inner, field) => {
            let (file, class) = class_of(r, file, inner)?;
            member_of(r, file, class, |c| c.fields.get(field).cloned())?
        }
        Recv::Api(api) => api_type(r, file, api)?,
        Recv::Hook(hook) => {
            let (file, name) = local(r, file, hook)?;
            let api = r.summaries[file].api_hooks.get(&name)?.clone();
            api_type(r, file, &api)?
        }
    };
    r.declaration(file, &class)
}

/// The file and local name a reference read in `file` is declared as.
fn local<Fs: FileSystem + 'static>(r: &mut Resolve<Fs>, file: usize, reference: &Ref) -> Option<(usize, String)> {
    match r.declaration(file, reference)? {
        (file, Ref::Local { name, .. }) => Some((file, name)),
        _ => None,
    }
}

/// The type `T` of `const api = createApiRef<T>(..)`, with the file it is read in.
fn api_type<Fs: FileSystem + 'static>(r: &mut Resolve<Fs>, file: usize, api: &Ref) -> Option<(usize, Ref)> {
    let (file, name) = local(r, file, api)?;
    Some((file, r.summaries[file].api_refs.get(&name)?.clone()))
}

/// The one class in the snapshot that `implements` the interface `iface`
/// (declared in `file`); a class is itself.
// ponytail: scans every class per call; index implementers if snapshots with
// many `implements` make binding slow.
fn implementer<Fs: FileSystem + 'static>(r: &mut Resolve<Fs>, file: usize, iface: Ref) -> Option<(usize, Ref)> {
    let summaries = r.summaries;
    let is = |c: &ClassInfo, f: usize| f == file && Ref::Local { name: c.name.clone(), line: c.line } == iface;
    if summaries[file].classes.iter().any(|c| is(c, file)) {
        return Some((file, iface));
    }
    let mut found = None;
    for (f, summary) in summaries.iter().enumerate() {
        for c in &summary.classes {
            if c.implements.iter().any(|i| r.declaration(f, i) == Some((file, iface.clone()))) {
                if found.is_some() {
                    return None;
                }
                found = Some((f, Ref::Local { name: c.name.clone(), line: c.line }));
            }
        }
    }
    found
}

/// The class a method call `target` made from file `file` is declared to
/// return, with the file its reference is read in.
fn returned<Fs: FileSystem + 'static>(
    r: &mut Resolve<Fs>,
    file: usize,
    target: &Target,
) -> Option<(usize, Ref)> {
    let (file, class, name) = match target {
        Target::Member { class, name, .. } | Target::Method { class, name } => {
            let (file, class) = r.declaration(file, class)?;
            (file, class, name)
        }
        Target::Chain { receiver, name } => {
            let (file, class) = class_of(r, file, receiver)?;
            (file, class, name)
        }
        _ => return None,
    };
    member_of(r, file, class, |c| c.returns.get(name).cloned())
}

/// The class a member of local class `class` (in file `file`) or of its
/// nearest base declaring it is typed as: `pick` reads a class's member,
/// `Some(None)` when declared without a known class.
fn member_of<Fs: FileSystem + 'static>(
    r: &mut Resolve<Fs>,
    mut file: usize,
    mut class: Ref,
    pick: impl Fn(&ClassInfo) -> Option<Option<Ref>>,
) -> Option<(usize, Ref)> {
    for _ in 0..MAX_HOPS {
        let Ref::Local {
            name: class_name,
            line,
        } = &class
        else {
            return None;
        };
        let info = r.summaries[file]
            .classes
            .iter()
            .find(|c| &c.name == class_name && c.line == *line)?;
        // The nearest declaration of the member decides, typed or not.
        if let Some(found) = pick(info) {
            return Some((file, found?));
        }
        let base = info.base.clone()?;
        (file, class) = r.declaration(file, &base)?;
    }
    None
}

/// Method `name` of the class `reference` (from file `file`) or its bases.
fn method<Fs: FileSystem + 'static, N: Nodes>(
    nodes: &N,
    r: &mut Resolve<Fs>,
    file: usize,
    reference: &Ref,
    name: &str,
) -> Option<N::Id> {
    let (file, class) = r.declaration(file, reference)?;
    let (mut file, mut class) = implementer(r, file, class)?;
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

/// Each snapshot package.json `name`, resolving to its directory: what a
/// workspace's `node_modules` symlinks would, which a snapshot does not have.
fn workspace_packages(files: &[(PathBuf, Arc<str>)]) -> Alias {
    files
        .iter()
        .filter(|(p, _)| p.ends_with("package.json") && !p.to_string_lossy().contains("/node_modules/"))
        .filter_map(|(p, text)| {
            let json: serde_json::Value = serde_json::from_str(text).ok()?;
            let name = json.get("name")?.as_str()?.to_string();
            Some((name, vec![AliasValue::Path(p.parent()?.to_string_lossy().into_owned())]))
        })
        .collect()
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
    let packages = workspace_packages(files);
    let resolver = |tsconfig| {
        ResolverGeneric::new_with_file_system(
            fs.clone(),
            ResolveOptions { fallback: packages.clone(), ..super::resolve_options(tsconfig) },
        )
    };
    let mut r = Resolve {
        summaries: &summaries,
        by_path,
        resolver: resolver(None),
        root_resolver: texts
            .contains_key(root_tsconfig.as_path())
            .then(|| resolver(Some(root_tsconfig))),
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

    /// atlas-backstage: components reach API clients through `useApi(ref)` or
    /// a hook wrapping it, and the ref names an interface a class implements.
    #[test]
    fn binds_api_ref_receivers_to_the_class_implementing_the_interface() {
        let root = Path::new("/snapshot/repo");
        let files: Vec<(PathBuf, Arc<str>)> = [
            (
                "api/iam.ts",
                "import { createApiRef, useApi } from '@backstage/core-plugin-api';\nexport interface IamType {\n  addKey(id: string): Promise<void>;\n}\nclass Base {}\nexport class Iam extends Base implements IamType {\n  async addKey(id: string) {}\n}\nexport const iamRef = createApiRef<IamType>({ id: 'iam' });\nexport const useIam = () => useApi(iamRef);\n",
            ),
            ("api/index.ts", "export * from './iam';\n"),
            (
                "ui/Keys.tsx",
                "import { useApi } from '@backstage/core-plugin-api';\nimport { iamRef, useIam, IamType } from '../api';\nexport const Keys = () => {\n  const iam = useApi(iamRef);\n  const viaHook = useIam();\n  const enable = async () => { await iam.addKey('a'); };\n  return viaHook.addKey('b');\n};\nexport function direct(api: IamType) { return api.addKey('c'); }\n",
            ),
        ]
        .into_iter()
        .map(|(p, t)| (root.join(p), Arc::<str>::from(t)))
        .collect();
        let names = Names(
            [
                "api/iam.ts#Iam",
                "api/iam.ts#Iam.addKey",
                "api/iam.ts#IamType",
                "ui/Keys.tsx#Keys",
                "ui/Keys.tsx#Keys.enable",
                "ui/Keys.tsx#direct",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        let bound = bind_snapshot(&files, root, &names);
        let lines: Vec<(&str, usize)> = bound
            .iter()
            .filter(|b| b.target == "api/iam.ts#Iam.addKey" && b.resolution == "oxc_type")
            .map(|b| (b.caller.as_str(), b.line))
            .collect();
        assert_eq!(
            lines,
            [("ui/Keys.tsx#Keys", 6), ("ui/Keys.tsx#Keys", 7), ("ui/Keys.tsx#direct", 9)],
            "{bound:#?}"
        );
    }

    /// A component rendered as a JSX element is called by its parent
    /// (atlas-backstage #1044: DeploymentTabs renders the changed
    /// DeleteDeploymentModal at line 358; the block showed its imports only).
    #[test]
    fn binds_jsx_elements_to_the_components_they_render() {
        let root = Path::new("/snapshot/repo");
        let files: Vec<(PathBuf, Arc<str>)> = [
            ("ui/DeleteModal.tsx", "export const DeleteModal = ({ id }: { id: string }) => <div>{id}</div>;\n"),
            (
                "ui/Tabs.tsx",
                "import { DeleteModal } from './DeleteModal';\nfunction Row() { return <span />; }\nexport const Tabs = () => {\n  return (\n    <div>\n      <Row />\n      <DeleteModal id=\"x\" />\n    </div>\n  );\n};\n",
            ),
        ]
        .into_iter()
        .map(|(p, t)| (root.join(p), Arc::<str>::from(t)))
        .collect();
        let names = Names(
            ["ui/DeleteModal.tsx#DeleteModal", "ui/Tabs.tsx#Row", "ui/Tabs.tsx#Tabs"].into_iter().map(String::from).collect(),
        );
        let bound = bind_snapshot(&files, root, &names);
        let mut found: Vec<(&str, &str, usize, &str)> =
            bound.iter().map(|b| (b.caller.as_str(), b.target.as_str(), b.line, b.resolution)).collect();
        found.sort();
        // oxc_jsx, not oxc_binding: a parent rendering a changed component is a
        // consumer, not a required caller (atlas-backstage #1046: RootPage and
        // IAMHomePage pushed out the client methods the change calls).
        assert_eq!(
            found,
            [("ui/Tabs.tsx#Tabs", "ui/DeleteModal.tsx#DeleteModal", 7, "oxc_jsx"), ("ui/Tabs.tsx#Tabs", "ui/Tabs.tsx#Row", 6, "oxc_jsx")],
            "{bound:#?}"
        );
    }

    /// A yarn workspace package is imported by its package.json name; the
    /// snapshot has no `node_modules` symlinks to reach it through.
    #[test]
    fn binds_calls_into_workspace_packages_by_name() {
        let root = Path::new("/snapshot/repo");
        let files: Vec<(PathBuf, Arc<str>)> = [
            (
                "plugins/svc/package.json",
                r#"{"name": "@internal/plugin-svc", "main": "src/index.ts", "publishConfig": {"main": "dist/index.js"}}"#,
            ),
            ("plugins/svc/src/index.ts", "export * from './api';\n"),
            ("plugins/svc/src/api.ts", "export class Api {\n  get() {}\n}\n"),
            (
                "plugins/ui/src/Page.tsx",
                "import { Api } from '@internal/plugin-svc';\nexport function page(api: Api) { return api.get(); }\n",
            ),
        ]
        .into_iter()
        .map(|(p, t)| (root.join(p), Arc::<str>::from(t)))
        .collect();
        let names = Names(
            ["plugins/svc/src/api.ts#Api", "plugins/svc/src/api.ts#Api.get", "plugins/ui/src/Page.tsx#page"]
                .into_iter()
                .map(String::from)
                .collect(),
        );
        let bound = bind_snapshot(&files, root, &names);
        assert!(
            bound.iter().any(|b| b.caller == "plugins/ui/src/Page.tsx#page" && b.target == "plugins/svc/src/api.ts#Api.get"),
            "{bound:#?}"
        );
    }

    #[test]
    fn binds_commonjs_require_and_module_exports() {
        let root = Path::new("/snapshot/repo");
        let files: Vec<(PathBuf, Arc<str>)> = [
            (
                "src/util.js",
                "function add(a, b) { return a + b; }\nfunction twice(x) { return add(x, x); }\nmodule.exports = { add, double: twice };\n",
            ),
            (
                "src/store.js",
                "class Store {\n  get(k) { return k; }\n  static open() { return new Store(); }\n}\nmodule.exports = Store;\n",
            ),
            (
                "src/fmt.js",
                "function pad(s) { return s; }\nfunction trim(s) { return s; }\nexports.pad = pad;\nmodule.exports.trim = trim;\n",
            ),
            (
                "src/main.js",
                "const util = require('./util');\nconst { double: dbl } = require('./util');\nconst Store = require('./store');\nconst fmt = require('./fmt.js');\nfunction run() {\n  util.add(1, 2);\n  dbl(3);\n  const s = new Store();\n  s.get('k');\n  Store.open();\n  fmt.pad('x');\n  return fmt.trim('y');\n}\nmodule.exports = { run };\n",
            ),
            (
                // A local `require` is not CommonJS: nothing binds through it.
                "src/shadow.js",
                "function require(x) { return x; }\nconst util = require('./util');\nfunction other() { return util.add(1, 2); }\n",
            ),
        ]
        .into_iter()
        .map(|(p, t)| (root.join(p), Arc::<str>::from(t)))
        .collect();
        let names = Names(
            [
                "src/util.js#add",
                "src/util.js#twice",
                "src/store.js#Store",
                "src/store.js#Store.get",
                "src/store.js#Store.open",
                "src/fmt.js#pad",
                "src/fmt.js#trim",
                "src/main.js#run",
                "src/shadow.js#other",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        );
        let bound = bind_snapshot(&files, root, &names);
        let has = |caller: &str, target: &str| {
            bound
                .iter()
                .any(|b| b.caller == caller && b.target == target)
        };
        for target in [
            "src/util.js#add",
            // `double` is the exported name; the declaration is `twice`.
            "src/util.js#twice",
            "src/store.js#Store",
            "src/store.js#Store.get",
            "src/store.js#Store.open",
            "src/fmt.js#pad",
            "src/fmt.js#trim",
        ] {
            assert!(has("src/main.js#run", target), "{target}: {bound:#?}");
        }
        assert!(!has("src/shadow.js#other", "src/util.js#add"), "{bound:#?}");
    }
}
