//! String keys: a changed line that writes or reads a dict key links the
//! declarations in the same directory that read or write it. Python passes
//! rules and series as dicts; a call graph does not see who reads the key a
//! change adds (siren #185, #187: `_kept` read the "drop_inf" a changed
//! `parse_rule` line wrote, and never reached the block).
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use super::{key, Analysis, Input, Symbol};

const NAME: &str = r#"["']([A-Za-z_][A-Za-z0-9_]*)["']"#;
static WRITES: LazyLock<[Regex; 2]> = LazyLock::new(|| {
    [
        Regex::new(&format!(r"\[\s*{NAME}\s*\]\s*(?:=[^=]|[-+*/|&]=)")).unwrap(),
        Regex::new(&format!(r"{NAME}\s*:")).unwrap(),
    ]
});
static READS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(&format!(r"\[\s*{NAME}\s*\]")).unwrap(),
        Regex::new(&format!(r"\.(?:get|pop)\(\s*{NAME}")).unwrap(),
        Regex::new(&format!(r"{NAME}\s+(?:not\s+)?in\s")).unwrap(),
    ]
});

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Key {
    pub name: String,
    pub write: bool,
    pub start: usize,
    pub end: usize,
}

/// The keys `text` writes and reads, byte offsets from `at`. A subscript that
/// is assigned is a write, not also a read.
pub(super) fn keys(text: &str, at: usize) -> Vec<Key> {
    let mut out = vec![];
    for (write, patterns) in [(true, &WRITES[..]), (false, &READS[..])] {
        for re in patterns {
            for c in re.captures_iter(text) {
                let m = c.get(1).unwrap();
                let (start, end) = (at + m.start(), at + m.end());
                if !out.iter().any(|k: &Key| k.start == start) {
                    out.push(Key { name: m.as_str().to_owned(), write, start, end });
                }
            }
        }
    }
    out
}

/// The byte ranges of `head`'s lines that `base` does not have, from `at`.
pub(super) fn changed_lines(head: &str, base: &str, at: usize) -> Vec<(usize, usize)> {
    let old: BTreeSet<&str> = base.lines().map(str::trim).collect();
    let mut out = vec![];
    let mut offset = at;
    for line in head.split_inclusive('\n') {
        if !line.trim().is_empty() && !old.contains(line.trim()) {
            out.push((offset, offset + line.len()));
        }
        offset += line.len();
    }
    out
}

pub(super) fn is_test(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.starts_with("test_") || name.contains("_test.") || name.contains(".test.") || name.contains(".spec.")
        || name == "conftest.py" || ["tests/", "test/", "__tests__/"].iter().any(|d| file.starts_with(d) || file.contains(&format!("/{d}")))
}

fn text<'a>(input: &'a Input, symbol: &Symbol) -> &'a str {
    input.sources.get(&symbol.file).and_then(|s| s.content.text()).and_then(|t| t.get(symbol.start..symbol.end)).unwrap_or("")
}

fn line_of(input: &Input, file: &str, at: usize) -> usize {
    input.sources.get(file).and_then(|s| s.content.text()).map_or(1, |t| t[..at.min(t.len())].matches('\n').count() + 1)
}

/// For each changed head declaration, the reasons that link it to the
/// declarations in its directory sharing a key its changed lines touch: a
/// reader of a key it writes is a potential consumer, a writer of a key it
/// reads is a dependency. Tests are neither seeds nor targets.
pub(super) fn reasons(base: &Input, a: &Analysis, head: &Input, b: &Analysis, seeds: &BTreeSet<String>) -> Vec<(String, Value)> {
    let old: BTreeMap<&str, &str> = a.symbols.values().map(|(_, s)| (s.key.as_str(), text(base, s))).collect();
    let dir = |file: &str| Path::new(file).parent().map(Path::to_path_buf);
    let mut out = vec![];
    for seed in seeds {
        let Some((_, symbol)) = b.symbols.get(seed) else { continue };
        if symbol.kind == "basic_file" || is_test(&symbol.file) {
            continue;
        }
        let body = text(head, symbol);
        let lines = changed_lines(body, old.get(symbol.key.as_str()).copied().unwrap_or(""), symbol.start);
        let touched: Vec<Key> = keys(body, symbol.start).into_iter()
            .filter(|k| lines.iter().any(|&(x, y)| x <= k.start && k.end <= y)).collect();
        if touched.is_empty() {
            continue;
        }
        for (id, (w, other)) in &b.symbols {
            if id == seed || other.kind == "basic_file" || is_test(&other.file) || dir(&other.file) != dir(&symbol.file)
                || (other.file == symbol.file && ((other.start <= symbol.start && symbol.end <= other.end) || (symbol.start <= other.start && other.end <= symbol.end))) {
                continue;
            }
            let theirs = keys(text(head, other), other.start);
            let consumer = touched.iter().filter(|k| k.write)
                .find_map(|k| theirs.iter().find(|t| !t.write && t.name == k.name).map(|t| (other, t, other, symbol)));
            let dependency = touched.iter().filter(|k| !k.write)
                .find_map(|k| theirs.iter().any(|t| t.write && t.name == k.name).then_some((symbol, k, symbol, other)));
            for (reason, link) in [("potential_consumer", consumer), ("dependency", dependency)] {
                let Some((at, k, reader, writer)) = link else { continue };
                out.push((id.clone(), json!({"reason":reason,"path":[seed,id],"steps":[{
                    "relation":"uses_key","source_file":at.file,"source_location":format!("L{}", line_of(head, &at.file, k.start)),
                    "byte_start":k.start,"byte_end":k.end,"spelling":k.name,"caller":key(w,&reader.key),"callee":key(w,&writer.key),
                    "discarded_error":false,"evidence":"string_key"}]})));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(text: &str) -> Vec<(String, bool)> {
        keys(text, 0).into_iter().map(|k| (k.name, k.write)).collect()
    }

    #[test]
    fn subscripts_gets_and_literals_are_reads_or_writes() {
        assert_eq!(named("x = rule[\"delay_s\"]\n"), vec![("delay_s".into(), false)]);
        assert_eq!(named("rule['delay_s'] = 3\n"), vec![("delay_s".into(), true)]);
        assert_eq!(named("rule['n'] += 1\n"), vec![("n".into(), true)]);
        assert_eq!(named("if rule[\"a\"] == 1: pass\n"), vec![("a".into(), false)]);
        assert_eq!(named("return {\"expr\": e}\n"), vec![("expr".into(), true)]);
        assert_eq!(named("v = s.get('values', [])\n"), vec![("values".into(), false)]);
        assert_eq!(named("if \"queries\" in rule:\n"), vec![("queries".into(), false)]);
        assert!(named("print(\"a: b\")\nx = \"not a key\"\n").is_empty());
    }

    #[test]
    fn test_files_are_known_by_name_or_directory() {
        for f in ["svc/test_agent.py", "pkg/a_test.go", "web/a.test.ts", "web/a.spec.js", "app/src/test/kotlin/A.kt", "tests/x.py", "svc/conftest.py"] {
            assert!(is_test(f), "{f}");
        }
        for f in ["svc/agent.py", "pkg/testing.go", "web/contest.ts", "latest/a.go"] {
            assert!(!is_test(f), "{f}");
        }
    }

    #[test]
    fn changed_lines_are_the_head_lines_the_base_lacks() {
        let base = "def f():\n    a = 1\n    return a\n";
        let head = "def f():\n    a = 1\n    b = 2\n    return a\n";
        assert_eq!(changed_lines(head, base, 100), vec![(119, 129)]);
    }
}
