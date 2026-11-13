//! Resolves what `basic_refs` read against the snapshot: a path beside the
//! file or from its repository root, a stylesheet import also through the
//! load paths its build states, a search-path suffix, a declaration of that
//! name. Also the chart files a Helm template loads and the files a Go
//! `//go:embed` embeds. Every target is a candidate, never a build fact.
use super::basic::{ancestor_file, edge, file_key, link, relative, symbol_key, world};
use super::basic_refs::{Facts, To};
use super::*;
use std::collections::HashMap;

/// A directory names at most this many files; `COPY . .` names none.
// ponytail: a fixed cap on directory fan-out; per-kind limits if one needs more.
const DIRECTORY_FILES: usize = 64;

fn repo(path: &str) -> &str {
    path.split('/').next().unwrap_or("")
}

fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// The files under `prefix` (a directory, or a glob's literal head) that
/// `matches` accepts, in order; None past `limit`.
fn under(
    files: &BTreeMap<String, &'static str>,
    prefix: &str,
    limit: usize,
    matches: impl Fn(&str) -> bool,
) -> Option<Vec<String>> {
    let found: Vec<String> = files
        .range(prefix.to_owned()..)
        .map(|(p, _)| p)
        .take_while(|p| p.starts_with(prefix))
        .filter(|p| matches(p))
        .take(limit + 1)
        .cloned()
        .collect();
    (found.len() <= limit).then_some(found)
}

fn glob_matches(
    files: &BTreeMap<String, &'static str>,
    pattern: &str,
    limit: usize,
) -> Vec<String> {
    let Ok(compiled) = glob::Pattern::new(pattern) else {
        return vec![];
    };
    let options = glob::MatchOptions {
        require_literal_separator: true,
        ..Default::default()
    };
    let head = &pattern[..pattern.find(['*', '?', '[']).unwrap_or(pattern.len())];
    under(
        files,
        &head[..head.rfind('/').map_or(0, |i| i + 1)],
        limit,
        |p| compiled.matches_with(p, options),
    )
    .unwrap_or_default()
}

/// A path written in `file`: beside it, else from its repository root. A
/// directory means its first `index` file, or, with no index, its files.
fn paths(
    files: &BTreeMap<String, &'static str>,
    file: &str,
    literal: &str,
    index: &[&str],
) -> Vec<String> {
    if literal.contains("://") || literal.contains(['$', '{']) {
        return vec![];
    }
    let root = format!("{}/_", repo(file));
    let mut bases: Vec<String> = [relative(file, literal), relative(&root, literal)]
        .into_iter()
        .flatten()
        .collect();
    bases.dedup();
    for base in bases {
        if glob(&base) {
            let found = glob_matches(files, &base, DIRECTORY_FILES);
            if !found.is_empty() {
                return found;
            }
            continue;
        }
        if files.contains_key(&base) {
            return vec![base];
        }
        if let Some(found) = index
            .iter()
            .map(|i| format!("{base}/{i}"))
            .find(|p| files.contains_key(p))
        {
            return vec![found];
        }
        if index.is_empty()
            && base != repo(file)
            && let Some(found) = under(files, &format!("{base}/"), DIRECTORY_FILES, |_| true)
            && !found.is_empty()
        {
            return found;
        }
    }
    vec![]
}

/// The first of `names` beside `file`, else the one source of the repository
/// whose path ends with it.
fn suffix(
    files: &BTreeMap<String, &'static str>,
    by_base: &HashMap<(&str, &str), Vec<&str>>,
    file: &str,
    names: &[String],
) -> Option<String> {
    names.iter().find_map(|name| {
        if let Some(beside) = relative(file, name).filter(|p| files.contains_key(p)) {
            return Some(beside);
        }
        let base = name.rsplit('/').next().unwrap_or(name);
        let tail = format!("/{}", name.trim_start_matches("./"));
        let found: Vec<_> = by_base
            .get(&(repo(file), base))?
            .iter()
            .filter(|p| p.ends_with(&tail))
            .collect();
        match found[..] {
            [one] => Some((*one).to_owned()),
            _ => None,
        }
    })
}

/// The folders a stylesheet's build adds to its import search: each
/// `stylePreprocessorOptions.includePaths` of the angular.json projects whose
/// root holds the file, and of an Nx project.json, from the workspace root.
fn load_paths(input: &Input, file: &str) -> Vec<String> {
    let json = |path: &str| {
        input.sources[path]
            .content
            .text()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or_default()
    };
    let includes = |targets: &Value, root: &str| -> Vec<String> {
        targets
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(_, t)| {
                t["options"]["stylePreprocessorOptions"]["includePaths"]
                    .as_array()
                    .into_iter()
                    .flatten()
            })
            .filter_map(|p| relative(&format!("{root}/_"), p.as_str()?))
            .collect()
    };
    let mut found = vec![];
    if let Some(workspace) = ancestor_file(input, file, "angular.json") {
        let root = dir(&workspace);
        let projects = json(&workspace);
        for (_, project) in projects["projects"].as_object().into_iter().flatten() {
            let Some(base) = relative(&format!("{root}/_"), project["root"].as_str().unwrap_or(""))
            else {
                continue;
            };
            if file.starts_with(&format!("{base}/")) {
                found.extend(includes(&project["architect"], root));
                found.extend(includes(&project["targets"], root));
            }
        }
    }
    if let Some(project) = ancestor_file(input, file, "project.json")
        && let Some(nx) = ancestor_file(input, &project, "nx.json")
    {
        found.extend(includes(&json(&project)["targets"], dir(&nx)));
    }
    found
}

/// The first of `names` beside `file`, then in each load path, then from the
/// repository root: where Sass and Less look for an import, in order.
fn style(
    files: &BTreeMap<String, &'static str>,
    file: &str,
    names: &[String],
    load: &[String],
) -> Option<String> {
    std::iter::once(dir(file))
        .chain(load.iter().map(String::as_str))
        .chain(std::iter::once(repo(file)))
        .find_map(|base| {
            names
                .iter()
                .filter_map(|name| relative(&format!("{base}/_"), name))
                .find(|p| files.contains_key(p))
        })
}

/// The innermost declaration of `file` around `at`, else the file.
fn owner(file: &str, facts: &Facts, at: usize, declared: &BTreeSet<String>) -> String {
    facts
        .decls
        .iter()
        .filter(|d| d.start <= at && at < d.end.max(d.start + 1))
        .map(|d| (d.end - d.start, symbol_key(file, &d.name)))
        .filter(|(_, key)| declared.contains(key))
        .min()
        .map_or_else(|| file_key(file), |(_, key)| key)
}

pub(super) fn references(
    input: &Input,
    files: &BTreeMap<String, &'static str>,
    facts: &BTreeMap<&str, Facts>,
    declared: &BTreeSet<String>,
    out: &mut Analysis,
) -> Result<()> {
    let mut by_name = HashMap::<(&str, &str), Vec<&str>>::new();
    for (file, f) in facts {
        for d in &f.decls {
            if declared.contains(&symbol_key(file, &d.name)) {
                by_name.entry((repo(file), &d.name)).or_default().push(file);
            }
        }
    }
    let mut by_base = HashMap::<(&str, &str), Vec<&str>>::new();
    for file in files.keys() {
        by_base
            .entry((repo(file), file.rsplit('/').next().unwrap_or(file)))
            .or_default()
            .push(file);
    }
    // Load paths by folder: every stylesheet of a folder shares its build.
    let mut load = HashMap::<&str, Vec<String>>::new();
    for (file, f) in facts {
        checkpoint()?;
        let content = input.sources[*file].content.text().unwrap_or("");
        let line = super::basic_proto::lines(content);
        for r in &f.refs {
            let targets: Vec<(String, String)> = match &r.to {
                To::Path {
                    literal,
                    index,
                    required,
                } => {
                    let found = paths(files, file, literal, index);
                    if found.is_empty() && *required {
                        out.issues.push(issue(
                            "basic_unresolved_dependency",
                            json!({"file":file,"line":line(r.start),"kind":r.evidence,"path":literal,"cause":"missing_pinned_target"}),
                        ));
                    }
                    found.into_iter().map(|t| (file_key(&t), t)).collect()
                }
                To::Suffix(names) => suffix(files, &by_base, file, names)
                    .map(|t| (file_key(&t), t))
                    .into_iter()
                    .collect(),
                To::Style(names) => {
                    let load = load
                        .entry(dir(file))
                        .or_insert_with(|| load_paths(input, file));
                    style(files, file, names, load)
                        .map(|t| (file_key(&t), t))
                        .into_iter()
                        .collect()
                }
                To::Decl { name, wide } => {
                    let own = symbol_key(file, name);
                    if declared.contains(&own) {
                        vec![(own, (*file).to_owned())]
                    } else if *wide {
                        let found = by_name.get(&(repo(file), name.as_str()));
                        let all: Vec<&str> = found.into_iter().flatten().copied().collect();
                        let near: Vec<&str> = all
                            .iter()
                            .copied()
                            .filter(|f| dir(f) == dir(file))
                            .collect();
                        match (&near[..], &all[..]) {
                            ([one], _) | ([], [one]) => {
                                vec![(symbol_key(one, name), (*one).to_owned())]
                            }
                            _ => vec![],
                        }
                    } else {
                        vec![]
                    }
                }
            };
            if targets.is_empty() {
                continue;
            }
            let caller = owner(file, f, r.start, declared);
            for (target, to) in targets {
                edge(
                    out,
                    (file, &caller),
                    (&to, &target),
                    r.relation,
                    r.evidence,
                    line(r.start),
                    r.start..r.end,
                );
            }
        }
    }
    Ok(())
}

static FILES: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"\.Files\.(?:Get|GetBytes|Glob|Lines)\s+"([^"]+)""#).unwrap()
});

/// The chart files a Helm template loads with `.Files`, from the chart root.
pub(super) fn helm_files(
    input: &Input,
    files: &BTreeMap<String, &'static str>,
    out: &mut Analysis,
) -> Result<()> {
    for (file, _) in files.iter().filter(|(_, l)| **l == "helm") {
        let Some(content) = input.sources[file]
            .content
            .text()
            .filter(|c| c.contains(".Files."))
        else {
            continue;
        };
        checkpoint()?;
        let Some(chart) = ancestor_file(input, file, "Chart.yaml") else {
            continue;
        };
        let line = super::basic_proto::lines(content);
        for c in FILES.captures_iter(content) {
            let m = c.get(1).unwrap();
            let Some(target) = relative(&chart, m.as_str()) else {
                continue;
            };
            let found = if glob(&target) {
                glob_matches(files, &target, DIRECTORY_FILES)
            } else {
                vec![target]
                    .into_iter()
                    .filter(|t| files.contains_key(t))
                    .collect()
            };
            for target in found {
                link(
                    out,
                    file,
                    &target,
                    "potential_import",
                    "helm_files_path",
                    line(m.start()),
                    m.start()..m.end(),
                );
            }
        }
    }
    Ok(())
}

/// Embedded files: at most this many per pattern.
const EMBED_FILES: usize = 256;

/// Links each Go variable that Go review declares to the basic sources its
/// `//go:embed` patterns name, across worlds, so a changed embedded file
/// reaches the Go code that reads it.
pub(super) fn go_embeds(
    input: &Input,
    files: &BTreeMap<String, &'static str>,
    out: &mut Analysis,
) -> Result<()> {
    let mut variables = HashMap::<&str, Vec<(usize, usize, &str)>>::new();
    for (id, (world, symbol)) in &out.symbols {
        if !world.starts_with("basic:") && symbol.kind == "variable" {
            variables
                .entry(symbol.file.as_str())
                .or_default()
                .push((symbol.start, symbol.end, id));
        }
    }
    let mut edges = vec![];
    for (path, source) in input.sources.iter().filter(|(p, _)| p.ends_with(".go")) {
        let Some(content) = source.content.text().filter(|c| c.contains("//go:embed")) else {
            continue;
        };
        checkpoint()?;
        let mut patterns: Vec<&str> = vec![];
        let mut at = 0;
        for text in content.split_inclusive('\n') {
            let start = at;
            at += text.len();
            let trimmed = text.trim();
            if let Some(rest) = trimmed.strip_prefix("//go:embed") {
                patterns.extend(rest.split_whitespace().map(|p| p.trim_matches(['"', '`'])));
                continue;
            }
            if trimmed.is_empty() || trimmed.starts_with("//") || patterns.is_empty() {
                continue;
            }
            let offset = start + text.len() - text.trim_start().len();
            let variable = variables
                .get(path.as_str())
                .into_iter()
                .flatten()
                .find(|(s, e, _)| (*s..*e).contains(&offset));
            for pattern in std::mem::take(&mut patterns) {
                let Some(variable) = variable else { break };
                let hidden = !pattern.starts_with("all:");
                let pattern = pattern.trim_start_matches("all:");
                let Some(target) = relative(path, pattern) else {
                    continue;
                };
                let visible = |p: &str| {
                    !hidden
                        || !p[target.len()..]
                            .split('/')
                            .any(|part| part.starts_with(['.', '_']))
                };
                let found = if glob(&target) {
                    glob_matches(files, &target, EMBED_FILES)
                } else if files.contains_key(&target) {
                    vec![target.clone()]
                } else {
                    under(files, &format!("{target}/"), EMBED_FILES, visible).unwrap_or_default()
                };
                for file in found {
                    edges.push(services::Edge {
                        from: variable.2.to_owned(),
                        to: key(&world(&file), &file_key(&file)),
                        relation: "potential_import",
                        evidence: json!({"evidence":"go_embed_pattern","pattern":pattern,"go_file":path,"file":file}),
                    });
                }
            }
        }
    }
    out.services.edges.extend(edges);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> BTreeMap<String, &'static str> {
        paths.iter().map(|p| ((*p).to_owned(), "yaml")).collect()
    }

    #[test]
    fn paths_resolve_beside_the_file_then_from_the_root_through_indexes_and_globs() {
        let files = set(&[
            "r/a/x.yaml",
            "r/x.yaml",
            "r/b/kustomization.yaml",
            "r/c/1.json",
            "r/c/2.json",
            "r/c/sub/3.json",
            "r/s/run.sh",
        ]);
        let p = |file: &str, lit: &str, index: &[&str]| paths(&files, file, lit, index);
        assert_eq!(p("r/a/k.yaml", "x.yaml", &[]), ["r/a/x.yaml"]);
        assert_eq!(p("r/z/k.yaml", "x.yaml", &[]), ["r/x.yaml"]);
        assert_eq!(
            p("r/a/k.yaml", "../b", &["kustomization.yaml"]),
            ["r/b/kustomization.yaml"]
        );
        assert_eq!(
            p("r/Dockerfile", "c/*.json", &[]),
            ["r/c/1.json", "r/c/2.json"]
        );
        assert_eq!(p("r/Dockerfile", "c", &[]).len(), 3);
        assert!(
            p("r/Dockerfile", ".", &[]).is_empty(),
            "the repository is no directory reference"
        );
        assert!(p("r/Dockerfile", "/etc/x.yaml", &[]).is_empty());
        assert!(p("r/Dockerfile", "../../x.yaml", &[]).is_empty());
        assert_eq!(
            p("r/.github/workflows/c.yml", "./s/run.sh", &[]),
            ["r/s/run.sh"]
        );
    }

    #[test]
    fn a_suffix_resolves_beside_the_file_or_to_the_one_source_ending_in_it() {
        let files = set(&["r/inc/lib/u.h", "r/src/u.h", "r/other/x/u.h", "r/t/a.njk"]);
        let mut by_base = HashMap::<(&str, &str), Vec<&str>>::new();
        for f in files.keys() {
            by_base
                .entry(("r", f.rsplit('/').next().unwrap()))
                .or_default()
                .push(f);
        }
        let s = |file: &str, name: &str| suffix(&files, &by_base, file, &[name.to_owned()]);
        assert_eq!(s("r/src/m.c", "u.h").as_deref(), Some("r/src/u.h"));
        assert_eq!(s("r/main.c", "lib/u.h").as_deref(), Some("r/inc/lib/u.h"));
        assert_eq!(s("r/main.c", "u.h"), None, "ambiguous");
        assert_eq!(s("r/views/p.njk", "a.njk").as_deref(), Some("r/t/a.njk"));
    }
}
