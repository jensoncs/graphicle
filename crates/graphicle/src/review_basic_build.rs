//! Dockerfiles, Makefiles and GitHub Actions workflows: stages, targets and
//! jobs are declarations; what they copy, run, include, need or use is named.
use super::basic_refs::{Decl, Facts, Ref, To, path_like, shell_paths};

/// Lines with their byte offsets.
fn lines(content: &str) -> impl Iterator<Item = (usize, &str)> {
    content.split_inclusive('\n').scan(0, |at, line| {
        let start = *at;
        *at += line.len();
        Some((start, line))
    })
}

/// Each instruction with its continuation lines, and where it starts.
fn instructions(content: &str) -> Vec<(usize, &str)> {
    let mut out: Vec<(usize, usize)> = vec![];
    let mut continued = false;
    for (start, line) in lines(content) {
        let text = line.trim_end();
        let comment = text.trim_start().starts_with('#');
        if continued {
            if let Some(last) = out.last_mut() {
                last.1 = start + line.len();
            }
        } else if !text.trim_start().is_empty() && !comment {
            out.push((start, start + line.len()));
        }
        // A comment line neither starts nor ends a continuation: Docker drops it.
        if !comment {
            continued = text.ends_with('\\');
        }
    }
    out.into_iter().map(|(s, e)| (s, &content[s..e])).collect()
}

pub(super) fn dockerfile(content: &str) -> Facts {
    let mut f = Facts::default();
    let mut stages: Vec<String> = vec![];
    for (start, text) in instructions(content) {
        let body = text.trim_start();
        let Some((keyword, args)) = body.split_once(char::is_whitespace) else {
            continue;
        };
        let args_at = start + offset_in(text, args);
        let words: Vec<(usize, &str)> = args
            .split_whitespace()
            .map(|w| (start + offset_in(text, w), w))
            .collect();
        match keyword.to_ascii_uppercase().as_str() {
            "FROM" => {
                if let Some(open) = f.decls.last_mut().filter(|d| d.end == usize::MAX) {
                    open.end = start;
                }
                let plain: Vec<_> = words.iter().filter(|(_, w)| !w.starts_with("--")).collect();
                if let Some((at, image)) = plain.first()
                    && stages.iter().any(|s| s == image)
                {
                    f.decl_ref(
                        image,
                        false,
                        *at,
                        at + image.len(),
                        "uses_value",
                        "docker_stage_base",
                    );
                }
                if let [_, (_, r#as), (_, name)] = plain[..]
                    && r#as.eq_ignore_ascii_case("as")
                {
                    stages.push(name.to_string());
                    f.decls.push(Decl {
                        name: name.to_string(),
                        kind: "docker_stage",
                        start,
                        body_start: start + text.trim_end().len(),
                        end: usize::MAX,
                    });
                }
            }
            "COPY" | "ADD" => {
                let from = words
                    .iter()
                    .find_map(|(at, w)| w.strip_prefix("--from=").map(|s| (at + 7, s)));
                if let Some((at, stage)) = from {
                    f.decl_ref(
                        stage,
                        false,
                        at,
                        at + stage.len(),
                        "uses_value",
                        "docker_copy_from_stage",
                    );
                    continue;
                }
                let sources: Vec<_> = words.iter().filter(|(_, w)| !w.starts_with("--")).collect();
                // The last word is the destination; the rest are sources in
                // the build context, which is not known: beside the
                // Dockerfile, or the repository root.
                for (at, source) in sources.iter().rev().skip(1) {
                    if *source != "." && !source.contains("://") {
                        f.refs.push(Ref {
                            to: To::Path {
                                literal: source.to_string(),
                                index: &[],
                                required: false,
                            },
                            start: *at,
                            end: at + source.len(),
                            relation: "potential_import",
                            evidence: "docker_copy_source",
                        });
                    }
                }
            }
            _ => shell_paths(&mut f, args, args_at),
        }
    }
    if let Some(open) = f.decls.last_mut().filter(|d| d.end == usize::MAX) {
        open.end = content.trim_end().len().max(open.body_start);
    }
    f
}

/// Where `word`, a subslice of `text`, starts in it.
fn offset_in(text: &str, word: &str) -> usize {
    word.as_ptr() as usize - text.as_ptr() as usize
}

/// A rule line's targets and prerequisites, or None for any other line.
fn rule(line: &str) -> Option<(&str, &str)> {
    if line.starts_with(['\t', ' ', '#']) {
        return None;
    }
    let colon = line.find(':')?;
    let (targets, rest) = line.split_at(colon);
    let rest = rest.trim_start_matches(':');
    if targets.contains(['=', '$', '%']) || rest.starts_with('=') {
        return None;
    }
    let rest = rest.split([';', '#']).next().unwrap_or("");
    Some((targets, rest))
}

pub(super) fn makefile(content: &str) -> Facts {
    let mut f = Facts::default();
    let all: Vec<(usize, &str)> = lines(content).collect();
    for (i, (start, line)) in all.iter().enumerate() {
        let body = line.trim_end();
        if let Some(rest) = ["include ", "-include ", "sinclude "]
            .iter()
            .find_map(|p| body.strip_prefix(p))
        {
            for word in rest.split_whitespace().filter(|w| !w.contains('$')) {
                f.path(word, start + offset_in(line, word), "make_include");
            }
            continue;
        }
        if line.starts_with('\t') {
            shell_paths(&mut f, body, *start);
            continue;
        }
        let Some((targets, prerequisites)) = rule(body) else {
            continue;
        };
        // `.PHONY:` and other special targets declare nothing.
        if targets.split_whitespace().all(|t| t.starts_with('.')) {
            continue;
        }
        // The recipe: the tab-indented lines that follow, past blank lines
        // and comments between them.
        let mut end = start + body.len();
        for (s, l) in &all[i + 1..] {
            if l.starts_with('\t') {
                end = s + l.trim_end().len();
            } else if !(l.trim().is_empty() || l.trim_start().starts_with('#')) {
                break;
            }
        }
        for target in targets.split_whitespace().filter(|t| !t.starts_with('.')) {
            f.decls.push(Decl {
                name: target.into(),
                kind: "make_target",
                start: *start,
                body_start: start + body.len(),
                end,
            });
        }
        for word in prerequisites.split_whitespace().filter(|w| *w != "|") {
            let at = start + offset_in(line, word);
            f.decl_ref(
                word,
                false,
                at,
                at + word.len(),
                "may_call",
                "make_prerequisite",
            );
            if path_like(word) {
                f.path(word, at, "make_prerequisite_file");
            }
        }
    }
    f
}

const ACTION_FILES: &[&str] = &["action.yml", "action.yaml"];

pub(super) fn workflow(content: &str) -> Facts {
    let mut f = Facts::default();
    let all: Vec<(usize, &str)> = lines(content).collect();
    let significant = |l: &str| !l.trim().is_empty() && !l.trim_start().starts_with('#');
    // Jobs: the keys one level under the top-level `jobs:`.
    if let Some(jobs) = all.iter().position(|(_, l)| l.trim_end() == "jobs:") {
        let mut indent = None;
        for (s, l) in all[jobs + 1..].iter().filter(|(_, l)| significant(l)) {
            let depth = l.len() - l.trim_start().len();
            if depth == 0 {
                break;
            }
            let indent = *indent.get_or_insert(depth);
            let line_end = s + l.trim_end().len();
            match l
                .trim()
                .strip_suffix(':')
                .filter(|n| depth == indent && !n.contains(' '))
            {
                Some(name) => f.decls.push(Decl {
                    name: name.trim_matches(['"', '\'']).into(),
                    kind: "workflow_job",
                    start: *s,
                    body_start: line_end,
                    end: line_end,
                }),
                None => {
                    if let Some(open) = f.decls.last_mut() {
                        open.end = line_end;
                    }
                }
            }
        }
    }
    let parsed = graphicle::extractors::yaml_frontmatter::parse(content).unwrap_or_default();
    let starts: Vec<(String, usize, usize)> = f
        .decls
        .iter()
        .map(|d| (d.name.clone(), d.start, d.body_start))
        .collect();
    for (name, start, header_end) in starts {
        let needs = &parsed["jobs"][&name]["needs"];
        let needs: Vec<&str> = match needs {
            serde_json::Value::String(one) => vec![one],
            serde_json::Value::Array(many) => many.iter().filter_map(|n| n.as_str()).collect(),
            _ => vec![],
        };
        for need in needs {
            f.decl_ref(
                need,
                false,
                start,
                header_end,
                "uses_value",
                "workflow_needs",
            );
        }
    }
    for (s, l) in &all {
        let body = l.trim_start().trim_start_matches("- ").trim_start();
        if let Some(value) = body.strip_prefix("uses:") {
            let value = value.trim().trim_matches(['"', '\'']);
            if let Some(local) = value.strip_prefix("./") {
                // A local action or reusable workflow, from the repository root.
                let at = s + l.find(local).unwrap_or(0);
                f.refs.push(Ref {
                    to: To::Path {
                        literal: local.into(),
                        index: ACTION_FILES,
                        required: true,
                    },
                    start: at,
                    end: at + local.len(),
                    relation: "potential_import",
                    evidence: "workflow_local_uses",
                });
            }
            continue;
        }
        shell_paths(&mut f, l, *s);
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(f: &Facts) -> Vec<(String, &'static str)> {
        f.refs
            .iter()
            .map(|r| match &r.to {
                To::Path { literal, .. } => (literal.clone(), r.evidence),
                To::Suffix(names) | To::Style(names) => (names.join("|"), r.evidence),
                To::Decl { name, .. } => (format!("#{name}"), r.evidence),
            })
            .collect()
    }

    #[test]
    fn docker_stages_span_to_the_next_from_and_name_copies_and_scripts() {
        let text = "# syntax\nFROM golang:1.22 AS build\nCOPY go.mod scripts/b.sh ./\nRUN ./scripts/b.sh \\\n  && make\n\nFROM build AS test\nFROM alpine\nCOPY --from=build /out /app\nCMD [\"tools/run.py\"]\n";
        let f = dockerfile(text);
        let names: Vec<_> = f
            .decls
            .iter()
            .map(|d| (d.name.as_str(), &text[d.start..d.end]))
            .collect();
        assert_eq!(names[0].0, "build");
        assert!(names[0].1.ends_with("&& make\n\n"), "{:?}", names[0].1);
        assert_eq!(names[1], ("test", "FROM build AS test\n"));
        assert_eq!(f.decls.len(), 2, "an unnamed stage is not a declaration");
        assert_eq!(
            refs(&f),
            [
                ("scripts/b.sh".into(), "docker_copy_source"),
                ("go.mod".into(), "docker_copy_source"),
                ("./scripts/b.sh".into(), "literal_path_in_command"),
                ("#build".into(), "docker_stage_base"),
                ("#build".into(), "docker_copy_from_stage"),
                ("tools/run.py".into(), "literal_path_in_command"),
            ]
        );
        for r in &f.refs {
            let written = &text[r.start..r.end];
            assert!(
                refs(&f)
                    .iter()
                    .any(|(l, _)| l.trim_start_matches('#') == written),
                "{written}"
            );
        }
    }

    #[test]
    fn a_comment_ending_in_a_backslash_continues_nothing() {
        let text = "# run: docker run x \\\nFROM alpine\nRUN a \\\n# note \\\n  ./b.sh\nCMD c\n";
        let found = instructions(text);
        assert_eq!(
            found.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            [
                "FROM alpine\n",
                "RUN a \\\n# note \\\n  ./b.sh\n",
                "CMD c\n"
            ]
        );
        assert!(dockerfile(text).decls.is_empty());
    }

    #[test]
    fn make_targets_span_their_recipes_and_name_prerequisites() {
        let text = "include a.mk\nX := y\nall: build lib/x.c | dir\n\t./run.sh\n\n# c\n\tmore\nV = a:b\nbuild:\n\techo\n.PHONY: all\n$(BIN): x\n";
        let f = makefile(text);
        let decls: Vec<_> = f
            .decls
            .iter()
            .map(|d| (d.name.as_str(), &text[d.start..d.end]))
            .collect();
        assert_eq!(
            decls,
            [
                ("all", "all: build lib/x.c | dir\n\t./run.sh\n\n# c\n\tmore"),
                ("build", "build:\n\techo")
            ]
        );
        let r = refs(&f);
        assert!(r.contains(&("a.mk".into(), "make_include")));
        assert!(r.contains(&("#build".into(), "make_prerequisite")));
        assert!(r.contains(&("lib/x.c".into(), "make_prerequisite_file")));
        assert!(r.contains(&("./run.sh".into(), "literal_path_in_command")));
    }

    #[test]
    fn workflow_jobs_need_jobs_and_use_local_actions() {
        let text = "on: push\njobs:\n  a:\n    steps:\n      - run: ./x.sh\n\n  # gap\n  b:\n    needs: [a]\n    steps:\n      - uses: ./.github/actions/d\n      - uses: actions/checkout@v4\nenv:\n  K: v\n";
        let f = workflow(text);
        let decls: Vec<_> = f
            .decls
            .iter()
            .map(|d| (d.name.as_str(), &text[d.start..d.end]))
            .collect();
        assert_eq!(decls[0], ("a", "  a:\n    steps:\n      - run: ./x.sh"));
        assert!(
            decls[1].1.ends_with("actions/checkout@v4"),
            "{:?}",
            decls[1]
        );
        let r = refs(&f);
        assert!(r.contains(&("#a".into(), "workflow_needs")));
        assert!(r.contains(&(".github/actions/d".into(), "workflow_local_uses")));
        assert!(r.contains(&("./x.sh".into(), "literal_path_in_command")));
        assert!(!r.iter().any(|(l, _)| l.contains("checkout")));
    }
}
