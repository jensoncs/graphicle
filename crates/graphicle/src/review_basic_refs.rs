//! What a build, CI, configuration, markup, template, Lua or C source declares
//! and names, read from its bytes alone. `review_basic` resolves the names
//! against the snapshot; nothing here touches the filesystem or runs a tool.
pub(super) use super::basic_proto::Decl;

/// What a reference names.
pub(super) enum To {
    /// A path as written. `index` names the files a directory means, in order;
    /// a `required` path that matches nothing is reported.
    Path {
        literal: String,
        index: &'static [&'static str],
        required: bool,
        /// It may name a file basic review does not analyse (what a
        /// Dockerfile copies): resolved against every text source, and such
        /// a file becomes a node that is only a link target.
        any_file: bool,
    },
    /// A file named by a search path the snapshot cannot show (a template
    /// loader's roots, Lua's package.path): the one source
    /// in the repository whose path ends with one of these.
    Suffix(Vec<String>),
    /// A stylesheet import's candidate files, in order: the first found
    /// beside the file, then in each load path its build states, then from
    /// the repository root.
    Style(Vec<String>),
    /// A C `#include` name; `angle` for `<x.h>`.
    Include { name: String, angle: bool },
    /// A declaration of this file, or, when `wide`, the one of that name in
    /// the repository. A `namespace` (a Kubernetes reference's) rules out a
    /// declaration stated in another namespace.
    Decl {
        name: String,
        wide: bool,
        namespace: Option<String>,
    },
}

pub(super) struct Ref {
    pub to: To,
    pub start: usize,
    pub end: usize,
    pub relation: &'static str,
    pub evidence: &'static str,
}

#[derive(Default)]
pub(super) struct Facts {
    pub decls: Vec<Decl>,
    pub refs: Vec<Ref>,
    /// The namespace a Kubernetes object states, by declaration name.
    pub namespaces: std::collections::HashMap<String, String>,
}

impl Facts {
    pub(super) fn path(&mut self, literal: &str, start: usize, evidence: &'static str) {
        self.refs.push(Ref {
            to: To::Path {
                literal: literal.into(),
                index: &[],
                required: false,
                any_file: false,
            },
            start,
            end: start + literal.len(),
            relation: "potential_import",
            evidence,
        });
    }
    pub(super) fn decl_ref(
        &mut self,
        name: &str,
        wide: bool,
        start: usize,
        end: usize,
        relation: &'static str,
        evidence: &'static str,
    ) {
        self.refs.push(Ref {
            to: To::Decl {
                name: name.into(),
                wide,
                namespace: None,
            },
            start,
            end,
            relation,
            evidence,
        });
    }
}

/// Larger sources stay file level: no hand-written build or config file is.
const FACT_LIMIT: usize = 2 * 1024 * 1024;

pub(super) fn facts(lang: &str, path: &str, content: &str) -> Facts {
    if content.len() > FACT_LIMIT {
        return Facts::default();
    }
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match lang {
        "dockerfile" => super::basic_build::dockerfile(content),
        "make" => super::basic_build::makefile(content),
        "github_actions" => super::basic_build::workflow(content),
        "groovy" | "hcl" => {
            let mut f = Facts::default();
            shell_paths(&mut f, content, 0);
            f
        }
        "yaml" => super::basic_config::yaml(path, content),
        "json" => super::basic_config::json(content),
        "html" => super::basic_markup::html(content),
        "css" => super::basic_markup::css(ext, content),
        "template" => super::basic_markup::template(ext, content),
        "lua" => super::basic_lua::lua(content),
        "c" | "cpp" => includes(content),
        _ => Facts::default(),
    }
}

/// Words of command text that look like relative file paths: a script a
/// recipe runs, a file a step passes. They are candidates only where a source
/// of that path exists, so a word that is not a path costs nothing.
pub(super) fn shell_paths(facts: &mut Facts, text: &str, offset: usize) {
    let mut at = 0;
    for word in text.split(separator) {
        let start = at;
        at += word.len()
            + text[start + word.len()..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        let word = word.trim_end_matches(':');
        if path_like(word) {
            facts.path(word, offset + start, "literal_path_in_command");
        }
    }
}

/// What ends a word of command text.
pub(super) fn separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | '=' | ',' | '(' | ')' | ';' | '|' | '&' | '<' | '>' | '[' | ']'
        )
}

pub(super) fn path_like(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    word.len() < 512
        && !word.contains("://")
        && !word.contains(['$', '{', '}', '*', '?', '@', '\\', '%'])
        && !word.starts_with(['-', '/', '~'])
        && (word.contains('/') || name.contains('.'))
        && name.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && ext.starts_with(|c: char| c.is_ascii_alphabetic())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// `#include "x.h"` and `#include <x.h>`; `basic_links` resolves them.
fn includes(content: &str) -> Facts {
    let mut f = Facts::default();
    let mut at = 0;
    for line in content.split_inclusive('\n') {
        let directive = line
            .trim_start()
            .strip_prefix('#')
            .and_then(|r| r.trim_start().strip_prefix("include"))
            .map(str::trim_start);
        if let Some(rest) = directive
            && let Some((angle, body)) = (rest.strip_prefix('"').map(|b| (false, b)))
                .or_else(|| rest.strip_prefix('<').map(|b| (true, b)))
            && let Some((name, _)) = body.split_once(if angle { '>' } else { '"' })
            && !name.is_empty()
        {
            let start = at + line.find(name).unwrap_or(0);
            f.refs.push(Ref {
                to: To::Include {
                    name: name.into(),
                    angle,
                },
                start,
                end: start + name.len(),
                relation: "potential_import",
                evidence: "c_include_candidate",
            });
        }
        at += line.len();
    }
    f
}

/// `content` with its comments and string literals blanked to spaces, offsets
/// unchanged, so a keyword scan skips quoted and commented text.
pub(super) fn masked(content: &str, line_comment: &str, block: Option<(&str, &str)>) -> String {
    let bytes = content.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, part: &[u8]| {
        out.extend(part.iter().map(|b| if *b == b'\n' { b'\n' } else { b' ' }));
    };
    // Bytes, not `&str` slices: `i` steps through non-ASCII text one byte at a time.
    while i < bytes.len() {
        let rest = &bytes[i..];
        if let Some((open, close)) = block
            && rest.starts_with(open.as_bytes())
        {
            let end = rest[open.len()..]
                .windows(close.len())
                .position(|w| w == close.as_bytes())
                .map_or(rest.len(), |e| open.len() + e + close.len());
            blank(&mut out, &bytes[i..i + end]);
            i += end;
        } else if rest.starts_with(line_comment.as_bytes()) {
            let end = rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len());
            blank(&mut out, &bytes[i..i + end]);
            i += end;
        } else if matches!(bytes[i], b'"' | b'\'') {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote && bytes[j] != b'\n' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(bytes.len());
            out.push(b' ');
            blank(&mut out, &bytes[i + 1..end]);
            i = end;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Each blanked span starts and ends at an ASCII byte, so it covers whole
    // characters and the result stays UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(f: &Facts) -> Vec<&str> {
        f.refs
            .iter()
            .filter_map(|r| match &r.to {
                To::Path { literal, .. } => Some(literal.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn command_words_that_look_like_files_are_path_candidates() {
        let text = "./hack/gen.sh --out=bin/app && kubectl apply -f k8s/a.yaml $(X)/y.sh https://a.io/b.sh v1.2 go build ./...";
        let mut f = Facts::default();
        shell_paths(&mut f, text, 10);
        assert_eq!(
            paths(&f),
            ["./hack/gen.sh", "k8s/a.yaml"],
            "bin/app has no extension; v1.2 a version"
        );
        assert_eq!(
            &text[f.refs[1].start - 10..f.refs[1].end - 10],
            "k8s/a.yaml"
        );
    }

    #[test]
    fn includes_name_quoted_and_angle_headers_and_skip_comments() {
        let text = "#include <stdio.h>\n  #  include \"lib/util.h\"\n// #include \"no.h\"\n";
        let f = includes(text);
        let got: Vec<_> = f
            .refs
            .iter()
            .filter_map(|r| match &r.to {
                To::Include { name, angle } => Some((name.as_str(), *angle)),
                _ => None,
            })
            .collect();
        assert_eq!(got, [("stdio.h", true), ("lib/util.h", false)]);
        assert_eq!(&text[f.refs[1].start..f.refs[1].end], "lib/util.h");
    }

    #[test]
    fn masking_keeps_offsets_and_hides_strings_and_comments() {
        let text = "a = \"end\" -- end\nb = 'x' --[[ end ]] c";
        let m = masked(text, "--", Some(("--[[", "]]")));
        assert_eq!(m.len(), text.len());
        assert!(!m.contains("end"));
        assert!(m.ends_with(" c"));
    }

    #[test]
    fn masking_keeps_non_ascii_code_and_blanks_non_ascii_strings() {
        let text = "local café = 'naïve' -- été\nx = 1";
        let m = masked(text, "--", Some(("--[[", "]]")));
        assert_eq!(m.len(), text.len());
        assert!(m.starts_with("local café = "));
        assert!(m.ends_with("\nx = 1"));
        assert!(!m.contains('ï') && !m.contains("été"));
    }
}
