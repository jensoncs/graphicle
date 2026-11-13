//! HTML, CSS and templates: the files a page loads, a stylesheet imports or a
//! template includes, and Go templates' `define`d blocks and their callers.
use super::basic_refs::{Decl, Facts, Ref, To};
use regex::Regex;
use std::sync::LazyLock;

static HTML: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b(?:src|href)\s*=\s*["']([^"'#?]+)"#).unwrap());
// Less import options come first: `@import (reference) "x"`.
static CSS_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"@(?:import|use|forward)\s+(?:\([^)]*\)\s*)?(?:url\(\s*)?["']?([^"')\s;]+)"#)
        .unwrap()
});
// Comments and inline script bodies; group 1 is a script's body.
static UNLOADED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<!--.*?(?:-->|\z)|<script\b[^>]*>(.*?)(?:</script|\z)").unwrap()
});
static CSS_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\burl\(\s*["']?([^"')\s#?]+)"#).unwrap());
static PARTIAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{>\s*([\w./-]+)").unwrap());
static JINJA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{%-?\s*(?:include|extends|import|from)\s+["']([^"']+)["']"#).unwrap()
});
static GO_ACTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"\{\{-?\s*(define|block|template|include|if|range|with|end)\b\s*(?:"([^"]*)")?[^}]*\}\}"#,
    )
    .unwrap()
});

/// A relative reference to another file of the site or project; a URL, an
/// absolute path, a data URI or a template expression is not one.
fn local(value: &str) -> bool {
    !value.is_empty() && !value.contains([':', '{', '}', '$']) && !value.starts_with(['/', '#'])
}

fn captures(f: &mut Facts, re: &Regex, content: &str, evidence: &'static str) {
    for c in re.captures_iter(content) {
        let m = c.get(1).unwrap();
        if local(m.as_str()) {
            f.path(m.as_str(), m.start(), evidence);
        }
    }
}

pub(super) fn html(content: &str) -> Facts {
    // A page is often a Jinja/Django or Go template: read its template
    // actions as a template's, so `{% extends %}` and `{{ template }}` link.
    let mut f = if JINJA.is_match(content) {
        template("jinja", content)
    } else if GO_ACTION.is_match(content) {
        template("gohtml", content)
    } else {
        Facts::default()
    };
    captures(&mut f, &HTML, &unloaded_blanked(content), "html_resource");
    f
}

/// `content` with its comments and inline script bodies blanked, offsets
/// unchanged: a `src` written there is no file the page loads.
fn unloaded_blanked(content: &str) -> String {
    let mut out = content.as_bytes().to_vec();
    for c in UNLOADED.captures_iter(content) {
        let span = c.get(1).unwrap_or_else(|| c.get(0).unwrap()).range();
        for b in &mut out[span] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    // Each span starts and ends at an ASCII byte, so it covers whole
    // characters and the result stays UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

/// The files an import names, as its stylesheet language resolves it: Less
/// adds `.less` to a bare name; Sass tries the name's extensions, its partial
/// `_name` and its `_index`; plain CSS loads the name as written.
fn import_candidates(ext: &str, name: &str) -> Vec<String> {
    let (dir, file) = name.rsplit_once('/').map_or(("", name), |(d, f)| (d, f));
    let partial = |file: &str| match dir {
        "" => format!("_{file}"),
        dir => format!("{dir}/_{file}"),
    };
    let sass = ["scss", "sass", "css"];
    match ext {
        "less" if !file.contains('.') => vec![format!("{name}.less")],
        "scss" | "sass" if !file.contains('.') => sass
            .iter()
            .map(|e| format!("{name}.{e}"))
            .chain(sass.iter().map(|e| partial(&format!("{file}.{e}"))))
            .chain(
                ["_index.scss", "index.scss", "_index.sass", "index.sass"]
                    .iter()
                    .map(|i| format!("{name}/{i}")),
            )
            .collect(),
        "scss" | "sass" if file.ends_with(".scss") || file.ends_with(".sass") => {
            vec![name.to_owned(), partial(file)]
        }
        _ => vec![name.to_owned()],
    }
}

pub(super) fn css(ext: &str, content: &str) -> Facts {
    let mut f = Facts::default();
    for c in CSS_IMPORT.captures_iter(content) {
        let m = c.get(1).unwrap();
        if local(m.as_str()) {
            f.refs.push(Ref {
                to: To::Style(import_candidates(ext, m.as_str())),
                start: m.start(),
                end: m.end(),
                relation: "potential_import",
                evidence: "css_import",
            });
        }
    }
    let imported: Vec<usize> = f.refs.iter().map(|r| r.start).collect();
    for c in CSS_URL.captures_iter(content) {
        let m = c.get(1).unwrap();
        if local(m.as_str()) && !imported.contains(&m.start()) {
            f.path(m.as_str(), m.start(), "css_url");
        }
    }
    f
}

pub(super) fn template(ext: &str, content: &str) -> Facts {
    let mut f = Facts::default();
    match ext {
        "mustache" | "hbs" | "handlebars" => {
            for c in PARTIAL.captures_iter(content) {
                let m = c.get(1).unwrap();
                f.refs.push(Ref {
                    to: To::Suffix(vec![format!("{}.{ext}", m.as_str()), m.as_str().into()]),
                    start: m.start(),
                    end: m.end(),
                    relation: "potential_import",
                    evidence: "template_partial",
                });
            }
        }
        "njk" | "j2" | "jinja" | "jinja2" => {
            for c in JINJA.captures_iter(content) {
                let m = c.get(1).unwrap();
                if local(m.as_str()) {
                    f.refs.push(Ref {
                        to: To::Suffix(vec![m.as_str().into()]),
                        start: m.start(),
                        end: m.end(),
                        relation: "potential_import",
                        evidence: "template_include",
                    });
                }
            }
        }
        _ => go_template(&mut f, content),
    }
    f
}

/// `define` and `block` are declarations, ended by their matching `end`;
/// `template` and `include` call the definition of that name, which a Go
/// program may parse from any of its template files.
fn go_template(f: &mut Facts, content: &str) {
    // Open actions: the index of the declaration they start, if any.
    let mut open: Vec<Option<usize>> = vec![];
    for c in GO_ACTION.captures_iter(content) {
        let action = c.get(0).unwrap();
        let name = c.get(2).map(|m| (m.as_str(), m.start(), m.end()));
        match &c[1] {
            "define" | "block" => {
                if c[1] == *"block"
                    && let Some((name, s, e)) = name
                {
                    f.decl_ref(name, true, s, e, "may_call", "go_template_call");
                }
                open.push(name.map(|(name, ..)| {
                    f.decls.push(Decl {
                        name: name.into(),
                        kind: "template_definition",
                        start: action.start(),
                        body_start: action.end(),
                        end: content.len(),
                    });
                    f.decls.len() - 1
                }));
            }
            "if" | "range" | "with" => open.push(None),
            "end" => {
                if let Some(Some(decl)) = open.pop() {
                    f.decls[decl].end = action.end();
                }
            }
            _ => {
                if let Some((name, s, e)) = name {
                    f.decl_ref(name, true, s, e, "may_call", "go_template_call");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(f: &Facts) -> Vec<String> {
        f.refs
            .iter()
            .map(|r| match &r.to {
                To::Path { literal, .. } => literal.clone(),
                To::Decl { name, .. } => format!("#{name}"),
                To::Suffix(n) | To::Style(n) => n.join("|"),
            })
            .collect()
    }

    #[test]
    fn pages_and_stylesheets_name_their_local_files() {
        let page = html(
            "<link href='css/a.css'><img SRC=\"img/x.png?v=1\"><a href=\"https://x.io/y.js\"><a href=\"#top\"><a href=\"/abs.css\"><a href=\"{{ url }}\">",
        );
        assert_eq!(targets(&page), ["css/a.css", "img/x.png"]);
        let sheet = css(
            "scss",
            "@import 'b.css';\n@use \"lib/vars\";\n@import url(\"c.css\");\na { background: url(i/bg.png) }\n.x { background: url(data:image/png;base64,AA) }\n",
        );
        let t = targets(&sheet);
        assert_eq!(t[0], "b.css");
        assert!(t[1].split('|').any(|c| c == "lib/_vars.scss"), "{t:?}");
        assert!(t.contains(&"c.css".to_owned()));
        assert_eq!(t.last().unwrap(), "i/bg.png");
        assert_eq!(t.iter().filter(|x| *x == "c.css").count(), 1);
    }

    #[test]
    fn commented_and_scripted_sources_are_not_what_a_page_loads() {
        let page = html(
            "<!--[if IE]><script src=\"ie.js\"></script><![endif]-->\n<!-- <a href=\"old.html\"> -->\n<script>var t = '<img src=\"x.png\">';</script>\n<SCRIPT type=\"module\" src=\"a.js\"></SCRIPT><link href=\"é.css\">",
        );
        assert_eq!(targets(&page), ["a.js", "é.css"]);
    }

    #[test]
    fn imports_name_the_files_their_language_resolves() {
        let less = targets(&css(
            "less",
            "@import \"theme\";\n@import (reference) \"lib/mix.less\";\n",
        ));
        assert_eq!(less, ["theme.less", "lib/mix.less"]);
        let scss = targets(&css(
            "scss",
            "@use \"lib/vars\";\n@import \"../t/theme.scss\";\n",
        ));
        assert_eq!(
            scss[0],
            "lib/vars.scss|lib/vars.sass|lib/vars.css|lib/_vars.scss|lib/_vars.sass|lib/_vars.css|lib/vars/_index.scss|lib/vars/index.scss|lib/vars/_index.sass|lib/vars/index.sass"
        );
        assert_eq!(scss[1], "../t/theme.scss|../t/_theme.scss");
        assert_eq!(targets(&css("css", "@import \"base\";\n")), ["base"]);
    }

    #[test]
    fn templates_include_partials_and_go_templates_declare_definitions() {
        assert_eq!(
            targets(&template("mustache", "{{> head}}{{name}}")),
            ["head.mustache|head"]
        );
        assert_eq!(
            targets(&template(
                "j2",
                "{% extends 'base.j2' %}{%- include \"p/n.j2\" %}{{ x }}"
            )),
            ["base.j2", "p/n.j2"]
        );
        let text = "{{/* c */}}{{ define \"a\" }}{{ range .X }}{{ template \"b\" . }}{{ end }}{{ end }}{{ define \"b\" }}x{{- end }}{{ template \"a\" }}";
        let f = template("tmpl", text);
        let decls: Vec<_> = f
            .decls
            .iter()
            .map(|d| (d.name.as_str(), &text[d.start..d.end]))
            .collect();
        assert_eq!(decls[0].0, "a");
        assert!(decls[0].1.ends_with("{{ end }}{{ end }}"), "{:?}", decls[0]);
        assert_eq!(decls[1], ("b", "{{ define \"b\" }}x{{- end }}"));
        assert_eq!(targets(&f), ["#b", "#a"]);
    }
}
