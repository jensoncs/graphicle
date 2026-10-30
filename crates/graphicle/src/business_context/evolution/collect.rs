use super::*;

struct Declaration {
    function: Function,
    body: String,
    syntax: String,
}
fn terms(source: &str) -> Vec<String> {
    source
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 3 && t.len() <= 64)
        .map(|t| text::sha256(t.to_lowercase().as_bytes()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(128)
        .collect()
}
fn declarations(path: &str, source: &str) -> Result<BTreeMap<String, Declaration>> {
    ensure_safe(source)?;
    let parsed = structure::analyze(path, source)
        .map_err(|_| Error::bad("Historical syntax unavailable"))?;
    if !structure::declaration_spans_complete(&parsed) {
        return Err(Error::bad("Historical declarations incomplete"));
    }
    let functions: Vec<_> = parsed["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| {
            matches!(
                store::s(s, "kind"),
                "function" | "method" | "function_definition" | "method_definition"
            )
        })
        .collect();
    if functions.len() > 512 {
        return Err(Error::bad("Historical function bound exceeded"));
    }
    let spans: Vec<_> = functions
        .iter()
        .map(|s| {
            (
                s["byte_start"].as_u64().unwrap_or(0),
                s["byte_end"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    let metrics = structure::declaration_metrics(path, source, &spans);
    let mut out = BTreeMap::new();
    let mut duplicate = BTreeSet::new();
    for (symbol, metric) in functions.into_iter().zip(metrics) {
        let Some(metric) = metric else {
            return Err(Error::bad("Historical syntax metric unavailable"));
        };
        let start = symbol["byte_start"].as_u64().unwrap() as usize;
        let end = symbol["byte_end"].as_u64().unwrap() as usize;
        let body = source
            .get(start..end)
            .ok_or_else(|| Error::bad("Invalid historical span"))?
            .to_owned();
        let function = Function {
            path: path.into(),
            file_sha256: text::sha256(source.as_bytes()),
            name: store::s(symbol, "name").into(),
            kind: store::s(symbol, "kind").into(),
            byte_start: start,
            byte_end: end,
            line_start: symbol["line"].as_u64().unwrap() as usize,
            line_end: symbol["end_line"].as_u64().unwrap() as usize,
            complexity: Some(metric.decision_complexity),
            terms: terms(&body),
        };
        let id = function.id();
        if out
            .insert(
                id.clone(),
                Declaration {
                    function,
                    body,
                    syntax: metric.syntax_sha256,
                },
            )
            .is_some()
        {
            duplicate.insert(id);
        }
    }
    // Overloads/duplicate names cannot be tracked by ordinal across revisions.
    if !duplicate.is_empty() {
        return Err(Error::bad("Ambiguous historical declarations"));
    }
    Ok(out)
}
fn noisy(path: &str) -> bool {
    path.split('/')
        .any(|part| matches!(part, "vendor" | "node_modules" | "generated" | "dist"))
        || path.ends_with(".pb.go")
        || path.ends_with(".gen.go")
        || path.ends_with(".min.js")
        || path.ends_with(".lock")
        || matches!(
            Path::new(path).file_name().and_then(|n| n.to_str()),
            Some("go.sum" | "package-lock.json" | "yarn.lock")
        )
}

fn in_modules(path: &str, pin: &Pin) -> bool {
    pin.modules
        .iter()
        .any(|module| module == "." || Path::new(path).starts_with(module))
}
fn blob(root: &Path, commit: &str, path: &str, deadline: Instant) -> Result<String> {
    let bytes = git(
        root,
        &["cat-file", "blob", &format!("{commit}:{path}")],
        deadline,
        1024 * 1024,
    )?;
    let source =
        String::from_utf8(bytes).map_err(|_| Error::bad("Historical input is not UTF-8"))?;
    ensure_safe(&source)?;
    if source.contains('\0') {
        return Err(Error::bad("Historical input is binary"));
    }
    Ok(source)
}
fn metadata(
    root: &Path,
    commit: &str,
    repo: &str,
    deadline: Instant,
) -> Result<(i64, String, String)> {
    let bytes = git(
        root,
        &[
            "show",
            "-s",
            "--no-show-signature",
            "--format=%ct%x00%ae%x00%s",
            commit,
            "--",
        ],
        deadline,
        64 * 1024,
    )?;
    let raw = String::from_utf8(bytes).map_err(|_| Error::bad("Invalid history metadata"))?;
    ensure_safe(&raw)?;
    let fields: Vec<_> = raw.trim_end().splitn(3, '\0').collect();
    if fields.len() != 3 || fields[2].len() > 512 || fields[2].chars().any(char::is_control) {
        return Err(Error::bad("Invalid history metadata"));
    }
    let time = fields[0]
        .parse()
        .map_err(|_| Error::bad("Invalid history time"))?;
    Ok((
        time,
        crate::digest(&json!([repo, fields[1].to_lowercase()])),
        fields[2].into(),
    ))
}
fn changes(
    root: &Path,
    commit: &str,
    parent: Option<&str>,
    deadline: Instant,
) -> Result<Vec<(String, String)>> {
    let mut args = vec![
        "diff-tree",
        "--no-ext-diff",
        "--no-textconv",
        "--no-commit-id",
        "--no-renames",
        "--name-status",
        "-r",
        "-z",
    ];
    if let Some(parent) = parent {
        args.extend([parent, commit]);
    } else {
        args.extend(["--root", commit]);
    }
    args.push("--");
    let bytes = git(root, &args, deadline, 256 * 1024)?;
    let raw = String::from_utf8(bytes).map_err(|_| Error::bad("Invalid historical paths"))?;
    let fields: Vec<_> = raw.split('\0').filter(|s| !s.is_empty()).collect();
    if !fields.len().is_multiple_of(2) {
        return Err(Error::bad("Invalid historical paths"));
    }
    Ok(fields
        .chunks_exact(2)
        .map(|p| (p[0].into(), p[1].into()))
        .collect())
}
fn causes(
    root: &Path,
    parent: &str,
    path: &str,
    ranges: &[(usize, usize)],
    deadline: Instant,
) -> Result<Vec<String>> {
    let mut commits = BTreeSet::new();
    for (start, end) in ranges.iter().take(4) {
        let blame = git(
            root,
            &[
                "blame",
                "--no-textconv",
                "--line-porcelain",
                "-L",
                &format!("{start},{end}"),
                parent,
                "--",
                path,
            ],
            deadline,
            1024 * 1024,
        )?;
        let raw = String::from_utf8(blame).map_err(|_| Error::bad("Invalid causal blame"))?;
        ensure_safe(&raw)?;
        for line in raw.lines().filter(|line| !line.starts_with('\t')) {
            if let Some(id) = line.split_whitespace().next()
                && commit_id(id)
            {
                commits.insert(id.to_owned());
            }
        }
    }
    Ok(commits.into_iter().take(4).collect())
}

/// Local-only analysis of first-parent integration commits. A squash commit is
/// naturally one unit; a merge is diffed against its first parent as one unit.
/// No PR identity is inferred from a commit subject and no remote calls run.
pub fn collect(root: &Path, mut history: History, max_bytes: usize) -> Result<History> {
    let mut budget = super::bounded::Budget::new(&mut history, max_bytes)?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let (anchor_time, _, _) = metadata(
        root,
        &history.pin.commit,
        &history.pin.repository_id,
        deadline,
    )?;
    let lines = git(
        root,
        &[
            "rev-list",
            "--first-parent",
            "--parents",
            "-n",
            "65",
            &history.pin.commit,
            "--",
        ],
        deadline,
        32 * 1024,
    )?;
    let lines = String::from_utf8(lines).map_err(|_| Error::bad("Invalid historical walk"))?;
    let mut analysis = Evolution {
        format: "graphicle.history-analysis.v1".into(),
        anchor_time,
        units: vec![],
        functions: vec![],
        omissions: BTreeMap::new(),
    };
    if lines.lines().count() > MAX_UNITS {
        analysis.omissions.insert("history_window".into(), 1);
    }
    let mut current_files = BTreeSet::new();
    // Explicit files must be present even if none changed within the window.
    for file in &history.files {
        let source = blob(root, &history.pin.commit, &file.path, deadline)?;
        for declaration in declarations(&file.path, &source)?.into_values() {
            budget.function(declaration.function, &mut analysis)?;
        }
        current_files.insert(file.path.clone());
    }
    let mut read_files = 0;
    for line in lines.lines().take(MAX_UNITS) {
        if Instant::now() >= deadline || read_files >= 128 {
            *analysis
                .omissions
                .entry("collection_bound".into())
                .or_default() += 1;
            break;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let commit = fields[0];
        let parent = fields.get(1).copied();
        let collected = (|| -> Result<Option<Unit>> {
            let (timestamp, author_id, subject) =
                metadata(root, commit, &history.pin.repository_id, deadline)?;
            let files = changes(root, commit, parent, deadline)?;
            if files.len() > 32 {
                *analysis.omissions.entry("mass_change".into()).or_default() += 1;
                return Ok(None);
            }
            if files.is_empty() {
                return Ok(None);
            }
            let files_touched = files.len();
            let mut changed = vec![];
            for (status, path) in files {
                if admitted_path(&path).is_err()
                    || !in_modules(&path, &history.pin)
                    || noisy(&path)
                    || graphicle::analyze::file_category(&path) != "code"
                {
                    *analysis
                        .omissions
                        .entry("excluded_path".into())
                        .or_default() += 1;
                    continue;
                }
                if !matches!(status.as_str(), "A" | "M" | "D" | "T") {
                    return Err(Error::bad("Unsupported historical status"));
                }
                read_files += 1;
                if read_files > 128 {
                    return Err(Error::bad("History file bound"));
                }
                let before = match parent {
                    Some(parent) if status != "A" => blob(root, parent, &path, deadline)?,
                    _ => String::new(),
                };
                let after = if status == "D" {
                    String::new()
                } else {
                    blob(root, commit, &path, deadline)?
                };
                if before
                    .lines()
                    .take(4)
                    .chain(after.lines().take(4))
                    .any(|line| line.contains("Code generated") && line.contains("DO NOT EDIT"))
                {
                    *analysis.omissions.entry("generated".into()).or_default() += 1;
                    continue;
                }
                let old = if before.is_empty() {
                    BTreeMap::new()
                } else {
                    declarations(&path, &before)?
                };
                let new = if after.is_empty() {
                    BTreeMap::new()
                } else {
                    declarations(&path, &after)?
                };
                let ids: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
                for id in ids {
                    let a = old.get(&id);
                    let b = new.get(&id);
                    if a.map(|v| &v.syntax) == b.map(|v| &v.syntax) {
                        continue;
                    }
                    let a_text = a.map_or("", |v| v.body.as_str());
                    let b_text = b.map_or("", |v| v.body.as_str());
                    let diff = similar::TextDiff::configure()
                        .deadline(deadline)
                        .diff_lines(a_text, b_text);
                    let mut added = 0;
                    let mut deleted = 0;
                    let mut patch = String::new();
                    let mut removed = vec![];
                    for change in diff.iter_all_changes() {
                        match change.tag() {
                            similar::ChangeTag::Insert => {
                                added += 1;
                                patch.push_str(change.value());
                            }
                            similar::ChangeTag::Delete => {
                                deleted += 1;
                                patch.push_str(change.value());
                                if let (Some(old), Some(index)) = (a, change.old_index()) {
                                    removed.push((
                                        old.function.line_start + index,
                                        old.function.line_start + index,
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                    let cause_candidates = if assertion_kind(&subject) == "correctness_candidate"
                        && !removed.is_empty()
                    {
                        parent
                            .map(|parent| causes(root, parent, &path, &removed, deadline))
                            .transpose()?
                            .unwrap_or_default()
                    } else {
                        vec![]
                    };
                    let full_patch = diff.unified_diff().context_radius(2).to_string();
                    let mut patch_excerpt = String::new();
                    for line in full_patch.split_inclusive('\n') {
                        if patch_excerpt.len() + line.len() > 2048 {
                            break;
                        }
                        patch_excerpt.push_str(line);
                    }
                    let patch_truncated = patch_excerpt.len() != full_patch.len();
                    let symbol = &b.or(a).unwrap().function;
                    changed.push(Change {
                        path: path.clone(),
                        name: symbol.name.clone(),
                        kind: symbol.kind.clone(),
                        before: a.map(|v| text::sha256(v.body.as_bytes())),
                        after: b.map(|v| text::sha256(v.body.as_bytes())),
                        added,
                        deleted,
                        patch_terms: terms(&patch),
                        patch_excerpt,
                        patch_truncated,
                        before_line: a.map(|d| d.function.line_start),
                        after_line: b.map(|d| d.function.line_start),
                        cause_candidates,
                    });
                }
                // Re-read candidates at the pin, never attach historical bytes
                // to current context by name. A deleted path is not a candidate.
                if !current_files.contains(&path) && current_files.len() < 32 {
                    current_files.insert(path.clone());
                    if let Ok(source) = blob(root, &history.pin.commit, &path, deadline)
                        && let Ok(parsed) = declarations(&path, &source)
                    {
                        for declaration in parsed.into_values() {
                            budget.function(declaration.function, &mut analysis)?;
                        }
                    }
                }
            }
            if changed.is_empty() {
                *analysis
                    .omissions
                    .entry("no_function_change".into())
                    .or_default() += 1;
                return Ok(None);
            }
            if changed.len() > 128 {
                return Err(Error::bad("History unit function bound"));
            }
            Ok(Some(Unit {
                commit: commit.into(),
                parent: parent.map(str::to_owned),
                timestamp,
                author_id,
                subject,
                files_touched,
                changes: changed,
                omitted_changes: 0,
            }))
        })();
        match collected {
            Ok(Some(unit)) => budget.unit(unit, &mut analysis)?,
            Ok(None) => {}
            Err(_) => {
                *analysis
                    .omissions
                    .entry("unavailable_or_withheld_unit".into())
                    .or_default() += 1;
            }
        }
    }
    budget.finish(&mut analysis);
    history.evolution = Some(analysis);
    ensure_safe_value(&serde_json::to_value(&history)?)?;
    if serde_json::to_vec(&history)?.len() > max_bytes {
        return Err(Error::bad(
            "History byte budget cannot hold retained metadata",
        ));
    }
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn syntax_noise_filter_keeps_literal_changes_and_ignores_formatting() {
        let original =
            "package p\nfunc F(x int) string { if x>0 {return \"a  b\"}; return \"\" }\n";
        let formatted = "package p\nfunc F(x int) string {\n// new comment\nif x > 0 { return \"a  b\" }; return \"\"\n}\n";
        let changed = original.replace("a  b", "a b");
        let extract = |text: &str| {
            declarations("main.go", text)
                .unwrap()
                .into_values()
                .next()
                .unwrap()
        };
        let a = extract(original);
        let b = extract(formatted);
        let c = extract(&changed);
        assert_eq!(a.syntax, b.syntax);
        assert_ne!(a.syntax, c.syntax);
        assert_eq!(a.function.complexity, Some(2));
    }
    #[test]
    fn credential_shaped_source_is_rejected_before_indexing() {
        let token = format!("ghp_{}", "q".repeat(36));
        let source = format!("package p\nfunc F() string {{ return \"{token}\" }}\n");
        assert!(declarations("main.go", &source).is_err());
        assert!(ensure_safe(&format!("Fix with {token}")).is_err());
        assert!(noisy("vendor/example/a.go"));
        assert!(noisy("api.pb.go"));
    }
    #[test]
    fn syntax_parser_never_guesses_ambiguous_or_broken_symbols() {
        assert!(declarations("broken.go", "package p\nfunc F( {bad").is_err());
        // Both declarations are syntactically valid, but name-based historical
        // correspondence cannot choose which incarnation is which.
        let duplicates =
            "package p\nfunc F(a int) int { return a }\nfunc F(a string) int { return 0 }\n";
        assert!(declarations("duplicate.go", duplicates).is_err());
    }
    #[test]
    fn metrics_follow_declarations_in_python_javascript_and_typescript() {
        for (path, source) in [
            (
                "a.py",
                "def F(x):\n    if x:\n        return 1\n    return 0\ndef G():\n    return 2\n",
            ),
            (
                "a.js",
                "function F(x) { if(x) return 1; return 0; }\nfunction G() { return 2; }\n",
            ),
            (
                "a.ts",
                "function F(x: boolean): number { if(x) return 1; return 0; }\nfunction G(): number { return 2; }\n",
            ),
        ] {
            let declarations = declarations(path, source).unwrap();
            assert_eq!(declarations.len(), 2, "{path}");
            for d in declarations.values() {
                assert_eq!(
                    d.function.complexity,
                    Some(if d.function.name == "F" { 2 } else { 1 }),
                    "{path}: {}",
                    d.function.name
                );
            }
        }
    }
}
