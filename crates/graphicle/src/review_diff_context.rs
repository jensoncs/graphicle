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

    /// Lines of the entry the change touches.
    fn touched(&self, exact: &Exact, entry: &Value, path: &str) -> Vec<bool> {
        let ranges = self.changed(exact, s(entry, "side"), path, s(entry, "file_sha256"));
        line_spans(entry).iter().map(|&(a, b)| ranges.iter().any(|&(x, y)| x < b && a < y)).collect()
    }

    /// Replace a long changed declaration's text with its window (WINDOW_FROM).
    fn window(&self, exact: &Exact, entry: &mut Value, path: &str) {
        let spans = line_spans(entry);
        if spans.len() <= WINDOW_FROM {
            return;
        }
        let touched = self.touched(exact, entry, path);
        let calls = byte_pairs(&entry["_changed_calls"]);
        let mut keep = vec![false; spans.len()];
        keep[0] = true;
        for (i, &(a, b)) in spans.iter().enumerate() {
            if touched[i] {
                keep[i.saturating_sub(AROUND)..=(i + AROUND).min(spans.len() - 1)].fill(true);
            }
            if calls.iter().any(|&(x, y)| x < b && a < y) {
                keep[i] = true;
            }
        }
        if keep.iter().all(|k| *k) {
            return;
        }
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
        entry["text_window"] = json!({"lines":spans.len(),"shown_lines":keep.iter().filter(|k| **k).count(),
            "policy":"first_line_changed_lines_5_around_and_calls_to_changed_v1"});
        entry.as_object_mut().unwrap().remove("text");
    }

    /// Head seeds whose change is one inserted line, nothing removed or
    /// rewritten, and whose signature equals their base copy's: a caller of
    /// one learns little from the change (argus #111: main -> NewAppLogic).
    /// A rewritten line (return 1 -> return 2) keeps its callers required.
    fn minor_seeds(&self, exact: &Exact, entries: &[Value]) -> BTreeSet<String> {
        let seed = |e: &Value| e["reasons"].as_array().is_some_and(|rs| rs.iter().any(|r| r["reason"] == "change_seed"));
        let lines = |e: &Value| self.path(e).map_or(usize::MAX, |p| self.touched(exact, e, &p).iter().filter(|t| **t).count());
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
        for entry in report["entries"].as_array_mut().unwrap() {
            for reason in entry["reasons"].as_array_mut().into_iter().flatten() {
                let path = reason["path"].as_array().map_or(&[][..], Vec::as_slice);
                if reason["reason"] == "potential_consumer" && path.len() == 2 && minor.contains(path[0].as_str().unwrap_or("")) {
                    reason["callee_change"] = json!("one_line_same_signature");
                }
            }
            // A callee the changed lines call ranks with required code.
            let side = s(entry, "side").to_owned();
            let from_changed_lines = entry["reasons"].as_array().is_some_and(|rs| rs.iter().any(|r| {
                side == "head" && r["reason"] == "dependency" && r["path"].as_array().is_some_and(|p| p.len() == 2)
                    && r["steps"].as_array().is_some_and(|steps| steps.iter().any(|step| {
                        // calls only: a type the changed lines name is not what they run
                        if !matches!(step["relation"].as_str(), Some("calls" | "may_call" | "references_callable")) { return false }
                        let (Some(a), Some(b)) = (step["byte_start"].as_u64(), step["byte_end"].as_u64()) else { return false };
                        let path = s(step, "source_file");
                        let sha = self.files.get(&(side.clone(), path.to_owned())).map_or("", |f| f.sha.as_str());
                        self.changed(&exact, &side, path, sha).iter().any(|&(x, y)| (x as u64) < b && a < y as u64)
                    }))
            }));
            if from_changed_lines {
                entry["_provided_diff_callee"] = json!(true);
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
                self.window(&exact, entry, &path);
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
        let shown: Vec<_> = r["entries"][0]["excerpts"].as_array().unwrap().iter()
            .map(|e| (e["line"].as_u64().unwrap(), e["end_line"].as_u64().unwrap())).collect();
        assert_eq!(shown, vec![(1, 1), (95, 105)]);
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
        let mut r = report(vec![callee("gate", 12), callee("other", 30), typed, bound]);
        context_with(vec![("head", 10, 14)]).annotate(&mut r);
        assert_eq!(r["entries"][0]["_provided_diff_callee"], json!(true));
        assert!(r["entries"][1].get("_provided_diff_callee").is_none());
        assert!(r["entries"][2].get("_provided_diff_callee").is_none(), "argus #111: config types crowded out changed code");
        assert_eq!(r["entries"][3]["_provided_diff_callee"], json!(true));
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
