//! Conservative bindings within an authorized source manifest. No files outside
//! that manifest are consulted, and unresolved receivers remain unresolved.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}
fn visible(scope: &str, caller: &str) -> bool {
    scope.is_empty()
        || caller == scope
        || caller
            .strip_prefix(scope)
            .is_some_and(|r| r.starts_with('.'))
}

/// Strip JSONC comments/trailing commas without touching quoted content.
pub(crate) fn configuration(content: &str) -> Option<Value> {
    let chars: Vec<_> = content.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            } else if c == '"' {
                quoted = false;
            }
        } else if c == '"' {
            quoted = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push('\n');
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 == chars.len() {
                return None;
            }
            i += 1;
            out.push(' ');
        } else if c == ','
            && chars[i + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|c| matches!(c, '}' | ']'))
        {
        } else {
            out.push(c);
        }
        i += 1;
    }
    if quoted {
        return None;
    }
    // A second pass also handles a comment between a trailing comma and `}`.
    if let Ok(value) = serde_json::from_str::<Value>(&out) {
        return Some(value);
    }
    if out != content {
        configuration(&out)
    } else {
        None
    }
}

fn join(parent: &str, child: &str) -> Option<String> {
    crate::structure::normalize(&Path::new(parent).join(child).to_string_lossy())
}
fn parent(path: &str) -> &str {
    Path::new(path)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or("")
}

pub(crate) fn javascript_targets(base: &str, paths: &BTreeSet<String>) -> BTreeSet<String> {
    if paths.contains(base) {
        return BTreeSet::from([base.to_string()]);
    }
    let mut roots = vec![base.to_string()];
    for suffix in [".js", ".jsx", ".mjs", ".cjs"] {
        if let Some(root) = base.strip_suffix(suffix) {
            roots.push(root.to_string());
        }
    }
    let mut targets = BTreeSet::new();
    for root in roots {
        for suffix in ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"] {
            for path in [format!("{root}.{suffix}"), format!("{root}/index.{suffix}")] {
                if paths.contains(&path) {
                    targets.insert(path);
                }
            }
        }
    }
    targets
}

#[derive(Default)]
struct Paths {
    base: Option<String>,
    entries: BTreeMap<String, Vec<String>>,
}
fn load_paths(
    path: &str,
    analyses: &serde_json::Map<String, Value>,
    visiting: &mut BTreeSet<String>,
) -> Option<Paths> {
    if visiting.len() >= 8 || !visiting.insert(path.to_string()) {
        return None;
    }
    let config = analyses.get(path)?.get("module_configuration")?;
    let mut result = Paths::default();
    if let Some(extends) = config["extends"].as_str() {
        // Package-based tsconfig inheritance requires a package resolver.
        if !extends.starts_with('.') {
            return None;
        }
        let mut base = join(parent(path), extends)?;
        if !base.ends_with(".json") {
            base.push_str(".json");
        }
        result = load_paths(&base, analyses, visiting)?;
    }
    if let Some(base) = config["compilerOptions"]["baseUrl"].as_str() {
        result.base = Some(join(parent(path), base)?);
    }
    if let Some(entries) = config["compilerOptions"]["paths"].as_object() {
        if entries.len() > 128 {
            return None;
        }
        result.entries.clear();
        for (pattern, values) in entries {
            if pattern.matches('*').count() > 1 {
                continue;
            }
            let values: Vec<_> = values
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(16)
                .filter_map(|value| join(result.base.as_deref().unwrap_or(parent(path)), value))
                .collect();
            result.entries.insert(pattern.clone(), values);
        }
    }
    visiting.remove(path);
    Some(result)
}

/// The manifest's `tsconfig.json`/`jsconfig.json` files, each parsed once,
/// deepest first. Built once per graph: a per-import scan of every path was
/// quadratic in the repository's size.
pub(crate) struct JsConfigs {
    /// By directory; `jsconfig.json` wins a directory that has both, as it
    /// sorts first.
    configs: HashMap<String, Option<Paths>>,
}

impl JsConfigs {
    pub(crate) fn new(analyses: &serde_json::Map<String, Value>, paths: &BTreeSet<String>) -> Self {
        let mut configs = HashMap::new();
        for path in paths.iter().filter(|path| {
            matches!(
                Path::new(path).file_name().and_then(|s| s.to_str()),
                Some("tsconfig.json" | "jsconfig.json")
            )
        }) {
            configs
                .entry(parent(path).to_string())
                .or_insert_with(|| load_paths(path, analyses, &mut BTreeSet::new()));
        }
        Self { configs }
    }

    /// The nearest config governing `source`, and its parsed paths: one
    /// lookup per ancestor directory, however many configs the manifest has.
    fn governing(&self, source: &str) -> Option<&Option<Paths>> {
        ancestors(source).find_map(|dir| self.configs.get(dir))
    }
}

/// `source`'s directory, then each directory above it, ending with `""`.
fn ancestors(source: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(parent(source)), |dir| {
        (!dir.is_empty()).then(|| parent(dir))
    })
}

pub(crate) fn configured_targets(
    source: &str,
    module: &str,
    configs: &JsConfigs,
    paths: &BTreeSet<String>,
) -> BTreeSet<String> {
    let Some(Some(config)) = configs.governing(source) else {
        return BTreeSet::new();
    };
    let mut matches = Vec::new();
    for (pattern, bases) in &config.entries {
        let matched = if let Some((prefix, suffix)) = pattern.split_once('*') {
            module
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
                .map(|middle| (middle, prefix.len(), suffix.len()))
        } else if pattern == module {
            Some(("", usize::MAX, usize::MAX))
        } else {
            None
        };
        if let Some((middle, prefix, suffix)) = matched {
            matches.push(((prefix, suffix), middle, bases));
        }
    }
    matches.sort_by_key(|v| std::cmp::Reverse(v.0));
    if let Some((_, middle, bases)) = matches.first() {
        // Configured fallback targets are ordered, just as in TypeScript.
        for base in *bases {
            let targets = javascript_targets(&base.replace('*', middle), paths);
            if !targets.is_empty() {
                return targets;
            }
        }
        return BTreeSet::new();
    }
    config
        .base
        .as_deref()
        .and_then(|base| join(base, module))
        .map(|base| javascript_targets(&base, paths))
        .unwrap_or_default()
}

/// The manifest's JS workspaces: each root (a `package.json` with
/// `workspaces`, or a `pnpm-workspace.yaml`, which wins a directory that has
/// both, as in the graph engine) with its members by package name, `None` for
/// a name two members share. Built once per graph; a member is found under
/// its own ancestors, so building and lookups do not grow with the number of
/// roots.
pub(crate) struct Workspaces<'a> {
    roots: HashMap<&'a str, Members<'a>>,
}
/// Package name to its directory and `package.json`; `None` when shared.
type Members<'a> = HashMap<&'a str, Option<(&'a str, &'a Value)>>;
/// Globs past this many in one root are ignored.
const WORKSPACE_GLOBS: usize = 128;

impl<'a> Workspaces<'a> {
    pub(crate) fn new(analyses: &'a serde_json::Map<String, Value>) -> Self {
        let named = |file: &str, path: &str| {
            Path::new(path).file_name().and_then(|n| n.to_str()) == Some(file)
        };
        let mut globs: HashMap<&str, Vec<glob::Pattern>> = HashMap::new();
        for (path, analysis) in analyses {
            let listed = if named("pnpm-workspace.yaml", path) {
                &analysis["workspace_globs"]
            } else if named("package.json", path) {
                let w = &analysis["package_manifest"]["workspaces"];
                let pnpm = format!("{}pnpm-workspace.yaml", under(parent(path)));
                if w.is_null() || analyses.contains_key(&pnpm) {
                    continue; // a member, or a root pnpm's file defines
                }
                if w.is_array() { w } else { &w["packages"] }
            } else {
                continue;
            };
            let root = parent(path);
            let patterns = listed
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|glob| glob.trim_start_matches("./").trim_end_matches('/'))
                .filter(|glob| !glob.starts_with('!'))
                .take(WORKSPACE_GLOBS)
                .filter_map(|glob| {
                    let full = match (root, glob) {
                        (_, "" | ".") => root.to_string(),
                        ("", _) => glob.to_string(),
                        _ => format!("{root}/{glob}"),
                    };
                    glob::Pattern::new(&full).ok()
                })
                .collect();
            globs.insert(root, patterns);
        }
        let options = glob::MatchOptions {
            require_literal_separator: true,
            ..Default::default()
        };
        let mut roots: HashMap<&str, Members> =
            globs.keys().map(|r| (*r, HashMap::new())).collect();
        for (path, analysis) in analyses
            .iter()
            .filter(|(path, _)| named("package.json", path))
        {
            let manifest = &analysis["package_manifest"];
            let Some(name) = manifest["name"].as_str().filter(|n| !n.is_empty()) else {
                continue;
            };
            let dir = parent(path);
            // Every workspace root at or above the package that lists it.
            for root in std::iter::once(dir).chain(ancestors(dir)) {
                let (Some(patterns), Some(members)) = (globs.get(root), roots.get_mut(root)) else {
                    continue;
                };
                if patterns.iter().any(|p| p.matches_with(dir, options)) {
                    members
                        .entry(name)
                        .and_modify(|m| *m = None)
                        .or_insert(Some((dir, manifest)));
                }
            }
        }
        Self { roots }
    }

    /// A bare import of a member of `source`'s nearest workspace: the
    /// member's source entry for it, as the graph engine's on-disk resolver
    /// picks it (a build-output file only when no source file is offered).
    pub(crate) fn targets(
        &self,
        source: &str,
        module: &str,
        paths: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let Some(members) = ancestors(source).find_map(|dir| self.roots.get(dir)) else {
            return BTreeSet::new();
        };
        // A package name is `@scope/name` or `name`; the rest is a subpath.
        let segments = if module.starts_with('@') { 2 } else { 1 };
        let mut parts = module.splitn(segments + 1, '/');
        let name_len = parts.by_ref().take(segments).map(str::len).sum::<usize>() + segments - 1;
        let (name, sub) = (
            &module[..name_len.min(module.len())],
            parts.next().unwrap_or(""),
        );
        let Some(Some((dir, manifest))) = members.get(name) else {
            return BTreeSet::new();
        };
        let dir = *dir;
        let within = |p: &str| dir.is_empty() || p == dir || p.starts_with(&format!("{dir}/"));
        let inside = |p: &Path| {
            crate::structure::normalize(&p.to_string_lossy()).is_some_and(|p| within(&p))
        };
        let platform = graphicle::extractors::resolution_js::importer_platform(Path::new(source));
        let mut built = BTreeSet::new();
        for candidate in graphicle::extractors::resolution_js::entry_candidates(
            manifest,
            Path::new(dir),
            sub,
            platform,
            &inside,
        ) {
            let Some(base) = crate::structure::normalize(&candidate.to_string_lossy()) else {
                continue;
            };
            let found = javascript_targets(&base, paths);
            if found.is_empty() {
                continue;
            }
            let build_output = found.iter().all(|f| {
                f.strip_prefix(&format!("{dir}/"))
                    .unwrap_or(f)
                    .split('/')
                    .next()
                    .is_some_and(|first| {
                        graphicle::extractors::resolution_js::BUILD_OUTPUT_DIRS.contains(&first)
                    })
            });
            if !build_output {
                return found;
            }
            if built.is_empty() {
                built = found;
            }
        }
        built
    }
}

fn under(dir: &str) -> String {
    if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    }
}

/// Nearest ancestor directory of `source` (walking up to the manifest root)
/// holding a `package.json` or `pnpm-workspace.yaml`, mirroring Graphify's
/// `_find_js_project_anchor`. Falls back to the importer's own directory.
fn js_project_anchor(source: &str, paths: &BTreeSet<String>) -> String {
    let start = parent(source).to_string();
    let mut current = start.clone();
    loop {
        for marker in ["package.json", "pnpm-workspace.yaml"] {
            let candidate = if current.is_empty() {
                marker.to_string()
            } else {
                format!("{current}/{marker}")
            };
            if paths.contains(&candidate) {
                return current;
            }
        }
        if current.is_empty() {
            break;
        }
        current = parent(&current).to_string();
    }
    start
}

/// Unmapped `@/` project-root convention fallback (Graphify #3357). Active only
/// when no `tsconfig.json`/`jsconfig.json` governs `source`: `@/x` resolves to
/// `<anchor>/src/x` when the anchor has a `src/` directory, else `<anchor>/x`.
pub(crate) fn convention_alias_targets(
    source: &str,
    module: &str,
    configs: &JsConfigs,
    paths: &BTreeSet<String>,
) -> BTreeSet<String> {
    let Some(subpath) = module.strip_prefix("@/") else {
        return BTreeSet::new();
    };
    if subpath.is_empty() || configs.governing(source).is_some() {
        return BTreeSet::new();
    }
    let anchor = js_project_anchor(source, paths);
    let under = |dir: &str| {
        if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        }
    };
    let src = format!("{}src", under(&anchor));
    let under_src = format!("{src}/");
    if paths
        .range(under_src.clone()..)
        .next()
        .is_some_and(|path| path.starts_with(&under_src))
        && let Some(base) = join(&src, subpath)
    {
        let targets = javascript_targets(&base, paths);
        if !targets.is_empty() {
            return targets;
        }
    }
    join(&anchor, subpath)
        .map(|base| javascript_targets(&base, paths))
        .unwrap_or_default()
}

fn declaration(analysis: &Value, name: &str, caller: &str) -> Option<String> {
    innermost(
        items(analysis, "symbols")
            .iter()
            .filter(|v| {
                let full = s(v, "name");
                if full == name {
                    return true;
                }
                let (scope, local) = full.rsplit_once('.').unwrap_or(("", full));
                local == name && visible(scope, caller) && v["member"] != true
            })
            .collect(),
    )
}

/// The most deeply scoped of `matches`, unless two share that depth.
fn innermost(mut matches: Vec<&Value>) -> Option<String> {
    matches.sort_by_key(|v| std::cmp::Reverse(s(v, "name").matches('.').count()));
    let first = matches.first()?;
    if matches.get(1).is_some_and(|second| {
        s(second, "name").matches('.').count() == s(first, "name").matches('.').count()
    }) {
        return None;
    }
    Some(s(first, "name").to_string())
}

/// `from pkg import name` binds the submodule `pkg.name` only when the package
/// `__init__` leaves `name` unbound (importlib's `_handle_fromlist` imports the
/// submodule only for a missing attribute). That needs a complete `__init__`
/// analysis with no other top-level binding of `name`; `from . import name`
/// binds that same submodule.
pub(crate) fn leaves_submodule(init: &Value, name: &str) -> bool {
    if init["complete"] != true
        || declaration(init, name, "").is_some()
        || items(init, "shadowed_bindings")
            .iter()
            .any(|v| s(v, "scope").is_empty() && s(v, "name") == name)
    {
        return false;
    }
    items(init, "imports")
        .iter()
        .filter(|import| s(import, "scope").is_empty())
        .all(|import| {
            if items(import, "names").is_empty() {
                // `import a.b` binds `a`; `import a.b as c` binds `c`.
                let alias = s(import, "alias");
                let bound = if alias.is_empty() {
                    s(import, "module").split('.').next().unwrap_or("")
                } else {
                    alias
                };
                return bound != name;
            }
            match import["bindings"].get(name).and_then(Value::as_str) {
                None => true,
                Some(original) => {
                    import["level"] == 1 && s(import, "module").is_empty() && original == name
                }
            }
        })
}

/// Follow explicit named exports only. An incomplete/cyclic traversal cannot be
/// mistaken for a unique target, even if a different branch found a candidate.
fn exported(
    path: &str,
    name: &str,
    analyses: &serde_json::Map<String, Value>,
    import_edges: &ImportEdges,
    visiting: &mut BTreeSet<(String, String)>,
    remaining: &mut usize,
) -> Option<BTreeSet<(String, String)>> {
    let key = (path.to_string(), name.to_string());
    if *remaining == 0 || visiting.len() >= 16 || !visiting.insert(key.clone()) {
        return None;
    }
    *remaining -= 1;
    let analysis = analyses.get(path)?;
    if items(analysis, "shadowed_bindings")
        .iter()
        .any(|v| s(v, "scope").is_empty() && s(v, "name") == name)
    {
        return None;
    }
    let mut found = BTreeSet::new();
    if let Some(local) = declaration(analysis, name, "") {
        found.insert((path.to_string(), local));
    }
    for import in items(analysis, "imports") {
        if !s(import, "scope").is_empty()
            || import["type_only"] == true
            || (!path.ends_with(".py") && import["reexport"] != true)
        {
            continue;
        }
        let Some(original) = import["bindings"].get(name).and_then(Value::as_str) else {
            continue;
        };
        // A submodule edge binds a module object, never a symbol named `name`.
        let key = (path, import["line"].to_string(), s(import, "module"));
        for &(target, _) in import_edges
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|(_, submodule)| submodule.is_empty())
        {
            found.extend(exported(
                target,
                original,
                analyses,
                import_edges,
                visiting,
                remaining,
            )?);
        }
    }
    visiting.remove(&key);
    Some(found)
}

/// One file's symbols by name, as [`declaration`] filters them: a dotted name
/// matches a symbol by its full name, any other name a symbol by its last
/// segment whose scope the caller sees.
/// An import edge's file, line (as JSON text) and module.
type ImportKey<'a> = (&'a str, String, &'a str);
/// Each import's edges (target, submodule), by [`ImportKey`].
type ImportEdges<'a> = HashMap<ImportKey<'a>, Vec<(&'a str, &'a str)>>;

struct Declarations<'a> {
    full: HashMap<&'a str, Vec<&'a Value>>,
    local: HashMap<&'a str, Vec<(&'a str, &'a Value)>>,
}

impl<'a> Declarations<'a> {
    fn of(analysis: &'a Value) -> Self {
        let mut out = Self {
            full: HashMap::new(),
            local: HashMap::new(),
        };
        for symbol in items(analysis, "symbols") {
            let full = s(symbol, "name");
            out.full.entry(full).or_default().push(symbol);
            let (scope, local) = full.rsplit_once('.').unwrap_or(("", full));
            out.local.entry(local).or_default().push((scope, symbol));
        }
        out
    }
    /// [`declaration`] without scanning every symbol.
    fn find(&self, name: &str, caller: &str) -> Option<String> {
        // A last segment has no dot, so a dotted name only matches in full;
        // an undotted full match is a last-segment match at the top scope.
        let matches = if name.contains('.') {
            self.full.get(name).cloned().unwrap_or_default()
        } else {
            self.local
                .get(name)
                .into_iter()
                .flatten()
                .filter(|(scope, symbol)| visible(scope, caller) && symbol["member"] != true)
                .map(|(_, symbol)| *symbol)
                .collect()
        };
        innermost(matches)
    }
    /// The type whose `impl` (or trait) declares the Rust method `caller`.
    fn impl_type<'c>(&self, caller: &'c str) -> Option<&'c str> {
        let (parent, _) = caller.rsplit_once('.')?;
        self.full
            .get(caller)?
            .iter()
            .any(|symbol| symbol["member"] == true)
            .then_some(parent)
    }
    /// `self.m()` or `cls.m()` in Python, `this.m()` in JavaScript or
    /// TypeScript: the method `m` of the class the caller is defined in,
    /// when that class declares it and the receiver is not rebound. An
    /// inherited method, or an attribute's method (`self.repo.save()`),
    /// stays unresolved.
    fn receiver_method(
        &self,
        path: &str,
        name: &str,
        caller: &str,
        rebound: &HashMap<&str, Vec<&str>>,
    ) -> Option<String> {
        if path.ends_with(".rs") {
            // `self.m()` / `Self::m()` in a method: the method `m` of the
            // type its impl is for. A nested `fn` sees neither.
            let method = name
                .strip_prefix("self.")
                .or_else(|| name.strip_prefix("Self::"))?;
            if !identifier(method) {
                return None;
            }
            let full = format!("{}.{method}", self.impl_type(caller)?);
            let unique = self.full.get(full.as_str()).is_some_and(|v| v.len() == 1);
            return unique.then_some(full);
        }
        let (head, method) = name.split_once('.')?;
        let python = path.ends_with(".py") || path.ends_with(".pyi");
        let script = [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"]
            .iter()
            .any(|ext| path.ends_with(ext));
        let receiver = (python && matches!(head, "self" | "cls")) || (script && head == "this");
        if !receiver
            || !identifier(method)
            || rebound
                .get(head)
                .is_some_and(|scopes| scopes.iter().any(|scope| visible(scope, caller)))
        {
            return None;
        }
        // A Python closure keeps its method's `self`; a JavaScript
        // `function` rebinds `this`, so only a method's own body counts.
        let mut scope = caller;
        loop {
            let (parent, _) = scope.rsplit_once('.')?;
            let class = self
                .full
                .get(parent)
                .is_some_and(|symbols| symbols.iter().any(|symbol| symbol["kind"] == "class"));
            if class {
                let full = format!("{parent}.{method}");
                return self.full.contains_key(full.as_str()).then_some(full);
            }
            if !python {
                return None;
            }
            scope = parent;
        }
    }
}

/// One file's imports, each with its edges (target, submodule).
type FileImports<'a> = [(&'a Value, &'a [(&'a str, &'a str)])];

/// The type and method of a Rust call through a stated type: `Type::m`, or
/// `x.m` where `x` is bound once in the caller's scope and its code states
/// `Type`. `Self` is the type of the caller's impl.
fn rust_typed<'n>(
    name: &'n str,
    caller: &str,
    declarations: &Declarations<'_>,
    typed: &HashMap<&str, Vec<(&str, &str)>>,
    shadowed: &HashMap<&str, Vec<&str>>,
) -> Option<(String, &'n str)> {
    let (stated, method) = match name.split_once("::") {
        Some((head, method)) => (head, method),
        None => {
            let (head, method) = name.split_once('.')?;
            let visible_in =
                |scopes: &Vec<&str>| scopes.iter().filter(|scope| visible(scope, caller)).count();
            let types: Vec<&str> = typed
                .get(head)
                .into_iter()
                .flatten()
                .filter(|(scope, _)| visible(scope, caller))
                .map(|(_, stated)| *stated)
                .collect();
            if shadowed.get(head).map_or(0, visible_in) != 1 || types.len() != 1 {
                return None;
            }
            (types[0], method)
        }
    };
    let stated = if stated == "Self" {
        declarations.impl_type(caller)?
    } else {
        stated
    };
    let capitalised = stated.starts_with(|c: char| c.is_ascii_uppercase());
    (capitalised && identifier(stated) && identifier(method)).then(|| (stated.to_owned(), method))
}

/// `Type.method` declared once in `path`, else in the one file a visible
/// `use` brings `Type` from.
fn rust_member(
    path: &str,
    (stated, method): (String, &str),
    caller: &str,
    declarations: &Declarations<'_>,
    imports: &FileImports<'_>,
    analyses: &serde_json::Map<String, Value>,
) -> Option<(String, String)> {
    let local = format!("{stated}.{method}");
    if let Some(symbols) = declarations.full.get(local.as_str()) {
        return (symbols.len() == 1).then(|| (path.to_owned(), local));
    }
    let mut found = BTreeSet::new();
    for &(import, targets) in imports {
        let Some(original) = import["bindings"].get(&stated).and_then(Value::as_str) else {
            continue;
        };
        if !visible(s(import, "scope"), caller) {
            continue;
        }
        let member = format!("{original}.{method}");
        for &(target, _) in targets {
            let declared = analyses.get(target).is_some_and(|analysis| {
                items(analysis, "symbols")
                    .iter()
                    .filter(|symbol| s(symbol, "name") == member)
                    .count()
                    == 1
            });
            if declared {
                found.insert((target.to_owned(), member.clone()));
            }
        }
    }
    let mut found = found.into_iter();
    match (found.next(), found.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// A CommonJS `const ns = require('./m')` of a file whose `module.exports =
/// Name`: `ns()` and `new ns()` name `Name`, and `ns.m()` its member
/// `Name.m`, when `./m` declares it.
fn default_export_member(
    import: &Value,
    name: &str,
    target: &str,
    analyses: &serde_json::Map<String, Value>,
) -> Option<String> {
    let namespace = s(import, "namespace");
    let analysis = analyses.get(target)?;
    let default = s(analysis, "default_export");
    if namespace.is_empty() || default.is_empty() {
        return None;
    }
    let member = if name == namespace {
        default.to_owned()
    } else {
        let method = name.strip_prefix(namespace)?.strip_prefix('.')?;
        if method.contains('.') {
            return None;
        }
        format!("{default}.{method}")
    };
    items(analysis, "symbols")
        .iter()
        .any(|symbol| s(symbol, "name") == member)
        .then_some(member)
}

/// Resolve explicit imported callable names. Unknown object receiver types,
/// wildcard imports and shadowed identifiers never become guessed call edges.
pub(crate) fn resolve_calls(
    analyses: &serde_json::Map<String, Value>,
    edges: &[Value],
) -> (Vec<Value>, Vec<Value>) {
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    // An import's edges, by its file, line and module: computed once, not per
    // call (each call used to scan every edge for each visible import). The
    // line is keyed by its JSON text, which compares as `Value` equality does.
    let mut import_edges = ImportEdges::new();
    for edge in edges.iter().filter(|e| e["kind"] == "imports") {
        import_edges
            .entry((
                s(edge, "source"),
                edge["line"].to_string(),
                s(edge, "module"),
            ))
            .or_default()
            .push((s(edge, "target"), s(edge, "submodule")));
    }
    for (path, analysis) in analyses {
        let declarations = Declarations::of(analysis);
        let mut shadowed: HashMap<&str, Vec<&str>> = HashMap::new();
        // Names assigned directly (`self = other`), not through a member.
        let mut rebound: HashMap<&str, Vec<&str>> = HashMap::new();
        for binding in items(analysis, "shadowed_bindings") {
            shadowed
                .entry(s(binding, "name"))
                .or_default()
                .push(s(binding, "scope"));
            if s(binding, "kind") == "assignment" {
                rebound
                    .entry(s(binding, "name"))
                    .or_default()
                    .push(s(binding, "scope"));
            }
        }
        let mut typed: HashMap<&str, Vec<(&str, &str)>> = HashMap::new();
        for binding in items(analysis, "typed_bindings") {
            typed
                .entry(s(binding, "name"))
                .or_default()
                .push((s(binding, "scope"), s(binding, "type")));
        }
        let rust = path.ends_with(".rs");
        let imports: Vec<_> = items(analysis, "imports")
            .iter()
            .filter(|import| import["reexport"] != true && import["type_only"] != true)
            .map(|import| {
                let key = (
                    path.as_str(),
                    import["line"].to_string(),
                    s(import, "module"),
                );
                (
                    import,
                    import_edges.get(&key).map_or(&[][..], Vec::as_slice),
                )
            })
            .collect();
        for call in items(analysis, "calls") {
            let name = s(call, "callee");
            let caller = s(call, "caller");
            let head = name.split(['.', ':']).next().unwrap_or(name);
            let receiver = declarations.receiver_method(path, name, caller, &rebound);
            if rust
                && receiver.is_none()
                && let Some(stated) = rust_typed(name, caller, &declarations, &typed, &shadowed)
                && let Some((target, callee)) =
                    rust_member(path, stated, caller, &declarations, &imports, analyses)
            {
                resolved.push(json!({"path":path,"caller":caller,"callee_path":target,"callee":callee,"line":call["line"],
                    "basis":if target==*path{"unique_visible_declaration_candidate"}else{"explicit_import_binding"},"runtime_verified":false}));
                continue;
            }
            if receiver.is_none()
                && shadowed
                    .get(head)
                    .is_some_and(|scopes| scopes.iter().any(|scope| visible(scope, caller)))
            {
                unresolved.push(json!({"path":path,"caller":caller,"callee":name,"line":call["line"],"reason":"shadowed_or_dynamic_binding"}));
                continue;
            }
            let mut candidates = BTreeSet::new();
            let mut incomplete_export = false;
            if let Some(method) = receiver {
                candidates.insert((path.clone(), method));
            } else if let Some(local) = declarations.find(name, caller) {
                candidates.insert((path.clone(), local));
            }
            if candidates.is_empty() {
                for &(import, targets) in &imports {
                    if !visible(s(import, "scope"), caller) {
                        continue;
                    }
                    for &(target, submodule) in targets {
                        if let Some(found) = default_export_member(import, name, target, analyses) {
                            candidates.insert((target.to_string(), found));
                            continue;
                        }
                        let imported = if !submodule.is_empty() {
                            // `from pkg import sub` binds the submodule object, so
                            // `sub.fn()` (or an alias's) names `fn` inside it.
                            import["bindings"].as_object().and_then(|bindings| {
                                bindings.iter().find_map(|(local, original)| {
                                    (original.as_str() == Some(submodule))
                                        .then(|| name.strip_prefix(&format!("{local}.")))
                                        .flatten()
                                        .map(str::to_string)
                                })
                            })
                        } else {
                            import["bindings"]
                                .get(name)
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .or_else(|| {
                                    let namespace = if !s(import, "namespace").is_empty() {
                                        s(import, "namespace")
                                    } else if path.ends_with(".py")
                                        && items(import, "names").is_empty()
                                    {
                                        if !s(import, "alias").is_empty() {
                                            s(import, "alias")
                                        } else {
                                            s(import, "module")
                                        }
                                    } else {
                                        ""
                                    };
                                    if namespace.is_empty() {
                                        return None;
                                    }
                                    name.strip_prefix(&format!("{namespace}."))
                                        .or_else(|| name.strip_prefix(&format!("{namespace}::")))
                                        .map(str::to_string)
                                })
                                .or_else(|| {
                                    // `use crate::cache;` binds the module: `cache::stats()`
                                    // names `stats` in the module's own file.
                                    let (local, rest) = name.split_once("::")?;
                                    let original = import["bindings"].get(local)?.as_str()?;
                                    let module_file =
                                        [format!("{original}.rs"), format!("{original}/mod.rs")]
                                            .iter()
                                            .any(|file| {
                                                target == file
                                                    || target.ends_with(&format!("/{file}"))
                                            });
                                    (path.ends_with(".rs") && module_file).then(|| rest.to_string())
                                })
                        };
                        if let Some(imported) = imported {
                            if imported.contains('.') || imported.contains("::") {
                                continue;
                            }
                            match exported(
                                target,
                                &imported,
                                analyses,
                                &import_edges,
                                &mut BTreeSet::new(),
                                &mut 256,
                            ) {
                                Some(found) => candidates.extend(found),
                                None => incomplete_export = true,
                            }
                        }
                    }
                }
            }
            if candidates.len() == 1 && !incomplete_export {
                let (target, callee) = candidates.into_iter().next().unwrap();
                resolved.push(json!({"path":path,"caller":caller,"callee_path":target,"callee":callee,"line":call["line"],
                    "basis":if target==*path{"unique_visible_declaration_candidate"}else{"explicit_import_binding"},"runtime_verified":false}));
            } else {
                unresolved.push(json!({"path":path,"caller":caller,"callee":name,"line":call["line"],"reason":if incomplete_export {"incomplete_or_cyclic_export"} else if candidates.is_empty(){"external_or_dynamic_call"}else{"ambiguous_call_binding"}}));
            }
        }
    }
    (resolved, unresolved)
}
