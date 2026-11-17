//! Caller-declared diff text can yield its packing budget, never prove review
//! coverage. Match complete spans against the authorized side's exact bytes.
use super::*;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub side: String,
    pub repository_id: String,
    pub file: String,
    pub file_sha256: String,
    pub line_start: usize,
    pub line_end: usize,
}

struct File {
    sha: String,
    ranges: Vec<(usize, usize)>,
}

/// A changed declaration longer than this carries a window, not its whole text:
/// its first line, its changed lines with AROUND lines either side, and every
/// line that calls a declaration the change seeds (argus #111: declarations of
/// 185-400 lines took the packet for a few changed lines each).
const WINDOW_FROM: usize = 100;
const AROUND: usize = 5;
/// A skipped run this short costs more as a marker than as its lines.
const FILL_GAP: usize = 3;
/// A caller's call site needs the logic around it, not only its line.
const CALLER_AROUND: usize = 5;
/// ponytail: lexical def-use, not data flow: at most this many calls per
/// declaration that receive a value assigned on a changed line. A real def-use
/// graph per language would replace it.
const FLOW_CALLS: usize = 8;
const NOT_NAMES: &[&str] = &["var", "val", "let", "const", "final", "auto", "if", "for", "while", "return", "else",
    "def", "fun", "func", "_", "err", "ok", "ctx", "self", "this"];

/// Names a line writes: those it assigns (`a, b :=`, `x =`, `val x: T =`,
/// `x +=`) and those it passes by address (`&x`: Go, Rust, C++; a reference
/// argument in Kotlin, Java or Python has no such mark), lexically.
fn written(line: &str) -> Vec<&str> {
    let mut out = assigned(line);
    let b = line.as_bytes();
    for i in (0..b.len()).filter(|&i| b[i] == b'&' && b.get(i + 1) != Some(&b'&') && (i == 0 || b[i - 1] != b'&')) {
        let name: &str = line[i + 1..].split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).next().unwrap_or("");
        let name = name.trim_end_matches('.');
        if !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit() || c.is_uppercase() && !name.contains('.'))
            && !NOT_NAMES.contains(&name) && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

fn assign_op(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    (0..b.len()).find(|&i| {
        b[i] == b'=' && !matches!(b.get(i + 1), Some(b'=' | b'>')) && !(i > 0 && matches!(b[i - 1], b'=' | b'!' | b'<' | b'>'))
    })
}

/// The last statement's left-hand side, when the line assigns.
fn lhs(line: &str) -> Option<&str> {
    line[..assign_op(line)?].rsplit(['{', ';', '(', ')']).next()
}

/// Whether the line replaces `name`'s value without reading it: not `x += 1`,
/// not `x = f(x)`, and not a pass by address.
fn replaces(line: &str, name: &str) -> bool {
    let Some(op) = assign_op(line) else { return false };
    let compound = op > 0 && ":+-*/%&|^".contains(line.as_bytes()[op - 1] as char) && line.as_bytes()[op - 1] != b':';
    assigned(line).contains(&name) && !compound && !has_word(&line[op + 1..], name)
}

/// Whether the assignment declares its names, so they end with the block:
/// Go's `:=`, a declaring keyword, or a type before the name (`String x =`).
/// A plain `=` writes a name declared further out (or, in Python, the
/// function's), which outlives the block.
fn declares(line: &str) -> bool {
    let Some(lhs) = lhs(line) else { return false };
    let words: Vec<&str> = lhs.trim_end_matches(|c| ":+-*/%&|^".contains(c)).split_whitespace().collect();
    lhs.ends_with(':')
        || words.iter().any(|w| matches!(*w, "var" | "val" | "let" | "const" | "final" | "auto"))
        || words.len() >= 2 && !words[words.len() - 2].ends_with(',') && !lhs.contains(':')
}

fn assigned(line: &str) -> Vec<&str> {
    let Some(lhs) = lhs(line) else { return vec![] };
    let lhs = lhs.trim_end_matches(|c| ":+-*/%&|^".contains(c));
    let lhs = lhs.split(':').next().unwrap_or(lhs);   // a type annotation: name: Type =
    lhs.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()) && !NOT_NAMES.contains(w))
        .collect()
}

fn has_word(line: &str, word: &str) -> bool {
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    line.match_indices(word).any(|(i, _)| !ident(line[..i].chars().last()) && !ident(line[i + word.len()..].chars().next()))
}

/// A definition and the guard after it, at most this many lines each.
const CONTEXT_SPAN: usize = 12;
/// Names per anchor whose last write is kept.
const CONTEXT_NAMES: usize = 4;

fn depth(line: &str) -> i64 {
    line.chars().map(|c| match c { '(' | '{' | '[' => 1, ')' | '}' | ']' => -1, _ => 0 }).sum()
}

fn opens_block(line: &str) -> bool {
    let t = line.trim();
    t.ends_with('{') || t.ends_with(':') && ["if ", "for ", "while ", "def ", "with ", "try", "elif ", "else", "class "].iter().any(|k| t.starts_with(k))
}

/// Lines (indexes) before each anchor that a reader needs to judge it,
/// lexically: the enclosing block headers, and for up to CONTEXT_NAMES names
/// it uses the nearest earlier line that writes the name, through its
/// closing bracket, with the `if` guard right after it (argus #111:
/// `githubSvc, err := github.New(` and its panic; `triggerOpts := &Opts{`).
fn context_lines(lines: &[&str], anchors: &[usize]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for &i in anchors {
        let mut level = indent(lines[i]);
        for k in (0..i).rev() {
            if level == 0 {
                break;
            }
            if !lines[k].trim().is_empty() && indent(lines[k]) < level && opens_block(lines[k]) {
                out.insert(k);
                level = indent(lines[k]);
            }
        }
        let names: Vec<&str> = lines[i].split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| w.len() >= 3 && !w.starts_with(|c: char| c.is_ascii_digit()) && !NOT_NAMES.contains(w)).collect();
        let mut found = 0;
        for name in names {
            if found == CONTEXT_NAMES {
                break;
            }
            let Some(k) = (0..i).rev().find(|&k| !is_comment(lines[k]) && written(lines[k]).contains(&name)) else { continue };
            found += 1;
            let mut end = k;
            let mut open = depth(lines[k]);
            while open > 0 && end + 1 < i && end - k < CONTEXT_SPAN {
                end += 1;
                open += depth(lines[end]);
            }
            out.extend(k..=end);
            let guard = end + 1;
            // Every name the line assigns, err and ok included: the guard tests those.
            let wrote: Vec<&str> = lhs(lines[k]).unwrap_or("").split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|w| !w.is_empty() && !matches!(*w, "var" | "val" | "let" | "const" | "final" | "auto")).collect();
            if guard < i && lines[guard].trim_start().starts_with("if ") && wrote.iter().any(|w| has_word(lines[guard], w)) {
                let base = indent(lines[guard]);
                let mut last = guard;
                while last + 1 < i && last - guard < CONTEXT_SPAN && (indent(lines[last + 1]) > base || lines[last + 1].trim().is_empty()) {
                    last += 1;
                }
                if last + 1 < i && indent(lines[last + 1]) == base && lines[last + 1].trim_start().starts_with('}') {
                    last += 1;
                }
                out.extend(guard..=last);
            }
        }
    }
    out
}

/// Shows skipped runs of FILL_GAP lines or fewer: a marker costs more.
fn fill(keep: &mut [bool]) {
    let mut gap = 0;
    for i in 0..keep.len() {
        if !keep[i] {
            gap += 1;
            continue;
        }
        if (1..=FILL_GAP).contains(&gap) {
            keep[i - gap..i].fill(true);
        }
        gap = 0;
    }
}

/// Replaces the entry's text with excerpts of the kept lines.
fn cut(entry: &mut Value, spans: &[(usize, usize)], keep: &[bool], policy: &str) {
    let text = entry["text"].as_str().unwrap().to_owned();
    let (start, first) = (spans[0].0, entry["line"].as_u64().unwrap_or(1) as usize);
    let mut excerpts = vec![];
    let mut i = 0;
    while i < spans.len() {
        if !keep[i] {
            i += 1;
            continue;
        }
        let j = (i..spans.len()).take_while(|&k| keep[k]).last().unwrap();
        let (a, b) = (spans[i].0, spans[j].1);
        excerpts.push(json!({"line":first + i,"end_line":first + j,"byte_start":a,"byte_end":b,"text":&text[a - start..b - start]}));
        i = j + 1;
    }
    entry["excerpts"] = json!(excerpts);
    entry["text_window"] = json!({"lines":spans.len(),"shown_lines":keep.iter().filter(|k| **k).count(),"policy":policy});
    entry.as_object_mut().unwrap().remove("text");
}

/// A long caller goes as its first line, CALLER_AROUND lines either side of
/// each call it is in the packet for, and those calls' context_lines (argus #111: two callers of 114 and
/// 116 lines took 11 KB for one call each).
fn window_caller(entry: &mut Value, path: &str) {
    let spans = line_spans(entry);
    if spans.len() <= WINDOW_FROM || entry.get("text").is_none() {
        return;
    }
    let sites: Vec<(u64, u64)> = entry["reasons"].as_array().into_iter().flatten()
        .filter(|r| r["reason"] == "potential_consumer")
        .flat_map(|r| r["steps"].as_array().cloned().unwrap_or_default())
        .filter(|st| s(st, "source_file") == path)
        .filter_map(|st| Some((st["byte_start"].as_u64()?, st["byte_end"].as_u64()?)))
        .collect();
    let mut keep = vec![false; spans.len()];
    keep[0] = true;
    let mut anchors = vec![];
    for (i, &(a, b)) in spans.iter().enumerate() {
        if sites.iter().any(|&(x, y)| x < b as u64 && (a as u64) < y) {
            keep[i.saturating_sub(CALLER_AROUND)..=(i + CALLER_AROUND).min(spans.len() - 1)].fill(true);
            anchors.push(i);
        }
    }
    if anchors.is_empty() {
        return;   // no call site inside: send it whole
    }
    let lines: Vec<&str> = entry["text"].as_str().unwrap_or("").split_inclusive('\n').collect();
    if lines.len() == spans.len() {
        for k in context_lines(&lines, &anchors) {
            keep[k] = true;
        }
    }
    fill(&mut keep);
    if !keep.iter().all(|k| *k) {
        cut(entry, &spans, &keep, "first_line_5_around_calls_and_their_definitions_v1");
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn is_comment(line: &str) -> bool {
    let line = line.trim_start();
    ["//", "#", "/*", "*", "--"].iter().any(|mark| line.starts_with(mark))
}

/// Where a call's argument list opens on the line: a `(` after a name or a
/// closing bracket, not Go's type assertion `.(`.
fn call_open(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    (1..b.len()).find(|&i| b[i] == b'(' && b[i - 1] != b'.' && (b[i - 1].is_ascii_alphanumeric() || matches!(b[i - 1], b'_' | b']' | b')' | b'>')))
}

/// Lines (indexes) after the first changed one that call with a value a
/// changed line wrote: the line naming it and the line opening the call it is
/// an argument of (argus #111: triggerInfo -> gateExecution, payload ->
/// parkForApproval). A name flows until indentation falls below the line
/// that wrote it or a later line writes it again; one line per callee.
fn flow_lines(entry: &Value, touched: &[bool]) -> Vec<usize> {
    let lines: Vec<&str> = entry["text"].as_str().unwrap_or("").split_inclusive('\n').collect();
    let Some(first) = touched.iter().position(|t| *t) else {
        return vec![];
    };
    let mut live: BTreeMap<&str, usize> = BTreeMap::new();
    let mut out = vec![];
    let mut callees = BTreeSet::new();
    for i in first..lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || is_comment(line) {
            continue;
        }
        live.retain(|_, depth| indent(line) >= *depth);
        let wrote = written(line);
        if touched[i] {
            let depth = if declares(line) { indent(line) } else { 0 };
            live.extend(wrote.into_iter().filter(|w| w.len() >= 3).map(|w| (w, depth)));
            continue;
        }
        let uses = |at: usize| live.keys().any(|n| !replaces(line, n) && line.get(at..).is_some_and(|rest| has_word(rest, n)));
        let mut depth = 0i64;
        let call = match call_open(line) {
            Some(open) if uses(open) => Some(i),
            Some(_) => None,
            None if uses(0) => (i.saturating_sub(8)..i).rev().find(|&k| {
                depth += lines[k].matches('(').count() as i64 - lines[k].matches(')').count() as i64;
                depth > 0
            }),
            None => None,
        };
        for w in wrote.iter().filter(|w| replaces(line, w)) {
            live.remove(w);
        }
        let Some(k) = call else { continue };
        let callee = lines[k][..call_open(lines[k]).unwrap_or(0)].trim().rsplit([' ', '\t', '=', ',']).next().unwrap_or("").to_owned();
        if out.len() / 2 < FLOW_CALLS && callees.insert(callee) {
            out.extend([k, i]);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Each line of an entry's text as an absolute [start, end) byte range.
fn line_spans(entry: &Value) -> Vec<(usize, usize)> {
    let (Some(text), Some(start)) = (entry["text"].as_str(), entry["byte_start"].as_u64()) else {
        return vec![];
    };
    let mut at = start as usize;
    text.split_inclusive('\n')
        .map(|line| {
            at += line.len();
            (at - line.len(), at)
        })
        .collect()
}

fn byte_pairs(value: &Value) -> Vec<(usize, usize)> {
    value.as_array().into_iter().flatten()
        .filter_map(|p| Some((p[0].as_u64()? as usize, p[1].as_u64()? as usize)))
        .collect()
}

/// The lines each diff changed, per (side, path), without the hunk context:
/// Graphicle's own diff (review.rs `_changed_base`/`_changed_head`).
type Exact = BTreeMap<(String, String), Vec<(usize, usize)>>;

fn exact_changes(report: &Value) -> Exact {
    let mut out = Exact::new();
    for diff in report["diffs"].as_array().into_iter().flatten() {
        for side in ["base", "head"] {
            if let Some(pairs) = diff.get(format!("_changed_{side}")) {
                out.entry((side.to_owned(), s(diff, "file").to_owned())).or_default().extend(byte_pairs(pairs));
            }
        }
    }
    out
}

pub(super) struct Context {
    files: BTreeMap<(String, String), File>,
    directories: BTreeMap<(String, String), String>,
}

impl Context {
    pub(super) fn new(spans: &[Span], base: &Input, head: &Input) -> Result<Self> {
        if spans.len() > 4096 {
            return Err(Error::bad("provided_diff accepts at most 4096 line ranges"));
        }
        let mut out = Self {
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
        };
        let mut lines = BTreeMap::<(String, String), Vec<usize>>::new();
        for span in spans {
            let input = match span.side.as_str() {
                "base" => base,
                "head" => head,
                _ => return Err(Error::bad("provided_diff side must be base or head")),
            };
            store::path(&span.file)?;
            store::hash(&span.file_sha256)?;
            let pin = input
                .pins
                .iter()
                .find(|p| p.repository_id == span.repository_id)
                .ok_or_else(|| Error::bad("provided_diff repository is not pinned on that side"))?;
            let path = format!("{}/{}", pin.directory, span.file);
            let source = input
                .sources
                .get(&path)
                .filter(|s| s.repository == span.repository_id && s.sha == span.file_sha256)
                .ok_or_else(|| {
                    Error::bad("provided_diff file/hash does not match the admitted side")
                })?;
            let content = source
                .content
                .text()
                .ok_or_else(|| Error::bad("provided_diff requires text source"))?;
            let key = (span.side.clone(), path);
            let offsets = lines.entry(key.clone()).or_insert_with(|| {
                let mut offsets = vec![0];
                for line in content.split_inclusive('\n') {
                    offsets.push(offsets.last().unwrap() + line.len());
                }
                offsets
            });
            if span.line_start == 0
                || span.line_start > span.line_end
                || span.line_end >= offsets.len()
            {
                return Err(Error::bad(
                    "provided_diff needs valid inclusive, one-based source lines",
                ));
            }
            out.files
                .entry(key)
                .or_insert_with(|| File {
                    sha: source.sha.clone(),
                    ranges: vec![],
                })
                .ranges
                .push((offsets[span.line_start - 1], offsets[span.line_end]));
            out.directories.insert(
                (span.side.clone(), span.repository_id.clone()),
                pin.directory.clone(),
            );
        }
        for file in out.files.values_mut() {
            file.ranges.sort_unstable();
            let mut merged: Vec<(usize, usize)> = vec![];
            for (start, end) in std::mem::take(&mut file.ranges) {
                if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            file.ranges = merged;
        }
        Ok(out)
    }

    pub(super) fn identity(&self) -> Option<String> {
        (!self.files.is_empty()).then(|| {
            crate::digest(&json!(
                self.files
                    .iter()
                    .map(|((side, path), file)| json!([side, path, file.sha, file.ranges]))
                    .collect::<Vec<_>>()
            ))
        })
    }

    fn covered(&self, side: &str, path: &str, sha: &str, start: u64, end: u64) -> bool {
        start < end
            && self
                .files
                .get(&(side.to_owned(), path.to_owned()))
                .is_some_and(|file| {
                    file.sha == sha
                        && file
                            .ranges
                            .iter()
                            .any(|(a, b)| *a as u64 <= start && end <= *b as u64)
                })
    }

    fn overlaps(&self, side: &str, path: &str, sha: &str, start: u64, end: u64) -> bool {
        start < end
            && self
                .files
                .get(&(side.to_owned(), path.to_owned()))
                .is_some_and(|file| {
                    file.sha == sha
                        && file
                            .ranges
                            .iter()
                            .any(|(a, b)| (*a as u64) < end && start < *b as u64)
                })
    }

    /// Bytes of [start, end) the provided ranges show (they are merged and sorted).
    fn shown(&self, side: &str, path: &str, sha: &str, start: u64, end: u64) -> u64 {
        self.files
            .get(&(side.to_owned(), path.to_owned()))
            .filter(|file| file.sha == sha)
            .map_or(0, |file| {
                file.ranges
                    .iter()
                    .map(|&(a, b)| (b as u64).min(end).saturating_sub((a as u64).max(start)))
                    .sum()
            })
    }

    fn ranges(&self, side: &str, path: &str, sha: &str) -> &[(usize, usize)] {
        self.files
            .get(&(side.to_owned(), path.to_owned()))
            .filter(|file| file.sha == sha)
            .map_or(&[], |file| &file.ranges)
    }

    fn path(&self, entry: &Value) -> Option<String> {
        let directory = self.directories.get(&(s(entry, "side").to_owned(), s(entry, "repository_id").to_owned()))?;
        Some(format!("{directory}/{}", s(entry, "file")))
    }

    /// The changed lines of a file: the diff's, else the provided ranges.
    fn changed<'a>(&'a self, exact: &'a Exact, side: &str, path: &str, sha: &str) -> &'a [(usize, usize)] {
        exact.get(&(side.to_owned(), path.to_owned())).map_or_else(|| self.ranges(side, path, sha), Vec::as_slice)
    }

    /// Lines of the entry the change touches; with `anchors`, a deletion (an
    /// empty range) touches the line it left.
    fn touched(&self, exact: &Exact, entry: &Value, path: &str, anchors: bool) -> Vec<bool> {
        let ranges = self.changed(exact, s(entry, "side"), path, s(entry, "file_sha256"));
        line_spans(entry).iter()
            .map(|&(a, b)| ranges.iter().any(|&(x, y)| (x < b && a < y) || (anchors && x == y && a <= x && x < b)))
            .collect()
    }

    /// Replace a long changed declaration's text with its window (WINDOW_FROM).
    /// Windows a large declaration; true when the window would show nothing
    /// the reviewers' hunks do not, beyond padding (the entry is then covered).
    fn window(&self, exact: &Exact, entry: &mut Value, path: &str, flows: &[usize]) -> bool {
        let spans = line_spans(entry);
        if spans.len() <= WINDOW_FROM {
            // Sent whole; unseen code only when a call to changed code or a
            // flow line is outside the hunks (odin #196: 31 one-line handler
            // changes outranked the queries the change relies on).
            let shown = self.ranges(s(entry, "side"), path, s(entry, "file_sha256"));
            let calls = byte_pairs(&entry["_changed_calls"]);
            let outside = |&(a, b): &(usize, usize)| !shown.iter().any(|&(x, y)| x <= a && b <= y);
            let unseen = spans.iter().enumerate().skip(1).any(|(i, &(a, b))| {
                (flows.contains(&i) || calls.iter().any(|&(x, y)| x < b && a < y)) && outside(&(a, b))
            });
            if !unseen {
                entry.as_object_mut().unwrap().remove("_provided_diff_mostly_unseen");
            }
            return false;
        }
        let touched = self.touched(exact, entry, path, true);
        let calls = byte_pairs(&entry["_changed_calls"]);
        let mut keep = vec![false; spans.len()];
        let mut needed = vec![false; spans.len()];   // calls and flow lines, not padding
        keep[0] = true;
        for (i, &(a, b)) in spans.iter().enumerate() {
            if touched[i] {
                keep[i.saturating_sub(AROUND)..=(i + AROUND).min(spans.len() - 1)].fill(true);
            }
            if calls.iter().any(|&(x, y)| x < b && a < y) {
                keep[i] = true;
                needed[i] = true;
            }
        }
        for &i in flows {
            keep[i] = true;
            needed[i] = true;
        }
        let text = entry["text"].as_str().unwrap_or("");
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        if lines.len() == spans.len() {
            let anchors: Vec<usize> = (0..spans.len()).filter(|&i| touched[i] || (needed[i] && !flows.contains(&i))).collect();
            for k in context_lines(&lines, &anchors) {
                keep[k] = true;
                needed[k] = true;
            }
        }
        fill(&mut keep);
        if keep.iter().all(|k| *k) {
            return false;
        }
        // Unseen only when a call or flow line is outside the reviewers'
        // hunks; padding around changed lines is the diff again (argus #111:
        // two values files, 4.6 KB of the block).
        let shown = self.ranges(s(entry, "side"), path, s(entry, "file_sha256"));
        let unseen = spans.iter().zip(&needed).skip(1)
            .any(|(&(a, b), k)| *k && !shown.iter().any(|&(x, y)| x <= a && b <= y));
        if !unseen {
            return true;
        }
        entry["_provided_diff_mostly_unseen"] = json!(true);
        cut(entry, &spans, &keep, "first_line_changed_lines_5_around_calls_to_changed_and_definitions_v1");
        false
    }

    /// Head seeds whose change is one inserted line, nothing removed or
    /// rewritten, and whose signature equals their base copy's: a caller of
    /// one learns little from the change (argus #111: main -> NewAppLogic).
    /// A rewritten line (return 1 -> return 2) keeps its callers required.
    fn minor_seeds(&self, exact: &Exact, entries: &[Value]) -> BTreeSet<String> {
        let seed = |e: &Value| e["reasons"].as_array().is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"));
        let lines = |e: &Value| self.path(e).map_or(usize::MAX, |p| self.touched(exact, e, &p, false).iter().filter(|t| **t).count());
        let base: BTreeMap<&str, &Value> = entries.iter().filter(|e| e["side"] == "base" && seed(e)).map(|e| (s(e, "symbol_id"), e)).collect();
        entries.iter()
            .filter(|e| e["side"] == "head" && seed(e) && lines(e) <= 1)
            .filter(|e| base.get(s(e, "symbol_id")).is_some_and(|b| b["signature_revision"] == e["signature_revision"] && lines(b) == 0))
            .map(|e| s(e, "symbol_id").to_owned())
            .collect()
    }

    pub(super) fn annotate(&self, report: &mut Value) {
        if self.files.is_empty() {
            return;
        }
        let mut entries = 0;
        let mut diffs = 0;
        let exact = exact_changes(report);
        let minor = self.minor_seeds(&exact, report["entries"].as_array().unwrap());
        // Calls on unchanged lines that receive a value a changed line assigned.
        let mut flows: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut flow_spans: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        for entry in report["entries"].as_array().unwrap() {
            let seed = entry["reasons"].as_array().is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"));
            let Some(path) = self.path(entry).filter(|_| seed && entry["side"] == "head") else { continue };
            let lines = flow_lines(entry, &self.touched(&exact, entry, &path, false));
            let spans = line_spans(entry);
            flow_spans.entry(path).or_default().extend(lines.iter().map(|&i| spans[i]));
            flows.insert(s(entry, "id").to_owned(), lines);
        }
        let generated_ids: BTreeSet<String> = report["entries"].as_array().unwrap().iter()
            .filter(|e| e["_generated"] == json!(true)).map(|e| s(e, "symbol_id").to_owned()).collect();
        for entry in report["entries"].as_array_mut().unwrap() {
            for reason in entry["reasons"].as_array_mut().into_iter().flatten() {
                let path = reason["path"].as_array().map_or(&[][..], Vec::as_slice);
                if reason["reason"] == "potential_consumer" && path.len() == 2 && minor.contains(path[0].as_str().unwrap_or("")) {
                    reason["callee_change"] = json!("one_line_same_signature");
                }
            }
            // A callee the changed lines call ranks with required code.
            let side = s(entry, "side").to_owned();
            // 1: called from a changed line; 2: only from a line receiving a
            // value a changed line wrote (ranked after).
            let generated = entry["_generated"] == json!(true);
            let reach = |r: &Value| -> u8 {
                let path = r["path"].as_array().map_or(&[][..], Vec::as_slice);
                // Or the source a generated callee is generated from: the
                // query behind a sqlc wrapper (argus #111: ORDER BY).
                let through_generated = path.len() == 3 && generated_ids.contains(path[1].as_str().unwrap_or(""))
                    && r["steps"].as_array().and_then(|st| st.last()).is_some_and(|st| st["relation"] == "generated_from");
                if side != "head" || generated || r["reason"] != "dependency" || !(path.len() == 2 || through_generated) {
                    return 0;
                }
                let mut best = 0;
                for step in r["steps"].as_array().into_iter().flatten() {
                    // calls only: a type the changed lines name is not what they run
                    if !matches!(step["relation"].as_str(), Some("calls" | "may_call" | "references_callable")) { continue }
                    let (Some(a), Some(b)) = (step["byte_start"].as_u64(), step["byte_end"].as_u64()) else { continue };
                    let path = s(step, "source_file");
                    let sha = self.files.get(&(side.clone(), path.to_owned())).map_or("", |f| f.sha.as_str());
                    let hit = |ranges: &[(usize, usize)]| ranges.iter().any(|&(x, y)| (x as u64) < b && a < y as u64);
                    if hit(self.changed(&exact, &side, path, sha)) {
                        return 1;
                    }
                    if hit(flow_spans.get(path).map_or(&[][..], Vec::as_slice)) {
                        best = 2;
                    }
                }
                best
            };
            match entry["reasons"].as_array().into_iter().flatten().map(reach).filter(|r| *r > 0).min() {
                Some(1) => entry["_provided_diff_callee"] = json!(true),
                Some(_) => entry["_provided_diff_flow_callee"] = json!(true),
                None => {}
            }
            // Linked only by type uses away from the changed lines: the
            // reviewers judge the change, not every type its code names.
            let off_change = |st: &Value| {
                let (Some(a), Some(b)) = (st["byte_start"].as_u64(), st["byte_end"].as_u64()) else { return true };
                let path = s(st, "source_file");
                let sha = self.files.get(&(side.clone(), path.to_owned())).map_or("", |f| f.sha.as_str());
                !self.changed(&exact, &side, path, sha).iter().any(|&(x, y)| (x as u64) < b && a < y as u64)
            };
            let reasons = entry["reasons"].as_array().map_or(&[][..], Vec::as_slice);
            let type_only = !reasons.is_empty() && reasons.iter().all(|r| {
                matches!(r["reason"].as_str(), Some("dependency" | "potential_consumer"))
                    && r["steps"].as_array().is_some_and(|st| !st.is_empty() && st.iter().all(|st| st["relation"] == "uses_type" && off_change(st)))
            });
            if type_only {
                entry["_type_only"] = json!(true);
            }
            let side = s(entry, "side");
            let Some(directory) = self
                .directories
                .get(&(side.to_owned(), s(entry, "repository_id").to_owned()))
            else {
                continue;
            };
            let path = format!("{directory}/{}", s(entry, "file"));
            if self.covered(
                side,
                &path,
                s(entry, "file_sha256"),
                entry["byte_start"].as_u64().unwrap_or(u64::MAX),
                entry["byte_end"].as_u64().unwrap_or(0),
            ) {
                entry["_provided_diff"] = json!(true);
                entries += 1;
            } else if self.overlaps(
                side,
                &path,
                s(entry, "file_sha256"),
                entry["byte_start"].as_u64().unwrap_or(u64::MAX),
                entry["byte_end"].as_u64().unwrap_or(0),
            ) {
                // A partial declaration stays intact and eligible. Packing
                // gives additional graph-linked source the first opportunity,
                // unless the diff leaves out most of it: then its unseen body
                // is context the reviewers have nowhere else (nexus #946).
                let (start, end) = (
                    entry["byte_start"].as_u64().unwrap_or(0),
                    entry["byte_end"].as_u64().unwrap_or(0),
                );
                let shown = self.shown(side, &path, s(entry, "file_sha256"), start, end);
                let mostly_unseen = end.saturating_sub(start) > 2 * shown;
                entry["_provided_diff_overlap"] = json!(true);
                if mostly_unseen {
                    entry["_provided_diff_mostly_unseen"] = json!(true);
                }
                let lines = flows.get(s(entry, "id")).cloned().unwrap_or_default();
                if self.window(&exact, entry, &path, &lines) {
                    let marks = entry.as_object_mut().unwrap();
                    marks.remove("_provided_diff_overlap");
                    marks.remove("_provided_diff_mostly_unseen");
                    marks.insert("_provided_diff".into(), json!(true));
                    entries += 1;
                }
            } else {
                window_caller(entry, &path);
            }
        }
        // A generated callee whose source ranks as a callee gives way to it:
        // the query is the contract the change relies on (odin #196).
        let replaced: BTreeSet<String> = report["entries"].as_array().unwrap().iter()
            .filter(|e| e["_provided_diff_callee"] == json!(true) || e["_provided_diff_flow_callee"] == json!(true))
            .flat_map(|e| e["reasons"].as_array().cloned().unwrap_or_default())
            .filter(|r| r["steps"].as_array().and_then(|st| st.last()).is_some_and(|st| st["relation"] == "generated_from"))
            .filter_map(|r| r["path"].as_array().filter(|p| p.len() == 3).map(|p| p[1].as_str().unwrap_or("").to_owned()))
            .filter(|id| generated_ids.contains(id))
            .collect();
        for entry in report["entries"].as_array_mut().unwrap() {
            if s(entry, "side") == "head" && replaced.contains(s(entry, "symbol_id")) {
                entry["_generated_with_source"] = json!(true);
            }
        }
        for diff in report["diffs"].as_array_mut().unwrap() {
            let covered = ["base", "head"].into_iter().all(|side| {
                let bytes = &diff[format!("{side}_bytes")];
                let (Some(start), Some(end)) = (bytes[0].as_u64(), bytes[1].as_u64()) else {
                    return false;
                };
                start == end
                    || self.covered(
                        side,
                        s(diff, "file"),
                        s(diff, &format!("{side}_sha256")),
                        start,
                        end,
                    )
            });
            if covered {
                diff["_provided_diff"] = json!(true);
                diffs += 1;
            }
        }
        // Keep only counters on the wire. The canonical span fingerprint is
        // already bound into review_reuse.context_sha256 and the receipt.
        report["coverage"]["provided_diff"] = json!({
            "ranges":self.files.values().map(|f|f.ranges.len()).sum::<usize>(),"entries":entries,"diffs":diffs});
        if entries + diffs > 0 {
            report["coverage"]["complete"] = json!(false);
        }
    }
}

pub(super) fn schema() -> Value {
    json!({"type":"array","maxItems":4096,"items":{"type":"object","additionalProperties":false,
        "required":["side","repository_id","file","file_sha256","line_start","line_end"],
        "properties":{"side":{"enum":["base","head"]},"repository_id":{"type":"string"},
            "file":{"type":"string"},"file_sha256":{"type":"string"},
            "line_start":{"type":"integer","minimum":1},"line_end":{"type":"integer","minimum":1}}}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_overlap_respects_byte_boundaries_hash_side_and_repository_path() {
        let context = Context {
            files: BTreeMap::from([(
                ("head".into(), "app/code.go".into()),
                File {
                    sha: "hash".into(),
                    ranges: vec![(10, 20), (30, 40)],
                },
            )]),
            directories: BTreeMap::new(),
        };
        assert!(context.overlaps("head", "app/code.go", "hash", 9, 11));
        assert!(context.overlaps("head", "app/code.go", "hash", 15, 35));
        assert!(!context.covered("head", "app/code.go", "hash", 15, 35));
        assert!(!context.overlaps("head", "app/code.go", "hash", 20, 30));
        assert!(!context.overlaps("head", "app/code.go", "hash", 10, 10));
        assert!(!context.overlaps("base", "app/code.go", "hash", 10, 20));
        assert!(!context.overlaps("head", "peer/code.go", "hash", 10, 20));
        assert!(!context.overlaps("head", "app/code.go", "other", 10, 20));
    }

    /// 200 numbered lines in app/code.go, one byte offset per line start.
    fn lines_file() -> (String, Vec<usize>) {
        let text: String = (1..=200).map(|n| format!("line {n:03}\n")).collect();
        let starts = (0..=200).map(|n| n * 9).collect();
        (text, starts)
    }

    fn context_with(ranges: Vec<(&str, usize, usize)>) -> Context {
        let (_, at) = lines_file();
        let mut files: BTreeMap<(String, String), File> = BTreeMap::new();
        for (side, first, last) in ranges {
            files.entry((side.into(), "app/code.go".into())).or_insert_with(|| File { sha: "hash".into(), ranges: vec![] })
                .ranges.push((at[first - 1], at[last]));
        }
        Context {
            files,
            directories: BTreeMap::from([(("head".into(), "repo".into()), "app".into()), (("base".into(), "repo".into()), "app".into())]),
        }
    }

    fn declaration(side: &str, id: &str, first: usize, last: usize, reasons: Value) -> Value {
        let (text, at) = lines_file();
        json!({"id":format!("{side}-{id}"),"symbol_id":id,"side":side,"repository_id":"repo","file":"code.go","file_sha256":"hash",
            "byte_start":at[first - 1],"byte_end":at[last],"line":first,"end_line":last,"signature_revision":"sig",
            "text":&text[at[first - 1]..at[last]],"reasons":reasons})
    }

    fn report(entries: Vec<Value>) -> Value {
        json!({"entries":entries,"diffs":[],"coverage":{}})
    }

    #[test]
    fn a_large_changed_declaration_carries_its_window_and_the_lines_calling_changed_code() {
        // argus #111: whole 185-400 line declarations took the packet for a few changed lines each.
        let (_, at) = lines_file();
        let mut seed = declaration("head", "big", 1, 200, json!([{"reason":"change_seed"}]));
        seed["_changed_calls"] = json!([[at[149] + 2, at[149] + 6]]);
        let small = declaration("head", "small", 1, 100, json!([{"reason":"change_seed"}]));
        let mut r = report(vec![seed, small]);
        context_with(vec![("head", 100, 100)]).annotate(&mut r);
        let big = &r["entries"][0];
        assert!(big.get("text").is_none());
        let shown: Vec<_> = big["excerpts"].as_array().unwrap().iter()
            .map(|e| (e["line"].as_u64().unwrap(), e["end_line"].as_u64().unwrap())).collect();
        assert_eq!(shown, vec![(1, 1), (95, 105), (150, 150)]);
        assert_eq!(big["excerpts"][1]["text"].as_str().unwrap().lines().next(), Some("line 095"));
        assert_eq!(big["excerpts"][1]["byte_start"], json!(at[94]));
        assert_eq!(big["excerpts"][1]["byte_end"], json!(at[105]));
        assert_eq!(big["text_window"]["lines"], json!(200));
        assert_eq!(big["text_window"]["shown_lines"], json!(13));
        assert!(r["entries"][1].get("text").is_some(), "100 lines stay whole");
        assert!(r["entries"][1].get("excerpts").is_none());
    }

    #[test]
    fn a_caller_of_a_one_line_change_with_an_unchanged_signature_is_marked_minor() {
        // argus #111: main was a required caller because NewAppLogic gained one line.
        let consumer = |id: &str, seed: &str| declaration("head", id, 190, 200,
            json!([{"reason":"potential_consumer","path":[seed, id],"steps":[{"relation":"calls"}]}]));
        let mut r = report(vec![
            declaration("head", "one", 10, 20, json!([{"reason":"change_seed"}])),
            declaration("base", "one", 10, 19, json!([{"reason":"change_seed"}])),
            declaration("head", "two", 30, 40, json!([{"reason":"change_seed"}])),
            declaration("base", "two", 30, 39, json!([{"reason":"change_seed"}])),
            consumer("main", "one"),
            consumer("handler", "two"),
        ]);
        r["entries"][2]["signature_revision"] = json!("new-sig");
        context_with(vec![("head", 15, 15), ("head", 35, 35)]).annotate(&mut r);
        assert_eq!(r["entries"][4]["reasons"][0]["callee_change"], json!("one_line_same_signature"));
        assert!(r["entries"][5]["reasons"][0].get("callee_change").is_none(), "its callee's signature changed");
        let mut r = report(vec![
            declaration("head", "one", 10, 20, json!([{"reason":"change_seed"}])),
            declaration("base", "one", 10, 20, json!([{"reason":"change_seed"}])),
            consumer("main", "one"),
        ]);
        context_with(vec![("head", 15, 15), ("base", 15, 15)]).annotate(&mut r);
        assert!(r["entries"][2]["reasons"][0].get("callee_change").is_none(), "a rewritten line can change what callers get");
    }

    #[test]
    fn changed_lines_come_from_the_diff_not_the_hunk_context_the_reviewers_read() {
        // argus #111: NewAppLogic's one inserted line arrived as a 7-line range
        // (git's 3 context lines), so its caller main stayed required.
        let (_, at) = lines_file();
        let consumer = declaration("head", "main", 190, 200,
            json!([{"reason":"potential_consumer","path":["one", "main"],"steps":[{"relation":"calls"}]}]));
        let mut r = report(vec![
            declaration("head", "one", 10, 20, json!([{"reason":"change_seed"}])),
            declaration("base", "one", 10, 19, json!([{"reason":"change_seed"}])),
            consumer,
        ]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[at[14], at[15]]],"_changed_base":[]}]);
        context_with(vec![("head", 12, 18), ("base", 12, 17)]).annotate(&mut r);
        assert_eq!(r["entries"][2]["reasons"][0]["callee_change"], json!("one_line_same_signature"));
    }

    #[test]
    fn a_window_centres_on_the_changed_lines_not_their_hunk_context() {
        let (_, at) = lines_file();
        let mut r = report(vec![declaration("head", "big", 1, 200, json!([{"reason":"change_seed"}]))]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[at[99], at[100]]],"_changed_base":[]}]);
        context_with(vec![("head", 97, 103)]).annotate(&mut r);
        // Its window would be the signature and the hunk with two lines of
        // padding: the diff again (argus #111: two values files, 4.6 KB). It
        // counts as covered and goes as history only.
        assert_eq!(r["entries"][0]["_provided_diff"], json!(true));
        assert!(r["entries"][0].get("_provided_diff_overlap").is_none());
        assert_eq!(r["coverage"]["provided_diff"]["entries"], json!(1));
    }

    /// A declaration of 150 lines whose line 20 assigns `info` and whose line
    /// 60 opens a call that receives it on line 62 (argus #111: triggerInfo
    /// set on a changed line, passed to gateExecution ten lines later).
    fn flowing() -> (String, Value) {
        let mut text: String = (1..=150).map(|n| format!("x{n:03} := 0\n")).collect();
        text = text.replace("x020 := 0\n", "info := build()\n").replace("x060 := 0\n", "gate(ctx,\n")
            .replace("x061 := 0\n", "  first,\n").replace("x062 := 0\n", "  &Opts{T: info})\n");
        let entry = json!({"id":"head-f","symbol_id":"f","side":"head","repository_id":"repo","file":"code.go","file_sha256":"hash",
            "byte_start":0,"byte_end":text.len(),"line":1,"end_line":150,"text":text,"reasons":[{"reason":"change_seed"}]});
        (text, entry)
    }

    fn line_range(text: &str, line: usize) -> (usize, usize) {
        let start: usize = text.split_inclusive('\n').take(line - 1).map(str::len).sum();
        (start, start + text.split_inclusive('\n').nth(line - 1).unwrap().len())
    }

    #[test]
    fn a_call_receiving_a_value_assigned_on_a_changed_line_is_kept_and_its_callee_marked() {
        let (text, entry) = flowing();
        let (a, b) = line_range(&text, 20);
        let (call, _) = line_range(&text, 60);
        let callee = json!({"id":"head-gate","side":"head","repository_id":"repo","file":"gate.go","reasons":[{"reason":"dependency","path":["f","gate"],
            "steps":[{"relation":"calls","source_file":"app/code.go","byte_start":call,"byte_end":call + 4}]}]});
        let mut wrapper = callee.clone();
        wrapper["id"] = json!("head-sqlc");
        wrapper["_generated"] = json!(true);
        let mut r = report(vec![entry, callee, wrapper]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[a, b]],"_changed_base":[]}]);
        let context = Context { files: BTreeMap::from([(("head".into(), "app/code.go".into()), File { sha: "hash".into(), ranges: vec![(a, b)] })]),
            directories: BTreeMap::from([(("head".into(), "repo".into()), "app".into())]) };
        context.annotate(&mut r);
        let shown: Vec<_> = r["entries"][0]["excerpts"].as_array().unwrap().iter()
            .map(|e| (e["line"].as_u64().unwrap(), e["end_line"].as_u64().unwrap())).collect();
        assert_eq!(shown, vec![(1, 1), (15, 25), (60, 62)], "60 and 62 joined across the one-line gap");
        assert_eq!(r["entries"][1]["_provided_diff_flow_callee"], json!(true), "reached by value flow, ranked after");
        assert!(r["entries"][2].get("_provided_diff_flow_callee").is_none(), "generated code is not a callee worth ranking");
    }

    fn flow_of(lines: &[&str], changed: &[usize]) -> Vec<usize> {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let touched: Vec<bool> = (0..lines.len()).map(|i| changed.contains(&i)).collect();
        flow_lines(&json!({"text":text}), &touched)
    }

    #[test]
    fn value_flow_ends_with_its_scope_or_a_reassignment_and_ignores_non_calls() {
        // argus #111: opts assigned in one changed closure matched every
        // sibling closure's own `opts, ok := payload.(*Opts)`.
        let lines = [
            "\tSanitize: func(raw any) {",
            "\t\topts := clean(raw)",          // changed
            "\t\tsend(ctx, opts)",             // a use inside its scope
            "\t},",
            "\tRun: func(payload any) {",
            "\t\topts, ok := payload.(*Opts)", // a sibling's own opts: scope ended
            "\t\trun(opts)",
            "\t},",
        ];
        assert_eq!(flow_of(&lines, &[1]), vec![2]);
        // argus #111: Submit reassigns its parameter inside an if and parks it
        // after the block: a plain assignment outlives the block it is in.
        let lines = [
            "\tif strip {",
            "\t\tpayload = sanitized", // changed
            "\t\tString label = name(payload)", // changed: a typed declaration
            "\t}",
            "\treturn park(ctx, payload)",
            "\tlog(label)",
        ];
        assert_eq!(flow_of(&lines, &[1, 2]), vec![4]);
        let lines = ["total := sum(xs)", "// log(total) in a comment", "y := total.(int)", "total = 0", "emit(total)"];
        assert!(flow_of(&lines, &[0]).is_empty(), "a comment, a type assertion and a reassignment are not uses");
        let lines = ["rate := 1", "rate += step", "rate = clamp(rate)", "emit(rate)"];
        assert_eq!(flow_of(&lines, &[0]), vec![2, 3], "updates that read the value carry it on");
        let lines = ["id := next()", "store(id)"];
        assert!(flow_of(&lines, &[0]).is_empty(), "names under three characters are too common to follow");
        let lines = ["record := build()", "log.Info(\"a\", record)", "log.Info(\"b\", record)", "save(record)"];
        assert_eq!(flow_of(&lines, &[0]), vec![1, 3], "one line per callee");
    }

    #[test]
    fn an_anchor_keeps_where_its_names_were_last_written_and_the_blocks_around_it() {
        // argus #111: githubSvc's guard sat 82 lines above the changed line
        // (a nil-client alarm the block could not rule out), and a caller's
        // triggerOpts literal and for header ended just outside its window.
        let lines = [
            "func New(cfg Config) *App {",
            "\tclient, err := github.New(cfg.Token,",
            "\t\tcfg.Org)",
            "\tif err != nil {",
            "\t\tpanic(err)",
            "\t}",
            "\tunrelated := 0",
            "\tfor _, sd := range subs {",
            "\t\topts := &Opts{",
            "\t\t\tTag: sd.Tag,",
            "\t\t}",
            "\t\tcount := 1",
            "\t\tcount = 2",
            "\t\trepo := newRepo(client, opts, count)",
        ];
        let kept: Vec<usize> = context_lines(&lines, &[13]).into_iter().collect();
        assert_eq!(kept, vec![0, 1, 2, 3, 4, 5, 7, 8, 9, 10, 12]);
    }

    #[test]
    fn a_name_passed_by_address_on_a_changed_line_is_written_there() {
        // argus #111: resolveChange(ctx, &triggerInfo, ...) fills triggerInfo,
        // which gateExecution receives fourteen lines later.
        assert_eq!(written("\tal.resolveChange(ctx, &triggerInfo, sub, \"\")\n"), vec!["triggerInfo"]);
        assert_eq!(written("if a && b { x = &y.z }\n"), vec!["x", "y.z"]);
        assert!(written("ok := a == b\n").is_empty(), "ok is noise");
        assert_eq!(written("val total: Int = 3\n"), vec!["total"]);
    }

    #[test]
    fn a_deletion_only_change_anchors_its_window_and_short_gaps_are_filled() {
        // A removed guard leaves no head line changed: the window keeps the
        // line after it. Gaps of three lines or fewer cost more as a marker.
        let (_, at) = lines_file();
        let mut big = declaration("head", "big", 1, 200, json!([{"reason":"change_seed"}]));
        big["_changed_calls"] = json!([[at[149], at[150]]]);   // line 150 calls changed code: the window has news
        let mut r = report(vec![big]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[at[79], at[79]], [at[91], at[92]]],"_changed_base":[[0, 9]]}]);
        context_with(vec![("head", 77, 83), ("head", 89, 95)]).annotate(&mut r);
        let shown: Vec<_> = r["entries"][0]["excerpts"].as_array().unwrap().iter()
            .map(|e| (e["line"].as_u64().unwrap(), e["end_line"].as_u64().unwrap())).collect();
        assert_eq!(shown, vec![(1, 1), (75, 97), (150, 150)], "75-85 and 87-97 joined across the one-line gap at 86");
    }

    #[test]
    fn a_short_change_is_unseen_code_only_when_a_call_to_changed_code_is_outside_the_hunks() {
        // odin #196: 31 handlers each gained one line; their unseen rest was
        // parse/validate/respond boilerplate and ranked above the SQL callees.
        let (_, at) = lines_file();
        let mut quiet = declaration("head", "quiet", 10, 40, json!([{"reason":"change_seed"}]));
        quiet["_changed_calls"] = json!([[at[20], at[20] + 4]]);   // the call on line 21, inside the hunk
        let mut loud = declaration("head", "loud", 50, 80, json!([{"reason":"change_seed"}]));
        loud["_changed_calls"] = json!([[at[74], at[74] + 4]]);    // line 75, outside the hunk
        let mut r = report(vec![quiet, loud]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[at[19], at[20]], [at[54], at[55]]],"_changed_base":[]}]);
        context_with(vec![("head", 17, 23), ("head", 52, 58)]).annotate(&mut r);
        assert!(r["entries"][0].get("_provided_diff_mostly_unseen").is_none(), "its unseen lines call nothing changed");
        assert_eq!(r["entries"][0]["_provided_diff_overlap"], json!(true), "still eligible as overlap");
        assert_eq!(r["entries"][1]["_provided_diff_mostly_unseen"], json!(true));
    }

    #[test]
    fn a_long_caller_goes_as_its_signature_and_the_logic_around_its_call() {
        let (_, at) = lines_file();
        let step = json!({"relation":"calls","source_file":"app/code.go","byte_start":at[149],"byte_end":at[149] + 4});
        let caller = declaration("head", "caller", 60, 190, json!([{"reason":"potential_consumer","path":["one","caller"],"steps":[step]}]));
        let mut r = report(vec![declaration("head", "one", 10, 20, json!([{"reason":"change_seed"}])), caller]);
        r["diffs"] = json!([{"file":"app/code.go","_changed_head":[[at[14], at[15]]],"_changed_base":[]}]);
        context_with(vec![("head", 12, 18)]).annotate(&mut r);
        let shown: Vec<_> = r["entries"][1]["excerpts"].as_array().unwrap().iter()
            .map(|e| (e["line"].as_u64().unwrap(), e["end_line"].as_u64().unwrap())).collect();
        assert_eq!(shown, vec![(60, 60), (145, 155)], "the call on line 150, 5 lines either side (its definitions: none here)");
    }

    #[test]
    fn a_callee_called_from_changed_lines_is_marked() {
        let (_, at) = lines_file();
        let callee = |id: &str, line: usize| declaration("head", id, 150, 160, json!([{"reason":"dependency","path":["seed", id],
            "steps":[{"relation":"calls","source_file":"app/code.go","byte_start":at[line - 1] + 1,"byte_end":at[line - 1] + 4}]}]));
        let mut typed = callee("Config", 12);
        typed["reasons"][0]["steps"][0]["relation"] = json!("uses_type");
        let mut bound = callee("Bind", 13);
        bound["reasons"][0]["steps"][0]["relation"] = json!("references_callable");   // argus #111: Bind: al.bindExecute
        // argus #111: the sqlc wrapper is generated; the query it is generated
        // from (ORDER BY) is what the caller's assumption rests on.
        let mut wrapper = callee("wrap", 12);
        wrapper["_generated"] = json!(true);
        let mut query = callee("query", 12);
        query["reasons"][0]["path"] = json!(["seed", "wrap", "query"]);
        query["reasons"][0]["steps"] = json!([wrapper["reasons"][0]["steps"][0], {"relation":"generated_from","source_file":"gen.go"}]);
        let mut beyond = query.clone();
        beyond["reasons"][0]["path"] = json!(["seed", "gate", "query"]);   // through a hand-written callee: no
        let mut r = report(vec![callee("gate", 12), callee("other", 30), typed, bound, wrapper, query, beyond]);
        context_with(vec![("head", 10, 14)]).annotate(&mut r);
        assert!(r["entries"][4].get("_provided_diff_callee").is_none());
        assert_eq!(r["entries"][5]["_provided_diff_callee"], json!(true));
        assert!(r["entries"][6].get("_provided_diff_callee").is_none());
        // odin #196: 15 sqlc wrappers took the block ahead of the queries they
        // run; the wrapper gives way to its source.
        assert_eq!(r["entries"][4]["_generated_with_source"], json!(true));
        assert!(r["entries"][0].get("_generated_with_source").is_none());
        assert_eq!(r["entries"][0]["_provided_diff_callee"], json!(true));
        assert!(r["entries"][1].get("_provided_diff_callee").is_none());
        assert!(r["entries"][2].get("_provided_diff_callee").is_none(), "argus #111: config types crowded out changed code");
        assert!(r["entries"][2].get("_type_only").is_none(), "a type a changed line uses stays");
        assert_eq!(r["entries"][1].get("_type_only"), None, "a call is not a type use");
        assert_eq!(r["entries"][3]["_provided_diff_callee"], json!(true));
    }

    #[test]
    fn a_declaration_linked_only_by_a_type_use_away_from_the_change_is_marked() {
        // argus #111: Constants, GrafanaConfig, DataSourceDefinitions refilled
        // 7 KB the windows freed, through fields of a changed struct.
        let (_, at) = lines_file();
        let typed = |id: &str, line: usize| declaration("head", id, 150, 160, json!([{"reason":"dependency","path":["seed", id],
            "steps":[{"relation":"uses_type","source_file":"app/code.go","byte_start":at[line - 1] + 1,"byte_end":at[line - 1] + 4}]}]));
        let mut r = report(vec![typed("far", 40), typed("near", 12)]);
        context_with(vec![("head", 10, 14)]).annotate(&mut r);
        assert_eq!(r["entries"][0]["_type_only"], json!(true));
        assert!(r["entries"][1].get("_type_only").is_none());
    }

    #[test]
    fn shown_bytes_count_only_what_the_provided_ranges_cover() {
        let context = Context {
            files: BTreeMap::from([(
                ("head".into(), "app/code.go".into()),
                File {
                    sha: "hash".into(),
                    ranges: vec![(10, 20), (30, 40)],
                },
            )]),
            directories: BTreeMap::new(),
        };
        // nexus #946: ProcessDelta 86-447 with hunks at 340-355 and 405-411.
        assert_eq!(context.shown("head", "app/code.go", "hash", 0, 100), 20);
        assert_eq!(context.shown("head", "app/code.go", "hash", 15, 35), 10);
        assert_eq!(context.shown("head", "app/code.go", "hash", 20, 30), 0);
        assert_eq!(context.shown("head", "app/code.go", "other", 0, 100), 0);
        assert_eq!(context.shown("base", "app/code.go", "hash", 0, 100), 0);
    }
}
