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

    pub(super) fn annotate(&self, report: &mut Value) {
        if self.files.is_empty() {
            return;
        }
        let mut entries = 0;
        let mut diffs = 0;
        for entry in report["entries"].as_array_mut().unwrap() {
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
