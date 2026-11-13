//! Basic, deliberately file-granular impact over admitted bytes. Dependencies
//! are potential relationships, never compiler-resolved calls or deployment facts.
//! No project code, package manager, Terraform, Helm or shell command is run.
use super::*;
use graphicle_memory::structure;

const VERSION: &str = "basic-source-impact-v2";

pub(super) fn ancestor_file(input: &Input, path: &str, name: &str) -> Option<String> {
    let mut dir = Path::new(path).parent()?;
    // A pin directory is the trust boundary: never search a sibling repository.
    loop {
        let candidate = dir.join(name).to_str()?.to_owned();
        if input.sources.contains_key(&candidate) {
            return Some(candidate);
        }
        if dir.components().count() <= 1 {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// A file the chart at `chart` (its Chart.yaml) renders or configures: the
/// chart's metadata, lock, values and their schema, and everything under
/// templates/ (Helm renders every file there). Other files in a chart
/// (files/, scripts/, crds/) keep their own language; a template reaches the
/// ones it loads through `.Files`.
fn chart_file(chart: &str, path: &str) -> bool {
    let Some(rel) = Path::new(chart)
        .parent()
        .and_then(|root| Path::new(path).strip_prefix(root).ok())
        .and_then(Path::to_str)
    else {
        return false;
    };
    rel.starts_with("templates/")
        || matches!(
            rel,
            "Chart.yaml"
                | "Chart.lock"
                | "requirements.yaml"
                | "requirements.lock"
                | "values.schema.json"
                | ".helmignore"
        )
        || (rel.starts_with("values")
            && !rel.contains('/')
            && (rel.ends_with(".yaml") || rel.ends_with(".yml")))
}

pub(super) fn language(input: &Input, path: &str) -> Option<&'static str> {
    let source = input.sources.get(path)?;
    let content = source.content.text()?;
    let name = Path::new(path).file_name()?.to_str()?;
    let chart = ancestor_file(input, path, "Chart.yaml");
    // A templates/ file using Helm's own objects with no Chart.yaml above it
    // is a chart whose metadata is missing (reported); a plain manifest or
    // another tool's template under templates/ is not.
    let orphan = chart.is_none()
        && path.contains("/templates/")
        && matches!(
            Path::new(path).extension().and_then(|e| e.to_str()),
            Some("yaml" | "yml" | "tpl")
        )
        && content.contains("{{")
        && [
            ".Values",
            ".Release",
            ".Chart",
            ".Capabilities",
            ".Files",
            "include \"",
        ]
        .iter()
        .any(|object| content.contains(object));
    if orphan || chart.is_some_and(|chart| chart_file(&chart, path)) {
        return Some("helm");
    }
    if let Some(build) = build_language(name) {
        return Some(build);
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    Some(match ext {
        "tf" | "tfvars" => "terraform",
        "hcl" if name == ".terraform.lock.hcl" || name.ends_with(".tftest.hcl") => "terraform",
        "hcl" => "hcl",
        "lua" => "lua",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "mustache" | "hbs" | "handlebars" | "njk" | "j2" | "jinja" | "jinja2" | "tmpl"
        | "gotmpl" | "gohtml" | "tpl" => "template",
        "py" | "pyi" => "python",
        "rs" => "rust",
        "sh" | "bash" => "bash",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "sql" => "sql",
        "proto" => "proto",
        _ if name.ends_with(".tf.json") || name.ends_with(".tfvars.json") => "terraform",
        _ if matches!(
            name,
            "Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml"
        ) =>
        {
            "rust"
        }
        _ if matches!(
            name,
            "pyproject.toml"
                | "setup.py"
                | "setup.cfg"
                | "requirements.txt"
                | "uv.lock"
                | "poetry.lock"
        ) =>
        {
            "python"
        }
        _ if matches!(name, "tsconfig.json" | "jsconfig.json") => "typescript",
        _ if matches!(name, "sqlc.yaml" | "sqlc.yml" | "sqlc.json") => "sql",
        _ if matches!(
            name,
            "buf.yaml" | "buf.work.yaml" | "buf.gen.yaml" | "buf.lock"
        ) =>
        {
            "proto"
        }
        _ if matches!(
            name,
            "package.json"
                | "package-lock.json"
                | "npm-shrinkwrap.json"
                | "yarn.lock"
                | "pnpm-lock.yaml"
                | "pnpm-workspace.yaml"
        ) =>
        {
            "javascript"
        }
        _ if content.lines().next().is_some_and(|line| {
            line.starts_with("#!")
                && line
                    .split(['/', ' ', '\t'])
                    .any(|part| matches!(part, "bash" | "sh"))
        }) =>
        {
            "bash"
        }
        _ if matches!(name, "action.yml" | "action.yaml")
            || path.contains("/.github/workflows/") && matches!(ext, "yml" | "yaml") =>
        {
            "github_actions"
        }
        "yaml" | "yml" => "yaml",
        "json" => "json",
        _ => return None,
    })
}

/// Build files known by name rather than extension.
fn build_language(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    // `dockerfile_test.go` or `Makefile.toml` is a source, data or document
    // file named after a build file: its extension decides.
    if matches!(
        ext,
        "go" | "py"
            | "pyi"
            | "rs"
            | "ts"
            | "js"
            | "java"
            | "kt"
            | "sh"
            | "bash"
            | "md"
            | "txt"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "j2"
            | "tpl"
            | "tmpl"
    ) {
        return None;
    }
    // `Dockerfile`, `Dockerfile.prod`, `Dockerfile-dev`, `api.Dockerfile`.
    let named = |base: &str| {
        lower == base
            || lower
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with(['.', '-', '_']))
            || lower
                .strip_suffix(base)
                .is_some_and(|rest| rest.ends_with(['.', '-', '_']))
    };
    if (named("dockerfile") || named("containerfile")) && !lower.ends_with(".dockerignore") {
        Some("dockerfile")
    } else if named("makefile") || lower == "gnumakefile" || ext == "mk" {
        Some("make")
    } else if lower.starts_with("jenkinsfile")
        || named("jenkinsfile")
        || matches!(ext, "groovy" | "gvy")
    {
        Some("groovy")
    } else {
        None
    }
}

/// A basic input of configuration (YAML, JSON) that code reads in ways basic
/// review cannot see: a change to it keeps the repository-wide conservative
/// scope a file outside basic review has, unless environment analysis
/// narrows it.
pub(super) fn configuration(input: &Value) -> bool {
    input["analysis_level"] == "basic_file" && matches!(s(input, "language"), "yaml" | "json")
}

/// A TypeScript or JavaScript source file, as opposed to its manifests.
pub(super) fn script_source(ext: &str) -> bool {
    matches!(
        ext,
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs"
    )
}

/// No line written by hand is this long; a bundler's or minifier's output is
/// one line of the whole program.
const MINIFIED_LINE: usize = 10_000;

/// A TypeScript or JavaScript file that is generated output, not source: it
/// stays a file node, with no declarations, imports or calls.
pub(super) fn minified(ext: &str, content: &str) -> bool {
    script_source(ext) && content.lines().any(|l| l.len() > MINIFIED_LINE)
}

/// The path `extend` parses a file's structure under, or None when it reads
/// no structure for it: a minified bundle, or a language it keeps at file level.
/// A file read for how imports resolve (paths, workspace members), not for
/// declarations.
fn module_config(path: &str) -> bool {
    matches!(
        Path::new(path).file_name().and_then(|n| n.to_str()),
        Some("tsconfig.json" | "jsconfig.json" | "package.json" | "pnpm-workspace.yaml")
    )
}

fn structure_path<'a>(lang: &str, path: &'a str, content: &str) -> Option<&'a str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let config = module_config(path);
    let read = !minified(ext, content)
        && (matches!(lang, "python" | "rust" | "bash")
            && (matches!(ext, "py" | "rs" | "sh" | "bash") || lang == "bash")
            || script_source(ext)
            || config);
    read.then_some(if lang == "bash" { "source.sh" } else { path })
}

/// Each file's structure, parsed on every core: parsing is most of `extend`'s
/// time and depends on the file alone. Workers keep the request's deadline.
fn parse_structures<'a>(
    input: &'a Input,
    files: &'a BTreeMap<String, &'static str>,
) -> Result<BTreeMap<&'a str, Value>> {
    let jobs: Vec<(&str, &str, &str)> = files
        .iter()
        .filter_map(|(path, lang)| {
            let source = &input.sources[path];
            let content = source.content.text()?;
            Some((
                path.as_str(),
                structure_path(lang, &source.path, content)?,
                content,
            ))
        })
        .collect();
    let left = graphicle::process::bounded_timeout(std::time::Duration::MAX)
        .map_err(|_| Error::timeout("Review request deadline exceeded"))?;
    let deadline = (left != std::time::Duration::MAX).then(|| std::time::Instant::now() + left);
    let parsed = graphicle_memory::cache::par_map(
        &jobs,
        || graphicle::process::deadline_scope(deadline),
        |_, (path, parse_path, content)| {
            checkpoint()?;
            Ok((
                *path,
                structure::analyze(parse_path, content)
                    .unwrap_or_else(|_| structure::skipped("basic parse failed")),
            ))
        },
    );
    parsed.into_iter().collect()
}

/// The review key of a declaration, stable across base and head.
pub(super) fn symbol_key(path: &str, name: &str) -> String {
    format!("symbol:{path}#{name}")
}

/// The bytes of line `line` (1-based) of `source`.
pub(super) fn line_span(source: &str, line: usize) -> std::ops::Range<usize> {
    let start = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let end = start
        + source
            .get(start..)
            .unwrap_or("")
            .split_inclusive('\n')
            .next()
            .unwrap_or("")
            .len();
    start..end
}

/// One review node per named declaration of a parsed source, beside its
/// file node: exact spans, the header (`signature`) and body
/// (`implementation`) digested apart, and `correspondence` the header with the
/// name masked, as the Go analyzer records them. A name declared twice in one
/// file is reported, not guessed. Returns the keys added.
fn declaration_nodes(
    out: &mut Analysis,
    owner: &str,
    path: &str,
    content: &str,
    parsed: &Value,
) -> BTreeSet<String> {
    let symbols = parsed["symbols"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut seen = BTreeMap::<&str, usize>::new();
    for d in symbols {
        *seen.entry(s(d, "name")).or_default() += 1;
    }
    let mut added = BTreeSet::new();
    for d in symbols {
        let name = s(d, "name");
        let (Some(start), Some(end)) = (d["byte_start"].as_u64(), d["byte_end"].as_u64()) else {
            continue;
        };
        let (start, end) = (start as usize, end as usize);
        let Some(text) = content
            .get(start..end)
            .filter(|_| !name.is_empty() && start < end)
        else {
            continue;
        };
        if seen[name] > 1 {
            if !out.issues.iter().any(|i| {
                i["reason"] == "basic_duplicate_declaration"
                    && i["detail"]["file"] == path
                    && i["detail"]["name"] == name
            }) {
                out.issues.push(issue(
                    "basic_duplicate_declaration",
                    json!({"file":path,"name":name}),
                ));
            }
            continue;
        }
        let body_start = d["body_start"]
            .as_u64()
            .map(|b| b as usize)
            .filter(|b| (start..=end).contains(b) && content.is_char_boundary(*b))
            .unwrap_or(end);
        let header = &content[start..body_start];
        let short = name.rsplit('.').next().unwrap_or(name);
        let symbol = Symbol {
            key: symbol_key(path, name),
            file: path.into(),
            package: path.into(),
            name: name.into(),
            kind: s(d, "kind").into(),
            parent: name
                .rsplit_once('.')
                .map_or("", |(parent, _)| parent)
                .into(),
            start,
            end,
            body_start,
            line: d["line"].as_u64().unwrap_or(1) as usize,
            end_line: d["end_line"].as_u64().unwrap_or(1) as usize,
            revision: text::sha256(text.as_bytes()),
            signature: text::sha256(header.as_bytes()),
            implementation: text::sha256(&content.as_bytes()[body_start..end]),
            correspondence: text::sha256(header.replacen(short, "\0", 1).as_bytes()),
        };
        added.insert(symbol.key.clone());
        out.symbols
            .insert(key(owner, &symbol.key), (owner.into(), symbol));
    }
    added
}

pub(super) fn world(path: &str) -> String {
    format!("basic:{}", path.split('/').next().unwrap_or(""))
}
pub(super) fn file_key(path: &str) -> String {
    format!("file:{path}")
}

/// Normalize lexically under this repository. Never touch the host filesystem.
pub(super) fn relative(file: &str, target: &str) -> Option<String> {
    if target.starts_with('/') || target.contains(['\\', ':', '\0']) {
        return None;
    }
    let mut parts: Vec<_> = file.split('/').collect();
    parts.pop();
    for part in target.split('/') {
        match part {
            "" | "." => (),
            ".." if parts.len() > 1 => {
                parts.pop();
            }
            ".." => return None,
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

pub(super) fn link(
    out: &mut Analysis,
    from: &str,
    to: &str,
    relation: &str,
    evidence: &str,
    line: usize,
    span: std::ops::Range<usize>,
) {
    if from == to {
        return;
    }
    edge(
        out,
        (from, &file_key(from)),
        (to, &file_key(to)),
        relation,
        evidence,
        line,
        span,
    );
}

/// An edge from `caller` in file `from` to `target` in file `to`.
pub(super) fn edge(
    out: &mut Analysis,
    (from, caller): (&str, &str),
    (to, target): (&str, &str),
    relation: &str,
    evidence: &str,
    line: usize,
    span: std::ops::Range<usize>,
) {
    if caller == target {
        return;
    }
    if out.calls.len() >= 200_000 {
        if !out.issues.iter().any(|i| i["reason"] == "basic_edge_limit") {
            out.issues.push(issue("basic_edge_limit", json!(200_000)));
        }
        return;
    }
    out.calls.push((
        world(from),
        Call {
            boundary: Value::Null,
            caller: caller.into(),
            target: target.into(),
            target_file: to.into(),
            target_package: to.into(),
            target_offset: 0,
            spelling: to.into(),
            file: from.into(),
            start: span.start,
            end: span.end,
            line,
            relation: relation.into(),
            reason: String::new(),
            evidence: evidence.into(),
            discarded_error: false,
        },
    ));
}

pub(super) fn extend(input: &Input, out: &mut Analysis) -> Result<()> {
    let files: BTreeMap<_, _> = input
        .sources
        .keys()
        .filter_map(|p| language(input, p).map(|l| (p.clone(), l)))
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    out.analyzer = format!("{}+{VERSION}", out.analyzer);
    let mut structures = parse_structures(input, &files)?;
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    let mut imports = BTreeMap::<String, serde_json::Map<String, Value>>::new();
    let mut declared = BTreeSet::<String>::new();
    let text = |path: &str| input.sources.get(path).and_then(|s| s.content.text());
    let sqlc: Vec<super::basic_sql::Config> = files
        .keys()
        .filter(|p| super::basic_sql::config_file(p))
        .flat_map(|p| super::basic_sql::configs(p, text(p).unwrap_or("")))
        .collect();
    let mut sql_queries = BTreeMap::<&str, Vec<super::basic_sql::Query>>::new();
    let mut protos = BTreeMap::<&str, super::basic_proto::Proto>::new();
    // (declaration's file, its name, the generated Go file, the Go name)
    let mut generated_from = Vec::<(String, String, String, String)>::new();
    // the snapshot's `.proto` and `.pb.go` files by (repository, file stem)
    let mut proto_stems = BTreeMap::<(&str, &str), Vec<&str>>::new();
    let mut facts = BTreeMap::<&str, super::basic_refs::Facts>::new();
    for path in input.sources.keys() {
        if let Some(stem) = path
            .strip_suffix(".pb.go")
            .or_else(|| path.strip_suffix(".proto"))
        {
            let repo = path.split('/').next().unwrap_or("");
            proto_stems
                .entry((repo, stem.rsplit('/').next().unwrap_or("")))
                .or_default()
                .push(path);
        }
    }
    for (path, lang) in &files {
        checkpoint()?;
        *counts.entry(lang).or_default() += 1;
        let source = &input.sources[path];
        let content = source.content.text().unwrap();
        let owner = world(path);
        // File nodes preserve top-level code, decorators, comments, and parse
        // failures. A body-only slice must never hide a relevant input change.
        let symbol = Symbol {
            key: file_key(path),
            file: path.clone(),
            package: path.clone(),
            name: source.path.clone(),
            kind: "basic_file".into(),
            parent: String::new(),
            start: 0,
            end: content.len(),
            body_start: 0,
            line: 1,
            end_line: content.lines().count().max(1),
            revision: source.sha.clone(),
            signature: text::sha256(lang.as_bytes()),
            implementation: source.sha.clone(),
            correspondence: (*lang).into(),
        };
        out.symbols
            .insert(key(&owner, &symbol.key), (owner.clone(), symbol));
        let mut inventory = json!({"file":path,"role":"selected","package":path,"consumer_build":owner,"language":lang,"analysis_level":"basic_file"});
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let config = module_config(path);
        if minified(ext, content) {
            out.issues.push(issue(
                "basic_minified_source",
                json!({"file":path,"bytes":content.len()}),
            ));
        } else if let Some(mut parsed) = structures.remove(path.as_str()) {
            if *lang == "python" && ext == "py" {
                let shadowed_os = input.sources.keys().any(|other| {
                    other.split('/').next() == path.split('/').next()
                        && (other.ends_with("/os.py") || other.ends_with("/os/__init__.py"))
                });
                let environment = &parsed["environment"];
                let incomplete = environment.is_null()
                    || environment["issues"]
                        .as_array()
                        .is_none_or(|issues| !issues.is_empty())
                    || shadowed_os;
                if incomplete {
                    out.issues.push(issue("python_environment_incomplete",json!({"file":path,"issues":environment["issues"],"local_os_module":shadowed_os})));
                }
                inventory["environment"] = environment.clone();
                for operation in environment["operations"].as_array().into_iter().flatten().cloned()
                    .chain(incomplete.then(||json!({"kind":"read","key":null,"byte_start":0,"byte_end":content.len(),"line":1}))) {
                    out.calls.push((owner.clone(),Call {
                        boundary: Value::Null,
                        caller:file_key(path),target:String::new(),target_file:String::new(),target_package:String::new(),target_offset:0,
                        spelling:if shadowed_os {String::new()}else{operation["key"].as_str().unwrap_or("").into()},
                        file:path.clone(),start:operation["byte_start"].as_u64().unwrap_or(0) as usize,end:operation["byte_end"].as_u64().unwrap_or(0) as usize,
                        line:operation["line"].as_u64().unwrap_or(1) as usize,
                        relation:if operation["kind"]=="write"{"writes_environment"}else{"reads_environment"}.into(),
                        reason:String::new(),evidence:"python_import_binding".into(),discarded_error:false,
                    }));
                }
            }
            let declarations: Vec<_> = parsed["symbols"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| {
                    let start = d["byte_start"].as_u64().unwrap_or(u64::MAX) as usize;
                    let end = d["byte_end"].as_u64().unwrap_or(u64::MAX) as usize;
                    start < end && content.get(start..end).is_some()
                })
                .take(128)
                .cloned()
                .collect();
            inventory["declarations"] = json!(declarations);
            inventory["declarations_omitted"] = json!(
                parsed["symbols"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .saturating_sub(declarations.len())
            );
            if !structure::declaration_spans_complete(&parsed) {
                out.issues.push(issue(
                    "basic_parse_incomplete",
                    json!({"file":path,"issues":parsed["issues"]}),
                ));
                parsed["imports"] = json!([]);
            }
            if matches!(*lang, "python" | "rust" | "typescript" | "javascript") {
                if !config
                    && parsed["imports"].is_array()
                    && structure::declaration_spans_complete(&parsed)
                {
                    declared.extend(declaration_nodes(out, &owner, path, content, &parsed));
                } else {
                    parsed["calls"] = json!([]);
                }
                imports
                    .entry(path.split('/').next().unwrap().into())
                    .or_default()
                    .insert(source.path.clone(), parsed);
            }
        }
        if *lang == "sql" && ext == "sql" && !minified(ext, content) {
            let found = super::basic_sql::queries(content);
            let generated = sqlc
                .iter()
                .find(|c| super::basic_sql::under(path, &c.queries))
                .and_then(|c| c.out.as_deref())
                .and_then(|o| super::basic_sql::generated(o, path, text));
            let symbols: Vec<Value> = found
                .iter()
                .map(|q| {
                    let mut d = json!({"name":q.name,"kind":"sql_query","byte_start":q.start,"byte_end":q.end,"body_start":q.body_start,"line":q.line,"end_line":q.end_line});
                    if let Some((go, _)) = generated
                        .as_ref()
                        .filter(|(_, methods)| methods.contains(q.name.as_str()))
                    {
                        d["generated"] = json!({"file":input.sources[go].path,"function":q.name});
                        generated_from.push((
                            path.clone(),
                            q.name.clone(),
                            go.clone(),
                            q.name.clone(),
                        ));
                    }
                    d
                })
                .collect();
            inventory["declarations"] = json!(symbols.iter().take(128).collect::<Vec<_>>());
            inventory["declarations_omitted"] = json!(symbols.len().saturating_sub(128));
            declared.extend(declaration_nodes(
                out,
                &owner,
                path,
                content,
                &json!({ "symbols": symbols }),
            ));
            sql_queries.insert(path, found);
        }
        if *lang == "proto" && ext == "proto" && !minified(ext, content) {
            let parsed = super::basic_proto::parse(content);
            if parsed.incomplete {
                out.issues.push(issue(
                    "basic_parse_incomplete",
                    json!({"file":path,"issues":["unbalanced braces"]}),
                ));
            }
            let generated = proto_generated(input, &proto_stems, path, &parsed);
            let line = super::basic_proto::lines(content);
            let symbols: Vec<Value> = parsed
                .decls
                .iter()
                .map(|d| {
                    let mut v = json!({"name":d.name,"kind":d.kind,"byte_start":d.start,"byte_end":d.end,"body_start":d.body_start,"line":line(d.start),"end_line":line(d.end)});
                    let go = super::basic_proto::go_name(&d.name);
                    if let Some((file, types)) = generated.as_ref().filter(|_| {
                        matches!(d.kind, "proto_message" | "proto_enum")
                    }) && types.contains(go.as_str())
                    {
                        v["generated"] = json!({"file":input.sources[*file].path,"type":go});
                        generated_from.push((
                            path.clone(),
                            d.name.clone(),
                            (*file).into(),
                            go,
                        ));
                    }
                    v
                })
                .collect();
            inventory["declarations"] = json!(symbols.iter().take(128).collect::<Vec<_>>());
            inventory["declarations_omitted"] = json!(symbols.len().saturating_sub(128));
            declared.extend(declaration_nodes(
                out,
                &owner,
                path,
                content,
                &json!({ "symbols": symbols }),
            ));
            protos.insert(path, parsed);
        }
        let found = super::basic_refs::facts(lang, path, content);
        if !found.decls.is_empty() || !found.refs.is_empty() {
            let line = super::basic_proto::lines(content);
            let symbols: Vec<Value> = found
                .decls
                .iter()
                .map(|d| json!({"name":d.name,"kind":d.kind,"byte_start":d.start,"byte_end":d.end,"body_start":d.body_start,"line":line(d.start),"end_line":line(d.end)}))
                .collect();
            if !symbols.is_empty() {
                inventory["declarations"] = json!(symbols.iter().take(128).collect::<Vec<_>>());
                inventory["declarations_omitted"] = json!(symbols.len().saturating_sub(128));
            }
            declared.extend(declaration_nodes(
                out,
                &owner,
                path,
                content,
                &json!({ "symbols": symbols }),
            ));
            facts.insert(path, found);
        }
        out.inputs.push(inventory);
        let group = match *lang {
            "helm" => ancestor_file(input, path, "Chart.yaml"),
            // A `terraform test` file in tests/ belongs to the module above it.
            "terraform" => Path::new(path)
                .parent()
                .map(|p| match p.parent() {
                    Some(module) if path.ends_with(".tftest.hcl") && p.ends_with("tests") => module,
                    _ => p,
                })
                .map(|p| format!("terraform:{}", p.display())),
            // A build/packaging configuration can affect every source in its
            // nearest project. This is conservative scope, not a call relation.
            "rust" => ancestor_file(input, path, "Cargo.toml"),
            "proto" => ancestor_file(input, path, "buf.yaml")
                .or_else(|| ancestor_file(input, path, "buf.work.yaml")),
            "sql" => ancestor_file(input, path, "sqlc.yaml")
                .or_else(|| ancestor_file(input, path, "sqlc.yml"))
                .or_else(|| ancestor_file(input, path, "sqlc.json")),
            "typescript" | "javascript" => ancestor_file(input, path, "package.json")
                .or_else(|| ancestor_file(input, path, "tsconfig.json"))
                .or_else(|| ancestor_file(input, path, "jsconfig.json")),
            "python" => ancestor_file(input, path, "pyproject.toml")
                .or_else(|| ancestor_file(input, path, "setup.py"))
                .or_else(|| ancestor_file(input, path, "setup.cfg"))
                .or_else(|| ancestor_file(input, path, "requirements.txt")),
            _ => None,
        };
        if let Some(group) = group {
            groups.entry(group).or_default().push(path.clone());
        } else if *lang == "helm" {
            out.issues
                .push(issue("basic_missing_chart_metadata", json!({"file":path})));
        }
    }
    for (lang, count) in counts {
        out.issues.push(issue("basic_analysis_limitations",json!({"language":lang,"files":count,"granularity":"file",
            "compiler_resolution":false,"dependency_coverage_complete":false,
            "limitations":match lang {
                "terraform"=>"No plan, state, provider schemas, remote module resolution or expression evaluation",
                "helm"=>"Chart source scope only; no rendering, release values, cluster capabilities or remote chart resolution",
                "bash"=>"Literal script paths are candidates; working directory, PATH, expansions and dynamic commands are unresolved",
                "python"=>"Local import candidates, calls bound by name through a visible declaration or explicit import, and literal environment reads only; runtime environments, dynamic imports, rebinding, method calls and runtime dispatch are not resolved",
                "proto"=>"Messages, enums, services, rpcs and extensions are declarations; a field's, rpc's or extension's message or enum type and a custom option name link it to the declaration of that name in its file or its imports, by protobuf scoping; an import names the one snapshot .proto at that path or ending in it (no buf or protoc import roots); the generated Go type, where Go review declares it, links the declaration to the Go code that uses it; option values, field numbers and wire compatibility are not resolved",
                "sql"=>"sqlc query blocks are declarations; a query's FROM, JOIN, INTO and UPDATE tables link it to the sqlc schema files that create, alter, drop or write a table of that name, as candidates; the generated Go method, where Go review declares it, links the query to the Go code that calls it; views, functions, search paths and dynamic SQL are not resolved; SQL outside an sqlc configuration is file level only",
                "dockerfile"=>"Named stages are declarations; COPY and ADD sources and literal paths in RUN, CMD and ENTRYPOINT link to the snapshot file beside the Dockerfile or at the repository root, as candidates (the build context is not known); COPY --from and FROM link a stage to the stage it names; build arguments, remote images and the context directory are not resolved",
                "make"=>"Rule targets are declarations; a prerequisite links its target to the target of that name in the file, and include, file prerequisites and literal paths in recipes link to snapshot files; variables, pattern rules, implicit rules and recursive make are not resolved",
                "groovy"=>"File level; literal paths in the file (sh steps, load, Dockerfile arguments) link to snapshot files beside it or at the repository root, as candidates; shared libraries, Groovy evaluation and plugins are not resolved",
                "github_actions"=>"Workflow jobs are declarations; needs links a job to the jobs it needs, uses ./ links to the local action or reusable workflow, and literal paths in run and with values link to snapshot files; remote actions, expressions, matrices and path filters are not resolved",
                "yaml"=>"Kubernetes objects (kind and metadata.name) are declarations and link to the ConfigMaps, Secrets, ServiceAccounts, claims, Services and scale or rollout targets they name, beside them first, as candidates; a kustomization links to its resources, bases, components, patches and generator files; other YAML is file level; no rendering, overlays merge, namespaces or cluster state",
                "json"=>"File level; a relative JSON schema $ref links to that file; consumers of configuration files are not resolved",
                "html"=>"File level; relative src and href values link to snapshot files, and a page written as a Jinja/Django or Go template links its include, extends and template actions as a template does; absolute, remote and generated URLs are not resolved",
                "css"=>"File level; relative @import, @use, @forward and url() values link to snapshot files, with Sass partial names; load paths and bundler aliases are not resolved",
                "template"=>"Mustache partials and Jinja/Nunjucks include, extends, import and from link to the template file beside it or the one source ending in that name; Go template define and block are declarations linked from template and include by name; loader roots and template data are not resolved",
                "lua"=>"Top-level functions are declarations; a call by a declared name links its caller to it within the file, and require links to the one source ending in the module path; metatables, upvalues, dynamic requires and host APIs (Envoy, nginx) are not resolved",
                "c"|"cpp"=>"File level; #include \"...\" links to the header beside the file or the one source ending in that path; include paths, macros, system headers and calls are not resolved",
                "hcl"=>"File level; literal paths in the file link to snapshot files; HCL evaluation and tool-specific semantics (Terragrunt, Packer, Nomad) are not resolved",
                "typescript"|"javascript"=>"Relative and tsconfig/jsconfig path import candidates, calls bound by name through a visible declaration or explicit import, and, in repositories within 64 MiB of sources, calls the oxc binder resolves (new, and methods on a receiver typed by an annotation or made by new); node_modules, package exports, CommonJS require, dynamic import(), bundler aliases, JSX, callbacks and untyped receivers are not resolved",
                _=>"Local module candidates and calls bound by name through a visible declaration or explicit import only; Cargo resolution, cfg, macros, build scripts, method calls and trait dispatch are not resolved"
            }})));
    }
    for (group, members) in groups {
        let anchor = if files.contains_key(&group) {
            group.as_str()
        } else {
            &members[0]
        };
        let infrastructure = matches!(files[anchor], "helm" | "terraform");
        for member in &members {
            link(
                out,
                member,
                anchor,
                "configuration_scope",
                "shared_source_configuration",
                1,
                0..0,
            );
            // Terraform/Helm inputs form a single conservative module/chart
            // scope. Language project files depend on their manifest only.
            if infrastructure
                || Path::new(member)
                    .file_name()
                    .is_some_and(|n| n == "build.rs")
                || !(matches!(
                    Path::new(member).extension().and_then(|e| e.to_str()),
                    Some("py" | "pyi" | "rs" | "sql" | "proto")
                ) || script_source(
                    Path::new(member)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or(""),
                ))
            {
                link(
                    out,
                    anchor,
                    member,
                    "configuration_scope",
                    "shared_source_configuration",
                    1,
                    0..0,
                );
            }
        }
        if matches!(files[anchor], "helm" | "rust")
            && let Some(parent_dir) = Path::new(anchor).parent().and_then(Path::parent)
        {
            let manifest = if files[anchor] == "helm" {
                "Chart.yaml"
            } else {
                "Cargo.toml"
            };
            let probe = parent_dir.join(manifest).to_string_lossy().into_owned();
            if let Some(parent) = ancestor_file(input, &probe, manifest)
                && files.contains_key(&parent)
            {
                let (from, to, evidence) = if files[anchor] == "helm" {
                    (parent.as_str(), anchor, "vendored_chart_candidate")
                } else {
                    (
                        anchor,
                        parent.as_str(),
                        "ancestor_cargo_configuration_candidate",
                    )
                };
                link(out, from, to, "configuration_scope", evidence, 1, 0..0);
            }
        }
    }
    for (repo, analyses) in imports {
        checkpoint()?;
        if analyses.len() > structure::GRAPH_FILE_LIMIT {
            out.issues.push(issue(
                "basic_import_limit",
                json!({"directory":repo,"files":analyses.len()}),
            ));
            continue;
        }
        // Reuse the existing static import resolver. Its targets remain
        // candidates here: a source layout is not a pinned runtime/build path.
        let graph = match structure::graph(&Value::Object(analyses)) {
            Ok(graph) => graph,
            Err(error) => {
                checkpoint()?; // past the deadline: a timeout, not a failed graph
                return Err(error.into());
            }
        };
        for edge in graph["edges"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["kind"] == "imports")
        {
            let from = format!("{repo}/{}", s(edge, "source"));
            let to = format!("{repo}/{}", s(edge, "target"));
            if files.contains_key(&from) && files.contains_key(&to) {
                let line = edge["line"].as_u64().unwrap_or(1) as usize;
                let source = input.sources[&from].content.text().unwrap();
                link(
                    out,
                    &from,
                    &to,
                    "potential_import",
                    "static_source_candidate",
                    line,
                    line_span(source, line),
                );
            }
        }
        // Calls the parser bound to one declaration through a visible
        // declaration or an explicit import are callers of that declaration;
        // a call at module level is its file's.
        for call in graph["symbol_calls"].as_array().into_iter().flatten() {
            let from = format!("{repo}/{}", s(call, "path"));
            let to = format!("{repo}/{}", s(call, "callee_path"));
            if !files.contains_key(&from) || !files.contains_key(&to) {
                continue;
            }
            let node = |file: &str, name: &str| {
                let key = symbol_key(file, name);
                if declared.contains(&key) {
                    key
                } else {
                    file_key(file)
                }
            };
            let line = call["line"].as_u64().unwrap_or(1) as usize;
            let source = input.sources[&from].content.text().unwrap();
            edge(
                out,
                (&from, &node(&from, s(call, "caller"))),
                (&to, &node(&to, s(call, "callee"))),
                "may_call",
                s(call, "basis"),
                line,
                line_span(source, line),
            );
        }
        super::basic_precise::calls(input, &repo, &declared, out);
        let unresolved_calls = graph["unresolved_calls"].as_array().map_or(0, Vec::len);
        if unresolved_calls > 0 {
            out.issues.push(issue(
                "basic_unresolved_calls",
                json!({"directory":repo,"calls":unresolved_calls}),
            ));
        }
        for unresolved in graph["unresolved"].as_array().into_iter().flatten() {
            out.issues.push(issue(
                "basic_unresolved_import",
                json!({"directory":repo,"cause":s(unresolved,"reason"),"language":files.get(&format!("{repo}/{}",s(unresolved,"source"))),"input":unresolved}),
            ));
        }
    }
    for (file, lang) in &files {
        checkpoint()?;
        if !matches!(*lang, "bash" | "terraform") {
            continue;
        }
        // A tfvars object's `module` key is a variable value, not a module
        // declaration. Values and lockfiles participate only in module scope.
        if *lang == "terraform" && !file.ends_with(".tf") && !file.ends_with(".tf.json") {
            continue;
        }
        let content = input.sources[file].content.text().unwrap();
        let grammar = if *lang == "terraform" && file.ends_with(".json") {
            "terraform_json"
        } else {
            lang
        };
        let parsed = graphicle::extractors::basic_dependencies::extract(grammar, content)?;
        for reason in parsed["issues"].as_array().into_iter().flatten() {
            out.issues.push(issue(
                "basic_dependency_parse_incomplete",
                json!({"file":file,"cause":reason}),
            ));
        }
        for dependency in parsed["dependencies"].as_array().into_iter().flatten() {
            let literal = dependency["path"].as_str();
            let local = literal
                .filter(|s| *lang == "bash" || s.starts_with("./") || s.starts_with("../"))
                .and_then(|s| relative(file, s));
            let targets: Vec<_> = if *lang == "terraform" {
                local
                    .as_ref()
                    .map(|dir| {
                        files
                            .iter()
                            .filter(|(p, l)| {
                                **l == "terraform"
                                    && Path::new(p).parent().and_then(Path::to_str)
                                        == Some(dir.as_str())
                            })
                            .map(|(p, _)| p.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                local
                    .filter(|p| files.contains_key(p))
                    .into_iter()
                    .collect()
            };
            // A command run by a path that is not a snapshot file is a tool or
            // build output, not a missing dependency.
            if targets.is_empty() && dependency["kind"] != "shell_command" {
                out.issues.push(issue("basic_unresolved_dependency",json!({"file":file,"line":dependency["line"],"kind":dependency["kind"],"dynamic":dependency["dynamic"],"cause":"dynamic_external_or_missing_pinned_target"})));
            }
            for target in targets {
                link(
                    out,
                    file,
                    &target,
                    "potential_import",
                    if *lang == "bash" {
                        "literal_shell_path_candidate"
                    } else {
                        "literal_local_module"
                    },
                    dependency["line"].as_u64().unwrap_or(1) as usize,
                    dependency["byte_start"].as_u64().unwrap_or(0) as usize
                        ..dependency["byte_end"].as_u64().unwrap_or(0) as usize,
                );
            }
        }
    }
    super::basic_links::references(input, &files, &facts, &declared, out)?;
    super::basic_links::helm_files(input, &files, out)?;
    super::basic_links::go_embeds(input, &files, out)?;
    sql_tables(input, &files, &sqlc, &sql_queries, &declared, out)?;
    proto_types(input, &protos, &declared, out)?;
    generated_links(out, &generated_from);
    Ok(())
}

/// Links each generated Go method or type that Go review declares to the
/// sqlc query or proto declaration it was generated from (`generated_from`,
/// across worlds), so a change to the source reaches the Go code that calls
/// or uses what was generated through Go review's own edges.
fn generated_links(out: &mut Analysis, generated: &[(String, String, String, String)]) {
    let files: BTreeSet<&str> = generated.iter().map(|g| g.2.as_str()).collect();
    let mut go = BTreeMap::<(&str, &str), Vec<&str>>::new();
    for (id, (world, symbol)) in &out.symbols {
        if world.starts_with("basic:") || !files.contains(symbol.file.as_str()) {
            continue;
        }
        // A method is `(*pkg.Type).Name` or `(pkg.Type).Name`.
        let name = match symbol.kind.as_str() {
            "function" => symbol
                .name
                .strip_prefix('(')
                .and_then(|n| n.rsplit_once(").").map(|(_, method)| method)),
            "type" => Some(symbol.name.as_str()),
            _ => None,
        };
        if let Some(name) = name {
            go.entry((symbol.file.as_str(), name)).or_default().push(id);
        }
    }
    let mut edges = vec![];
    for (file, name, go_file, go_name) in generated {
        let to = key(&world(file), &symbol_key(file, name));
        if !out.symbols.contains_key(&to) {
            continue;
        }
        for from in go
            .get(&(go_file.as_str(), go_name.as_str()))
            .into_iter()
            .flatten()
        {
            edges.push(services::Edge {
                from: (*from).into(),
                to: to.clone(),
                relation: "generated_from",
                evidence: json!({"evidence":"generated_code_name","source_file":file,"declaration":name,"generated_file":go_file,"generated_name":go_name}),
            });
        }
    }
    out.services.edges.extend(edges);
}

/// The generated `.pb.go` of a proto file and the types it declares: the one
/// snapshot `<stem>.pb.go` in the proto's folder or in a folder that ends
/// its `go_package` import path; when there is none, the one whose
/// `// source:` header names this proto from an import root that holds the
/// generated file, and names no other `<stem>.proto` of the repository so.
fn proto_generated<'a>(
    input: &'a Input,
    proto_stems: &BTreeMap<(&str, &str), Vec<&'a str>>,
    path: &str,
    proto: &super::basic_proto::Proto,
) -> Option<(&'a str, BTreeSet<&'a str>)> {
    let (repo, rest) = path.split_once('/')?;
    let stem = rest.rsplit('/').next()?.strip_suffix(".proto")?;
    let folder = rest.rsplit_once('/').map_or("", |(d, _)| d);
    let import = proto
        .go_package
        .as_deref()
        .map(|g| g.split(';').next().unwrap_or(g).trim_end_matches('/'));
    let same = proto_stems.get(&(repo, stem))?;
    let dir = |p: &'a str| p[repo.len() + 1..].rsplit_once('/').map_or("", |(d, _)| d);
    let pb_go = || same.iter().copied().filter(|p| p.ends_with(".pb.go"));
    let placed: Vec<&str> = pb_go()
        .filter(|go| {
            let dir = dir(go);
            dir == folder || import.is_some_and(|i| i == dir || i.ends_with(&format!("/{dir}")))
        })
        .collect();
    let found = if placed.is_empty() {
        pb_go()
            .filter(|go| {
                let Some(source) = input.sources[*go].content.text().and_then(|t| {
                    t.lines()
                        .take_while(|l| !l.starts_with("package "))
                        .find_map(|l| l.strip_prefix("// source: "))
                }) else {
                    return false;
                };
                let held = format!("{}/", dir(go));
                let names = |p: &str| {
                    p[repo.len() + 1..]
                        .strip_suffix(source)
                        .is_some_and(|root| {
                            (root.is_empty() || root.ends_with('/')) && held.starts_with(root)
                        })
                };
                names(path)
                    && same
                        .iter()
                        .filter(|p| p.ends_with(".proto") && names(p))
                        .count()
                        == 1
            })
            .collect()
    } else {
        placed
    };
    let [go] = found[..] else {
        return None;
    };
    Some((
        go,
        super::basic_proto::go_types(input.sources[go].content.text()?),
    ))
}

/// Each proto declaration's `uses_type` edges to the messages, enums and
/// extensions its fields, rpcs and custom options name in its file and the
/// files it imports.
fn proto_types(
    input: &Input,
    protos: &BTreeMap<&str, super::basic_proto::Proto>,
    declared: &BTreeSet<String>,
    out: &mut Analysis,
) -> Result<()> {
    let mut index = std::collections::HashMap::<String, Vec<(&str, &str)>>::new();
    for (file, proto) in protos {
        for d in &proto.decls {
            let full = match proto.package.as_str() {
                "" => d.name.clone(),
                package => format!("{package}.{}", d.name),
            };
            index.entry(full).or_default().push((file, &d.name));
        }
    }
    let mut by_suffix = std::collections::HashMap::<(&str, &str), Vec<&str>>::new();
    for file in protos.keys() {
        let (repo, rest) = file.split_once('/').unwrap_or(("", file));
        let mut tail = rest;
        loop {
            by_suffix.entry((repo, tail)).or_default().push(file);
            match tail.split_once('/') {
                Some((_, next)) => tail = next,
                None => break,
            }
        }
    }
    for (file, proto) in protos {
        checkpoint()?;
        let repo = file.split('/').next().unwrap_or("");
        let mut visible = BTreeSet::from([*file]);
        for import in &proto.imports {
            match by_suffix.get(&(repo, import.as_str())).map(Vec::as_slice) {
                Some([target]) => {
                    visible.insert(target);
                }
                found if !import.starts_with("google/protobuf/") => out.issues.push(issue(
                    "proto_import_unresolved",
                    json!({"file":file,"import":import,"candidates":found.map_or(0, <[_]>::len)}),
                )),
                _ => {}
            }
        }
        let content = input.sources[*file].content.text().unwrap_or("");
        let line = super::basic_proto::lines(content);
        for r in &proto.refs {
            let caller = r
                .owner
                .map(|o| symbol_key(file, &proto.decls[o].name))
                .filter(|k| declared.contains(k))
                .unwrap_or_else(|| file_key(file));
            let target = super::basic_proto::candidates(&proto.package, &r.scope, &r.name)
                .iter()
                .find_map(|full| {
                    let found: Vec<_> = index
                        .get(full)?
                        .iter()
                        .filter(|(f, _)| visible.contains(f))
                        .collect();
                    match found[..] {
                        [one] => Some(*one),
                        _ => None,
                    }
                });
            let Some((target_file, name)) = target else {
                continue;
            };
            let target = symbol_key(target_file, name);
            if declared.contains(&target) {
                edge(
                    out,
                    (file, &caller),
                    (target_file, &target),
                    "uses_type",
                    if r.option {
                        "proto_custom_option_name"
                    } else {
                        "proto_type_name"
                    },
                    line(r.start),
                    r.start..r.end,
                );
            }
        }
    }
    Ok(())
}

/// Each sqlc query's `uses_table` edges to the schema files that touch a
/// table it uses: a migration's change selects the queries of its tables.
fn sql_tables(
    input: &Input,
    files: &BTreeMap<String, &'static str>,
    sqlc: &[super::basic_sql::Config],
    queries: &BTreeMap<&str, Vec<super::basic_sql::Query>>,
    declared: &BTreeSet<String>,
    out: &mut Analysis,
) -> Result<()> {
    for config in sqlc {
        checkpoint()?;
        let mut touching = BTreeMap::<String, Vec<&str>>::new();
        for file in files
            .keys()
            .filter(|p| p.ends_with(".sql") && super::basic_sql::under(p, &config.schema))
        {
            let content = input.sources[file].content.text().unwrap_or("");
            for table in super::basic_sql::touched(content) {
                touching.entry(table).or_default().push(file);
            }
        }
        for (file, found) in queries
            .iter()
            .filter(|(p, _)| super::basic_sql::under(p, &config.queries))
        {
            checkpoint()?;
            let content = input.sources[*file].content.text().unwrap_or("");
            for q in found {
                let caller = symbol_key(file, &q.name);
                if !declared.contains(&caller) {
                    continue;
                }
                let migrations: BTreeSet<&str> =
                    super::basic_sql::used(&content[q.body_start..q.end])
                        .iter()
                        .filter_map(|t| touching.get(t))
                        .flatten()
                        .copied()
                        .collect();
                for migration in migrations {
                    edge(
                        out,
                        (file, &caller),
                        (migration, &file_key(migration)),
                        "uses_table",
                        "sqlc_table_name_candidate",
                        q.line,
                        q.start..q.body_start,
                    );
                }
            }
        }
    }
    Ok(())
}

pub(super) fn annotate(entry: &mut Value, analysis: &Analysis, file: &str) {
    if entry["kind"] != "basic_file" {
        // A declaration keeps its signature and implementation revisions.
        if let Some(input) = analysis
            .inputs
            .iter()
            .find(|i| i["file"] == file && i["analysis_level"] == "basic_file")
        {
            entry["language"] = input["language"].clone();
            if let Some(generated) = input["declarations"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|d| d["name"] == entry["name"])
                .and_then(|d| d.get("generated"))
            {
                entry["generated"] = generated.clone();
            }
        }
        entry["analysis_level"] = json!("basic_symbol");
        entry["test_classification"] = json!("not_analyzed");
        entry["test"] = Value::Null;
        entry["revision"] = json!(crate::digest(entry));
        return;
    }
    if let Some(input) = analysis
        .inputs
        .iter()
        .find(|i| i["file"] == file && i["analysis_level"] == "basic_file")
    {
        for field in [
            "language",
            "analysis_level",
            "declarations",
            "declarations_omitted",
        ] {
            if let Some(value) = input.get(field) {
                entry[field] = value.clone();
            }
        }
        entry["test_classification"] = json!("not_analyzed");
        entry["test"] = Value::Null;
        entry["signature_revision"] = Value::Null;
        entry["implementation_revision"] = Value::Null;
        entry["revision"] = json!(crate::digest(entry));
    }
}

#[cfg(test)]
#[path = "review_basic_langs_tests.rs"]
mod langs_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn input(files: &[(&str, &str)]) -> Input {
        let sources = files
            .iter()
            .map(|(p, t)| {
                let (repo, path) = p.split_once('/').unwrap();
                (
                    (*p).into(),
                    Source {
                        repository: repo.into(),
                        path: path.into(),
                        sha: text::sha256(t.as_bytes()),
                        content: crate::source_object::Content::Text((*t).into()).into(),
                    },
                )
            })
            .collect();
        Input {
            pins: vec![],
            sources,
            issues: vec![],
            bindings: vec![],
        }
    }
    fn profile() -> Profile {
        Profile {
            goos: "linux".into(),
            goarch: "amd64".into(),
            tags: vec![],
            tests: false,
            workspace: String::new(),
        }
    }
    pub(super) fn analysis(input: &Input) -> Analysis {
        analyze(
            input,
            &profile(),
            None,
            &mut crate::review_cache::PackagePool::default(),
            "test",
        )
        .unwrap()
    }
    pub(super) fn report(base: &Input, head: &Input) -> Value {
        compare(base, head, &analysis(base), &analysis(head), &request()).unwrap()
    }
    pub(super) fn request() -> Request {
        Request {
            timeout_ms: None,
            compact_provenance: false,
            repository_id: "repo".into(),
            base: vec![],
            head: vec![],
            profile: profile(),
            max_depth: 8,
            max_tokens: 16000,
            previous_packet_id: None,
            business_context: None,
            reuse_text: false,
            reviewed_entries: vec![],
            provided_diff: vec![],
        }
    }
    fn selected(report: &Value, side: &str, file: &str) -> bool {
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["side"] == side && e["file"] == file)
    }

    #[test]
    fn python_imports_include_unchanged_consumers_without_claiming_calls() {
        let caller = "from tax import calculate\ncalculate = replacement\ndef run():\n    return calculate()\n";
        let base = input(&[
            ("repo/app.py", caller),
            ("repo/tax.py", "def calculate():\n    return 1\n"),
            ("repo/unrelated.py", "def other(): return 3\n"),
        ]);
        let head = input(&[
            ("repo/app.py", caller),
            ("repo/tax.py", "def calculate():\n    return 2\n"),
            ("repo/unrelated.py", "def other(): return 3\n"),
        ]);
        let a = analysis(&head);
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "repo/app.py" && c.target_file == "repo/tax.py")
        );
        assert!(
            a.calls
                .iter()
                .all(|(_, c)| c.relation != "calls" && c.relation != "may_call")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "app.py"));
        assert!(!selected(&r, "head", "unrelated.py"));
        assert_eq!(r["coverage"]["complete"], false);
        assert!(
            !r["coverage"]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["reason"] == "unsupported_source_language_or_contract")
        );
        let e = r["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["file"] == "tax.py" && e["side"] == "head" && e["kind"] == "basic_file")
            .unwrap();
        assert_eq!(
            e["text"],
            head.sources["repo/tax.py"].content.text().unwrap()
        );
        assert_eq!(e["language"], "python");
        assert_eq!(e["declarations"][0]["name"], "calculate");
        assert!(e["signature_revision"].is_null());
        let changes = r["changes"].as_array().unwrap();
        assert!(
            changes
                .iter()
                .any(|c| c["granularity"] == "file" && c["signature_changed"].is_null())
        );
        // The declaration's own change is classified: its body changed.
        assert!(
            changes
                .iter()
                .any(|c| c["signature_changed"] == false && c["implementation_changed"] == true)
        );
    }

    #[test]
    fn typescript_and_javascript_imports_select_their_importers() {
        let files = |price: &str, manifest: &str| {
            input(&[
                ("repo/package.json", manifest),
                (
                    "repo/tsconfig.json",
                    "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"@lib/*\":[\"src/lib/*\"]}}}\n",
                ),
                (
                    "repo/src/lib/price.ts",
                    &format!("export function price(): number {{\n  return {price};\n}}\n"),
                ),
                (
                    "repo/src/cart.tsx",
                    "import { price } from './lib/price';\nexport const Cart = () => price();\n",
                ),
                (
                    "repo/src/alias.ts",
                    "import { price } from '@lib/price';\nexport const total = price();\n",
                ),
                (
                    "repo/src/legacy.mjs",
                    "import { price } from './lib/price.js';\nexport default price;\n",
                ),
                ("repo/src/unrelated.js", "export const other = 3;\n"),
            ])
        };
        let manifest = "{\"name\":\"shop\"}\n";
        let (base, head) = (files("1", manifest), files("2", manifest));
        let a = analysis(&head);
        for importer in [
            "repo/src/cart.tsx",
            "repo/src/alias.ts",
            "repo/src/legacy.mjs",
        ] {
            assert!(
                a.calls.iter().any(|(_, c)| c.file == importer
                    && c.target_file == "repo/src/lib/price.ts"
                    && c.relation == "potential_import"),
                "{importer}"
            );
        }
        // Resolved calls are `may_call` candidates, never compiler `calls`.
        assert!(a.calls.iter().all(|(_, c)| c.relation != "calls"));
        let r = report(&base, &head);
        for importer in ["src/cart.tsx", "src/alias.ts", "src/legacy.mjs"] {
            assert!(selected(&r, "head", importer), "{importer}");
        }
        assert!(!selected(&r, "head", "src/unrelated.js"));
        assert_eq!(r["coverage"]["complete"], false);
        assert!(
            !r["coverage"]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["reason"] == "unsupported_source_language_or_contract"),
            "{}",
            r["coverage"]["issues"]
        );
        // A manifest change keeps every source of its package in scope.
        let bumped = files("1", "{\"name\":\"shop\",\"version\":\"2\"}\n");
        let r = report(&base, &bumped);
        assert!(selected(&r, "head", "src/unrelated.js"));
    }

    #[test]
    fn a_migration_selects_the_sqlc_queries_of_its_tables() {
        let files = |migration: &str| {
            input(&[
                (
                    "repo/sqlc.yaml",
                    "version: \"2\"\nsql:\n  - engine: postgresql\n    schema: ./db/migrations/\n    queries: ./db/queries/\n    gen:\n      go:\n        out: ./db/sqlc\n",
                ),
                (
                    "repo/db/migrations/001_init.up.sql",
                    "CREATE TABLE IF NOT EXISTS users (id int);\nCREATE TABLE \"orders\" (id int);\n",
                ),
                ("repo/db/migrations/002_email.up.sql", migration),
                (
                    "repo/db/queries/users.sql",
                    "-- name: GetUser :one\nSELECT * FROM users WHERE id = $1;\n",
                ),
                (
                    "repo/db/queries/orders.sql",
                    "-- name: ListOrders :many\nWITH recent AS (SELECT * FROM public.orders)\nSELECT * FROM recent;\n",
                ),
                (
                    "repo/db/sqlc/users.sql.go",
                    "package db\n\nfunc (q *Queries) GetUser(ctx context.Context, id int32) (User, error) {\n\treturn User{}, nil\n}\n",
                ),
            ])
        };
        let base = files("ALTER TABLE users ADD COLUMN email text;\n");
        let head = files("ALTER TABLE users ADD COLUMN email text NOT NULL;\n");
        let a = analysis(&head);
        let uses = |query: &str, migration: &str| {
            a.calls.iter().any(|(_, c)| {
                c.relation == "uses_table" && c.caller == query && c.target_file == migration
            })
        };
        let get_user = symbol_key("repo/db/queries/users.sql", "GetUser");
        let list_orders = symbol_key("repo/db/queries/orders.sql", "ListOrders");
        assert!(uses(&get_user, "repo/db/migrations/002_email.up.sql"));
        assert!(uses(&get_user, "repo/db/migrations/001_init.up.sql"));
        assert!(uses(&list_orders, "repo/db/migrations/001_init.up.sql"));
        assert!(!uses(&list_orders, "repo/db/migrations/002_email.up.sql"));
        let r = report(&base, &head);
        assert!(selected(&r, "head", "db/queries/users.sql"), "{r}");
        assert!(!selected(&r, "head", "db/queries/orders.sql"));
        assert!(
            r["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["file"] == "db/queries/users.sql"
                    && e["generated"]
                        == json!({"file":"db/sqlc/users.sql.go","function":"GetUser"})),
            "{r}"
        );
        assert!(
            !r["coverage"]["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["reason"] == "unsupported_source_language_or_contract"
                    && i["detail"]["file"]
                        .as_str()
                        .is_some_and(|f| f.ends_with(".sql"))),
            "{}",
            r["coverage"]["issues"]
        );
    }

    #[test]
    fn a_changed_proto_message_selects_its_users_through_types_and_options() {
        let files = |money: &str, rules: &str| {
            input(&[
                (
                    "repo/proto/common/v1/common.proto",
                    &format!(
                        "syntax = \"proto3\";\npackage common.v1;\nimport \"google/protobuf/descriptor.proto\";\n\nmessage Money {{\n  {money}\n}}\n\nmessage Rules {{\n  {rules}\n}}\n\nextend google.protobuf.FieldOptions {{\n  optional Rules rules = 50000;\n}}\n"
                    ),
                ),
                (
                    "repo/proto/order/v1/order.proto",
                    "syntax = \"proto3\";\npackage order.v1;\nimport \"common/v1/common.proto\";\nimport \"missing/thing.proto\";\noption go_package = \"github.com/acme/repo/gen/order/v1;orderv1\";\n\nmessage Order {\n  common.v1.Money total = 1;\n  Line line = 2;\n  message Line {\n    string sku = 1 [(common.v1.rules).required = true];\n  }\n}\n\nmessage Other {\n  string note = 1;\n}\n\nservice Orders {\n  rpc Get(Order) returns (Order);\n}\n",
                ),
                (
                    "repo/gen/order/v1/order.pb.go",
                    "package orderv1\n\ntype Order struct {\n}\n\ntype Order_Line struct {\n}\n\ntype Other struct {\n}\n",
                ),
            ])
        };
        let base = files("int64 units = 1;", "bool required = 1;");
        let money = report(
            &base,
            &files(
                "int64 units = 1;\n  string currency = 2;",
                "bool required = 1;",
            ),
        );
        let named = |r: &Value, name: &str| {
            r["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == "head" && e["name"] == name)
        };
        assert!(named(&money, "Money"), "{money}");
        assert!(named(&money, "Order"), "{money}");
        assert!(named(&money, "Orders.Get"), "{money}");
        assert!(!named(&money, "Other"));
        assert!(!named(&money, "Order.Line"));
        // An option's extension carries a change to its type to the fields
        // that set it.
        let rules = report(
            &base,
            &files("int64 units = 1;", "bool required = 1;\n  bool strict = 2;"),
        );
        assert!(named(&rules, "rules"), "{rules}");
        assert!(named(&rules, "Order.Line"), "{rules}");
        assert!(!named(&rules, "Other"));
        let order = rules["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "Order.Line")
            .unwrap();
        assert_eq!(
            order["generated"],
            json!({"file":"gen/order/v1/order.pb.go","type":"Order_Line"})
        );
        let issues = rules["coverage"]["issues"].as_array().unwrap();
        assert!(
            issues
                .iter()
                .any(|i| i["reason"] == "proto_import_unresolved"
                    && i["detail"]["import"] == "missing/thing.proto"),
            "{issues:?}"
        );
        assert!(
            !issues
                .iter()
                .any(|i| i["detail"]["import"] == "google/protobuf/descriptor.proto")
        );
        assert!(
            !issues
                .iter()
                .any(|i| i["reason"] == "unsupported_source_language_or_contract"
                    && i["detail"]["file"]
                        .as_str()
                        .is_some_and(|f| f.ends_with(".proto"))),
            "{issues:?}"
        );
    }

    /// Go review's facts for `go`: each `(file, name, kind, text)` symbol,
    /// then `(caller, callee, relation)` calls by name, before basic review.
    pub(super) fn with_go(
        input: &Input,
        go: &[(&str, &str, &str, &str)],
        calls: &[(&str, &str, &str)],
    ) -> Analysis {
        let mut out = analyze_go(
            input,
            &profile(),
            None,
            &mut crate::review_cache::PackagePool::default(),
            "test",
        )
        .unwrap();
        for (file, name, kind, text) in go {
            let content = input.sources[*file].content.text().unwrap();
            let start = content.find(text).unwrap();
            let digest = text::sha256(text.as_bytes());
            let symbol = Symbol {
                key: format!("example.com/app|{file}|{name}|{kind}"),
                file: (*file).into(),
                package: "example.com/app".into(),
                name: (*name).into(),
                kind: (*kind).into(),
                parent: String::new(),
                start,
                end: start + text.len(),
                body_start: start,
                line: 1,
                end_line: 1,
                revision: digest.clone(),
                signature: digest.clone(),
                implementation: digest.clone(),
                correspondence: digest,
            };
            out.symbols
                .insert(key("app", &symbol.key), ("app".into(), symbol));
        }
        let go_key = |name: &str| {
            out.symbols
                .values()
                .find(|(w, s)| w == "app" && s.name == name)
                .unwrap()
                .1
                .key
                .clone()
        };
        let calls: Vec<_> = calls
            .iter()
            .map(|(caller, callee, relation)| Call {
                boundary: Value::Null,
                caller: go_key(caller),
                target: go_key(callee),
                target_file: String::new(),
                target_package: String::new(),
                target_offset: 0,
                spelling: (*callee).into(),
                file: String::new(),
                start: 0,
                end: 0,
                line: 1,
                relation: (*relation).into(),
                reason: String::new(),
                evidence: String::new(),
                discarded_error: false,
            })
            .collect();
        out.calls
            .extend(calls.into_iter().map(|c| ("app".to_string(), c)));
        extend(input, &mut out).unwrap();
        services::attach(input, &mut out).unwrap();
        out
    }

    #[test]
    fn a_proto_reaches_the_pb_go_whose_source_header_names_it() {
        let proto = |go_package: &str| {
            format!(
                "syntax = \"proto3\";\npackage p;\n{go_package}\nmessage Post {{\n  string id = 1;\n}}\n"
            )
        };
        let pb_go = |source: &str| {
            format!(
                "// Code generated by protoc-gen-go. DO NOT EDIT.\n// source: {source}\n\npackage p\n\ntype Post struct {{\n}}\n"
            )
        };
        let a = analysis(&input(&[
            // go_package "./" generated with --go_out=gen-proto
            (
                "repo/protos/sent/sent.proto",
                &proto("option go_package = \"./\";"),
            ),
            (
                "repo/protos/sent/gen-proto/sent.pb.go",
                &pb_go("sent.proto"),
            ),
            // paths=source_relative under another output root
            (
                "repo/protobuf/nexus/v1/config.proto",
                &proto("option go_package = \"nexus/v1\";"),
            ),
            (
                "repo/protobuf/generated/nexus/v1/config.pb.go",
                &pb_go("nexus/v1/config.proto"),
            ),
            // two protos of one name: only one's import root holds the file
            ("repo/go/protos/counter.proto", &proto("")),
            ("repo/node/proto/counter.proto", &proto("")),
            ("repo/go/protos/gen/counter.pb.go", &pb_go("counter.proto")),
            // the header names a proto the snapshot does not have
            ("repo/a/key.proto", &proto("")),
            ("repo/b/gen/key.pb.go", &pb_go("b/key.proto")),
            // two generated files name one proto
            ("repo/e/dmp.proto", &proto("")),
            ("repo/e/x/dmp.pb.go", &pb_go("dmp.proto")),
            ("repo/e/y/dmp.pb.go", &pb_go("dmp.proto")),
        ]));
        let generated: BTreeMap<&str, &Value> = a
            .inputs
            .iter()
            .flat_map(|i| {
                i["declarations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|d| d["name"] == "Post")
                    .map(|d| (i["file"].as_str().unwrap(), &d["generated"]))
            })
            .collect();
        let post = |file: &str| json!({"file": file, "type": "Post"});
        assert_eq!(
            generated["repo/protos/sent/sent.proto"],
            &post("protos/sent/gen-proto/sent.pb.go"),
            "{generated:?}"
        );
        assert_eq!(
            generated["repo/protobuf/nexus/v1/config.proto"],
            &post("protobuf/generated/nexus/v1/config.pb.go")
        );
        assert_eq!(
            generated["repo/go/protos/counter.proto"],
            &post("go/protos/gen/counter.pb.go")
        );
        for file in [
            "repo/node/proto/counter.proto",
            "repo/a/key.proto",
            "repo/e/dmp.proto",
        ] {
            assert!(generated[file].is_null(), "{file}: {generated:?}");
        }
    }

    #[test]
    fn a_change_reaches_the_go_code_that_uses_what_was_generated_from_it() {
        const QUERIES_GO: &str = "package db\n\nfunc (q *Queries) GetUser(ctx context.Context, id int32) (User, error) {\n\treturn User{}, nil\n}\n";
        const ORDER_GO: &str = "package orderv1\n\ntype Order struct {\n}\n";
        const APP_GO: &str = "package app\n\nfunc Handle(q *db.Queries) {\n\tq.GetUser(nil, 1)\n}\n\nfunc Total(o *orderv1.Order) {\n}\n\nfunc Other() {\n}\n";
        let files = |migration: &str, money: &str| {
            input(&[
                (
                    "repo/sqlc.yaml",
                    "version: \"2\"\nsql:\n  - schema: ./db/migrations/\n    queries: ./db/queries/\n    gen:\n      go:\n        out: ./db/sqlc\n",
                ),
                ("repo/db/migrations/001_init.up.sql", migration),
                (
                    "repo/db/queries/users.sql",
                    "-- name: GetUser :one\nSELECT * FROM users WHERE id = $1;\n",
                ),
                ("repo/db/sqlc/users.sql.go", QUERIES_GO),
                (
                    "repo/proto/order.proto",
                    &format!(
                        "syntax = \"proto3\";\npackage order.v1;\noption go_package = \"example.com/app/gen/order/v1\";\n\nmessage Money {{\n  {money}\n}}\n\nmessage Order {{\n  Money total = 1;\n}}\n"
                    ),
                ),
                ("repo/gen/order/v1/order.pb.go", ORDER_GO),
                ("repo/app/app.go", APP_GO),
            ])
        };
        let go = [
            (
                "repo/db/sqlc/users.sql.go",
                "(*example.com/app/db.Queries).GetUser",
                "function",
                "func (q *Queries) GetUser(ctx context.Context, id int32) (User, error) {\n\treturn User{}, nil\n}",
            ),
            (
                "repo/gen/order/v1/order.pb.go",
                "Order",
                "type",
                "Order struct {\n}",
            ),
            (
                "repo/app/app.go",
                "example.com/app.Handle",
                "function",
                "func Handle(q *db.Queries) {\n\tq.GetUser(nil, 1)\n}",
            ),
            (
                "repo/app/app.go",
                "example.com/app.Total",
                "function",
                "func Total(o *orderv1.Order) {\n}",
            ),
            (
                "repo/app/app.go",
                "example.com/app.Other",
                "function",
                "func Other() {\n}",
            ),
        ];
        let calls = [
            (
                "example.com/app.Handle",
                "(*example.com/app/db.Queries).GetUser",
                "calls",
            ),
            ("example.com/app.Total", "Order", "uses_type"),
        ];
        let review = |base: &Input, head: &Input| {
            let request = Request {
                timeout_ms: None,
                compact_provenance: false,
                repository_id: "repo".into(),
                base: vec![],
                head: vec![],
                profile: profile(),
                max_depth: 8,
                max_tokens: 16000,
                previous_packet_id: None,
                business_context: None,
                reuse_text: false,
                reviewed_entries: vec![],
                provided_diff: vec![],
            };
            compare(
                base,
                head,
                &with_go(base, &go, &calls),
                &with_go(head, &go, &calls),
                &request,
            )
            .unwrap()
        };
        let named = |r: &Value, name: &str| {
            r["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["side"] == "head" && e["name"] == name)
        };
        let base = files("CREATE TABLE users (id int);\n", "int64 units = 1;");
        let migration = review(
            &base,
            &files("CREATE TABLE users (id bigint);\n", "int64 units = 1;"),
        );
        assert!(named(&migration, "GetUser"), "{migration}");
        assert!(named(&migration, "example.com/app.Handle"), "{migration}");
        assert!(!named(&migration, "example.com/app.Other"));
        assert!(!named(&migration, "example.com/app.Total"));
        let handle = migration["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "example.com/app.Handle")
            .unwrap();
        assert!(handle.to_string().contains("generated_from"), "{handle}");
        let money = review(
            &base,
            &files(
                "CREATE TABLE users (id int);\n",
                "int64 units = 1;\n  string currency = 2;",
            ),
        );
        assert!(named(&money, "example.com/app.Total"), "{money}");
        assert!(!named(&money, "example.com/app.Handle"));
    }

    #[test]
    fn a_repository_past_five_thousand_sources_still_resolves_imports() {
        let callers: Vec<(String, String)> = (0..6000)
            .map(|i| {
                (
                    format!("repo/src/c{i}.ts"),
                    format!("import {{ price }} from './price';\nexport function c{i}(): number {{\n  return price();\n}}\n"),
                )
            })
            .collect();
        let mut files = vec![(
            "repo/src/price.ts",
            "export function price(): number {\n  return 1;\n}\n",
        )];
        files.extend(callers.iter().map(|(p, t)| (p.as_str(), t.as_str())));
        let a = analysis(&input(&files));
        assert!(
            !a.issues.iter().any(|i| i["reason"] == "basic_import_limit"),
            "{:?}",
            a.issues
                .iter()
                .find(|i| i["reason"] == "basic_import_limit")
        );
        let target = symbol_key("repo/src/price.ts", "price");
        let callers = a
            .calls
            .iter()
            .filter(|(_, c)| c.target == target && c.relation == "may_call")
            .count();
        assert_eq!(callers, 6000);
    }

    #[test]
    fn workspace_package_imports_reach_the_member_source() {
        let files = |root: (&str, &str)| {
            input(&[
                root,
                (
                    "repo/plugins/core/package.json",
                    "{\"name\":\"@internal/core\",\"main\":\"src/index.ts\",\"scripts\":{\"build\":\"tsc\"}}\n",
                ),
                (
                    "repo/plugins/core/src/index.ts",
                    "export function price(): number {\n  return 1;\n}\n",
                ),
                (
                    "repo/plugins/app/package.json",
                    "{\"name\":\"@internal/app\"}\n",
                ),
                (
                    "repo/plugins/app/src/a.ts",
                    "import { price } from '@internal/core';\nexport function total(): number {\n  return price();\n}\n",
                ),
            ])
        };
        for root in [
            (
                "repo/package.json",
                "{\"workspaces\":{\"packages\":[\"plugins/*\"]}}\n",
            ),
            ("repo/pnpm-workspace.yaml", "packages:\n  - plugins/*\n"),
        ] {
            let a = analysis(&files(root));
            assert!(
                a.calls
                    .iter()
                    .any(|(_, c)| c.file == "repo/plugins/app/src/a.ts"
                        && c.target_file == "repo/plugins/core/src/index.ts"
                        && c.relation == "potential_import"),
                "{}",
                root.0
            );
            assert!(a.calls.iter().any(|(_, c)| c.relation == "may_call"
                && c.target == symbol_key("repo/plugins/core/src/index.ts", "price")));
            assert!(
                !a.issues
                    .iter()
                    .any(|i| i["reason"] == "basic_unresolved_import"
                        || i["reason"] == "basic_parse_incomplete"),
                "{:?}",
                a.issues
            );
            // A manifest is read for resolution only, not as declarations.
            for m in a.inputs.iter().filter(|i| {
                i["file"]
                    .as_str()
                    .is_some_and(|f| f.ends_with("package.json") || f.ends_with(".yaml"))
            }) {
                assert!(
                    m["declarations"].as_array().is_none_or(Vec::is_empty),
                    "{m}"
                );
            }
        }
    }

    #[test]
    fn many_files_keep_every_caller_in_a_stable_order() {
        // Past the threshold where file parsing runs on several threads.
        let callers: Vec<(String, String)> = (0..80)
            .map(|i| {
                (
                    format!("repo/src/caller{i:02}.ts"),
                    format!("import {{ price }} from './price';\n\nexport function caller{i:02}(): number {{\n  return price();\n}}\n"),
                )
            })
            .collect();
        let mut files = vec![(
            "repo/src/price.ts",
            "export function price(): number {\n  return 1;\n}\n",
        )];
        files.extend(callers.iter().map(|(p, t)| (p.as_str(), t.as_str())));
        let data = input(&files);
        let (first, second) = (analysis(&data), analysis(&data));
        let target = symbol_key("repo/src/price.ts", "price");
        let found: Vec<_> = first
            .calls
            .iter()
            .filter(|(_, c)| c.target == target && c.relation == "may_call")
            .map(|(_, c)| c.caller.clone())
            .collect();
        let expected: Vec<_> = (0..80)
            .map(|i| {
                symbol_key(
                    &format!("repo/src/caller{i:02}.ts"),
                    &format!("caller{i:02}"),
                )
            })
            .collect();
        assert_eq!(found, expected);
        let shape = |a: &Analysis| {
            (
                a.calls
                    .iter()
                    .map(|(w, c)| {
                        (
                            w.clone(),
                            c.caller.clone(),
                            c.target.clone(),
                            c.evidence.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
                a.symbols.keys().cloned().collect::<Vec<_>>(),
                serde_json::to_string(&a.inputs).unwrap(),
                serde_json::to_string(&a.issues).unwrap(),
            )
        };
        assert_eq!(shape(&first), shape(&second));
    }

    #[test]
    fn resolved_calls_select_the_changed_functions_callers() {
        let files = |body: &str| {
            input(&[
                (
                    "repo/src/price.ts",
                    &format!(
                        "export function price(): number {{\n  return {body};\n}}\n\nexport function tax(): number {{\n  return 7;\n}}\n"
                    ),
                ),
                (
                    "repo/src/cart.ts",
                    "import { price } from './price';\n\nexport function total(): number {\n  return price() + 1;\n}\n",
                ),
                (
                    "repo/src/report.ts",
                    "import { tax } from './price';\n\nexport function audit(): number {\n  return tax();\n}\n",
                ),
            ])
        };
        let (base, head) = (files("1"), files("2"));
        let a = analysis(&head);
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.caller == symbol_key("repo/src/cart.ts", "total")
                    && c.target == symbol_key("repo/src/price.ts", "price")
                    && c.relation == "may_call"
                    && c.evidence == "explicit_import_binding"),
            "{:?}",
            a.calls
                .iter()
                .map(|(_, c)| (&c.caller, &c.target, &c.relation))
                .collect::<Vec<_>>()
        );
        let r = report(&base, &head);
        let price = key(
            &world("repo/src/price.ts"),
            &symbol_key("repo/src/price.ts", "price"),
        );
        let change = r["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["head"] == json!(price))
            .unwrap_or_else(|| panic!("{}", r["changes"]));
        assert_eq!(change["signature_changed"], false);
        assert_eq!(change["implementation_changed"], true);
        let entries = r["entries"].as_array().unwrap();
        let caller = entries
            .iter()
            .find(|e| e["side"] == "head" && e["file"] == "src/cart.ts" && e["name"] == "total")
            .unwrap_or_else(|| panic!("{r:#}"));
        assert_eq!(caller["analysis_level"], "basic_symbol");
        assert_eq!(caller["language"], "typescript");
        assert!(caller["reasons"].as_array().unwrap().iter().any(|reason| {
            reason["reason"] == "potential_consumer"
                && reason["steps"][0]["relation"] == "may_call"
                && reason["steps"][0]["evidence"] == "explicit_import_binding"
        }));
        // A function calling only the unchanged `tax` is not a caller, while
        // its file stays an importer of the changed file.
        assert!(
            !entries
                .iter()
                .any(|e| e["file"] == "src/report.ts" && e["name"] == "audit")
        );
        assert!(selected(&r, "head", "src/report.ts"));
        assert_eq!(r["coverage"]["complete"], false);
    }

    #[test]
    fn minified_bundles_stay_file_level_without_declarations() {
        // A bundler's one-line output: thousands of declarations nobody wrote.
        let bundle = |n: usize| {
            let body: String = (0..2_000)
                .map(|i| format!("function f{i}(){{return {n};}}"))
                .collect();
            format!("{body}\n")
        };
        let files = |n: usize| {
            input(&[
                ("repo/vendor/bundle.js", &bundle(n)),
                ("repo/src/app.js", "export function app() { return 1; }\n"),
            ])
        };
        let head = analysis(&files(2));
        assert!(
            head.issues
                .iter()
                .any(|i| i["reason"] == "basic_minified_source"
                    && i["detail"]["file"] == "repo/vendor/bundle.js"),
            "{:#?}",
            head.issues
        );
        assert!(
            !head
                .symbols
                .values()
                .any(|(_, s)| s.file == "repo/vendor/bundle.js" && s.kind != "basic_file"),
            "no declarations from a bundle"
        );
        assert!(
            head.symbols
                .values()
                .any(|(_, s)| s.key == "symbol:repo/src/app.js#app")
        );
        // A change to it is still a changed file.
        let r = report(&files(1), &files(2));
        assert!(selected(&r, "head", "vendor/bundle.js"), "{r:#}");
    }

    #[test]
    fn precise_calls_reach_method_and_constructor_callers() {
        let files = |amount: &str| {
            input(&[
                (
                    "repo/src/price.ts",
                    &format!(
                        "export class Price {{\n  amount(): number {{\n    return {amount};\n  }}\n}}\n"
                    ),
                ),
                (
                    "repo/src/cart.ts",
                    "import { Price } from './price';\n\nexport function total(p: Price): number {\n  return p.amount() + 1;\n}\n",
                ),
                (
                    "repo/src/make.ts",
                    "import { Price } from './price';\n\nexport function make(): Price {\n  return new Price();\n}\n",
                ),
            ])
        };
        let r = report(&files("1"), &files("2"));
        let caller = |file: &str, name: &str, evidence: &str| {
            r["entries"].as_array().unwrap().iter().any(|e| {
                e["side"] == "head"
                    && e["file"] == file
                    && e["name"] == name
                    && e["reasons"].as_array().unwrap().iter().any(|reason| {
                        reason["reason"] == "potential_consumer"
                            && reason["steps"].as_array().is_some_and(|s| s.len() == 1)
                            && reason["steps"][0]["relation"] == "may_call"
                            && reason["steps"][0]["evidence"] == evidence
                    })
            })
        };
        // `p: Price` types the receiver; `new Price()` binds the class.
        assert!(caller("src/cart.ts", "total", "oxc_type"), "{r:#}");
        assert!(caller("src/make.ts", "make", "oxc_binding"), "{r:#}");
        assert_eq!(r["coverage"]["complete"], false);
    }

    #[test]
    fn imports_never_cross_repository_or_ambiguous_source_roots() {
        let data = input(&[
            ("one/app.py", "import tax\n"),
            ("two/tax.py", "def pay(): pass\n"),
            ("three/app.py", "import tax\n"),
            ("three/src/tax.py", "def pay(): pass\n"),
            ("three/lib/tax.py", "def pay(): pass\n"),
        ]);
        let a = analysis(&data);
        assert!(a.calls.is_empty());
        assert!(
            a.issues
                .iter()
                .any(|i| i["reason"] == "basic_unresolved_import")
        );
        assert!(
            !a.issues
                .iter()
                .any(|i| i["reason"] == "compiler_analysis_unavailable")
        );
    }

    #[test]
    fn rust_modules_and_manifest_changes_keep_source_context() {
        let base = input(&[
            (
                "repo/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            (
                "repo/src/lib.rs",
                "mod tax;\npub fn call() { tax::pay(); }\n",
            ),
            ("repo/src/tax.rs", "pub fn pay() {}\n"),
        ]);
        let mut head = input(&[
            (
                "repo/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.2.0\"\n",
            ),
            (
                "repo/src/lib.rs",
                "mod tax;\npub fn call() { tax::pay(); }\n",
            ),
            ("repo/src/tax.rs", "pub fn pay() {}\n"),
        ]);
        assert!(
            analysis(&base)
                .calls
                .iter()
                .any(|(_, c)| c.file == "repo/src/lib.rs" && c.target_file == "repo/src/tax.rs")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "src/lib.rs"));
        assert!(selected(&r, "head", "src/tax.rs"));
        head.sources.remove("repo/src/tax.rs");
        let deleted = report(&base, &head);
        assert!(selected(&deleted, "base", "src/tax.rs"));
        assert!(selected(&deleted, "head", "src/lib.rs"));
        assert!(
            deleted["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "deleted")
        );
    }

    #[test]
    fn shell_paths_are_candidates_and_dynamic_or_missing_sources_are_visible() {
        let data = input(&[
            (
                "repo/run",
                "#!/bin/bash\nsource ./lib.sh\nsource \"$CONFIG\"\nsource ../outside.sh\n# source fake.sh\necho 'source fake.sh'\n",
            ),
            ("repo/lib.sh", "work() { echo ok; }\n"),
            ("other/outside.sh", "echo other\n"),
        ]);
        let a = analysis(&data);
        assert_eq!(a.calls.len(), 1);
        assert_eq!(a.calls[0].1.target_file, "repo/lib.sh");
        assert_eq!(a.calls[0].1.evidence, "literal_shell_path_candidate");
        assert_eq!(
            a.issues
                .iter()
                .filter(|i| i["reason"] == "basic_unresolved_dependency")
                .count(),
            2
        );
    }

    #[test]
    fn terraform_local_modules_and_same_directory_inputs_form_potential_scope() {
        let root = "module \"child\" { source = \"./child\" }\nmodule \"remote\" { source = \"registry/vendor/mod\" }\n";
        let base = input(&[
            ("repo/main.tf", root),
            ("repo/variables.tf", "variable \"count\" { default = 1 }\n"),
            ("repo/child/main.tf", "output \"answer\" { value = 1 }\n"),
            ("repo/unrelated/main.tf", "output \"other\" { value = 0 }\n"),
        ]);
        let head = input(&[
            ("repo/main.tf", root),
            ("repo/variables.tf", "variable \"count\" { default = 1 }\n"),
            ("repo/child/main.tf", "output \"answer\" { value = 2 }\n"),
            ("repo/unrelated/main.tf", "output \"other\" { value = 0 }\n"),
        ]);
        let a = analysis(&head);
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "repo/main.tf" && c.target_file == "repo/child/main.tf")
        );
        assert!(
            a.issues
                .iter()
                .any(|i| i["reason"] == "basic_unresolved_dependency")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "main.tf"));
        assert!(selected(&r, "head", "variables.tf"));
        assert!(!selected(&r, "head", "unrelated/main.tf"));
    }

    #[test]
    fn helm_values_helpers_and_vendored_chart_remain_source_only() {
        let chart = "apiVersion: v2\nname: app\nversion: 0.1.0\n";
        let template = "kind: Deployment\nreplicas: {{ .Values.replicas }}\n";
        let base = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/values.yaml", "replicas: 1\n"),
            ("repo/app/templates/deploy.yaml", template),
            (
                "repo/app/templates/_helpers.tpl",
                "{{- define \"app.name\" -}}app{{- end -}}",
            ),
            ("repo/other/Chart.yaml", chart),
            ("repo/other/values.yaml", "replicas: 1\n"),
        ]);
        let head = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/values.yaml", "replicas: 2\n"),
            ("repo/app/templates/deploy.yaml", template),
            (
                "repo/app/templates/_helpers.tpl",
                "{{- define \"app.name\" -}}app{{- end -}}",
            ),
            ("repo/other/Chart.yaml", chart),
            ("repo/other/values.yaml", "replicas: 1\n"),
        ]);
        let r = report(&base, &head);
        assert!(selected(&r, "head", "app/templates/deploy.yaml"));
        assert!(selected(&r, "head", "app/templates/_helpers.tpl"));
        assert!(!selected(&r, "head", "other/values.yaml"));
        assert!(!r["entries"].as_array().unwrap().iter().any(|e| {
            e["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.get("execution_flow").is_some())
        }));
        let nested = input(&[
            ("repo/app/Chart.yaml", chart),
            ("repo/app/charts/child/Chart.yaml", chart),
        ]);
        assert!(
            analysis(&nested)
                .calls
                .iter()
                .any(|(_, c)| c.file == "repo/app/Chart.yaml"
                    && c.target_file == "repo/app/charts/child/Chart.yaml")
        );
        let missing = input(&[("repo/templates/deploy.yaml", template)]);
        assert!(
            analysis(&missing)
                .issues
                .iter()
                .any(|i| i["reason"] == "basic_missing_chart_metadata")
        );
    }

    #[test]
    fn malformed_source_and_budget_exhaustion_never_claim_complete_impact() {
        let base = input(&[("repo/bad.py", "def broken(:\n pass\n")]);
        let head = input(&[(
            "repo/bad.py",
            &format!("def broken(:\n pass\n#{}", " evidence".repeat(5000)),
        )]);
        assert!(
            analysis(&head)
                .issues
                .iter()
                .any(|i| i["reason"] == "basic_parse_incomplete")
        );
        let r = report(&base, &head);
        assert!(selected(&r, "head", "bad.py"));
        match pack(r, 2000, None, None) {
            Ok((packet, _)) => assert_eq!(packet["coverage"]["complete"], false),
            Err(error) => assert!(error.to_string().contains("budget")),
        }
    }

    #[test]
    fn delta_invalidates_deleted_basic_entries_and_cites_exact_bytes() {
        let base = input(&[("repo/a.py", "def pay(): return 1\n")]);
        let initial = input(&[("repo/a.py", "def pay(): return 2\n")]);
        let next = input(&[("repo/b.py", "def pay(): return 2\n")]);
        let (first, receipt) = pack(report(&base, &initial), 16000, None, None).unwrap();
        let (delta, _) = pack(report(&base, &next), 16000, Some(&receipt), None).unwrap();
        let previous = first["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head")
            .unwrap();
        assert!(
            delta["invalidated"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == previous["id"])
        );
        let current = delta["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["side"] == "head")
            .unwrap();
        assert_eq!(current["file"], "b.py");
        assert_eq!(
            current["file_sha256"],
            text::sha256(s(current, "text").as_bytes())
        );
    }

    #[test]
    fn workspace_and_requirements_changes_retain_potential_consumers() {
        let before = input(&[
            ("rust/Cargo.toml", "[workspace]\nmembers = [\"app\"]\n"),
            (
                "rust/app/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/app/src/lib.rs", "pub fn run() {}\n"),
            ("python/requirements.txt", "example==1\n"),
            ("python/src/app.py", "def run(): pass\n"),
        ]);
        let after = input(&[
            (
                "rust/Cargo.toml",
                "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
            ),
            (
                "rust/app/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/app/src/lib.rs", "pub fn run() {}\n"),
            ("python/requirements.txt", "example==2\n"),
            ("python/src/app.py", "def run(): pass\n"),
        ]);
        let r = report(&before, &after);
        assert!(selected(&r, "head", "app/src/lib.rs"));
        assert!(selected(&r, "head", "src/app.py"));
    }

    #[test]
    fn terraform_json_and_build_script_dependencies_are_conservative() {
        let data = input(&[
            (
                "repo/main.tf.json",
                r#"{"module":{"child":{"source":"./child"}}}"#,
            ),
            (
                "repo/terraform.tfvars.json",
                r#"{"module":{"child":{"source":"./child"}}}"#,
            ),
            ("repo/child/main.tf", "output \"answer\" { value = 2 }\n"),
            (
                "rust/Cargo.toml",
                "[package]\nname=\"app\"\nversion=\"0.1.0\"\n",
            ),
            ("rust/build.rs", "fn main() {}\n"),
            ("rust/src/lib.rs", "pub fn run() {}\n"),
        ]);
        let a = analysis(&data);
        assert!(
            a.calls.iter().any(
                |(_, c)| c.file == "repo/main.tf.json" && c.target_file == "repo/child/main.tf"
            )
        );
        assert!(
            a.calls
                .iter()
                .any(|(_, c)| c.file == "rust/Cargo.toml" && c.target_file == "rust/build.rs")
        );
        let invalid =
            graphicle::extractors::basic_dependencies::extract("bash", "source '\n").unwrap();
        assert!(!a.calls.iter().any(
            |(_, c)| c.file == "repo/terraform.tfvars.json" && c.relation == "potential_import"
        ));
        assert!(invalid["dependencies"].as_array().unwrap().is_empty());
        assert_eq!(invalid["issues"][0], "syntax_errors");
    }
}
