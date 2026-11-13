//! What an answer that lists a symbol's callers, references or neighbours
//! may be missing: the unresolved calls of the same code graph whose callee
//! text ends in the symbol's name, and the files the answer read whose type
//! information was unavailable. Without it a Go package that failed to load
//! answers "0 references" for a function `main.go` calls.
//!
//! A call is matched by the last segment of its callee text
//! ([`last_segment`]): `server.RunAthenaServer`, `self.run()` and
//! `crate::tax::pay` name `RunAthenaServer`, `run` and `pay`. It says a call
//! *could* name the symbol, never that it does: resolution is unchanged.
//! Lists are bounded ([`MAX_SITES`], [`MAX_FILES`]); the rest are counted.
//!
//! Read by `get_node` and `get_neighbors` ([`crate::graph_tools`]), the
//! identifier route of `context` ([`crate::identifier_route`]) and basic
//! review (`review_basic.rs`, over its own unresolved calls with [`by_name`]
//! and [`summary`]).
use crate::Result;
use crate::store::{rows, s};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The field an answer carries the report in.
pub const FIELD: &str = "possibly_incomplete";
/// Call sites listed per name; the rest are counted (`sites_omitted`).
pub const MAX_SITES: usize = 3;
/// Incomplete files listed per answer; the rest are counted.
pub const MAX_FILES: usize = 10;

/// The last name segment of a callee text: `pkg.Run` → `Run`,
/// `crate::tax::pay()` → `pay`, `obj->m` → `m`, `Map[int]` → `Map`.
pub fn last_segment(callee: &str) -> &str {
    let callee = callee.trim().trim_end_matches("()");
    let segment = callee.rsplit(['.', ':', '>', '#']).next().unwrap_or(callee);
    segment
        .split(['[', '<', '('])
        .next()
        .unwrap_or(segment)
        .trim()
}

/// Whether an unresolved call's reason says its file had no type
/// information (a Go package that did not load, or a Go analysis that did not
/// finish), so no call in that file was resolved.
pub fn file_incomplete(reason: &str) -> bool {
    reason == "type_information_unavailable" || reason.starts_with("go_")
}

/// Unresolved calls grouped by the last segment of their `callee`.
pub fn by_name<'a>(
    calls: impl IntoIterator<Item = &'a Value>,
) -> BTreeMap<&'a str, Vec<&'a Value>> {
    let mut grouped: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for call in calls {
        let segment = call["callee"].as_str().map_or("", last_segment);
        if !segment.is_empty() {
            grouped.entry(segment).or_default().push(call);
        }
    }
    grouped
}

/// Where an unresolved call was read: an episode's `source_file` and
/// `source_location` (`L31`), or basic review's `path` and `line`.
fn site(call: &Value) -> (String, Option<u64>) {
    let file = call["source_file"]
        .as_str()
        .or_else(|| call["path"].as_str())
        .unwrap_or("")
        .replace('\\', "/");
    let line = call["line"].as_u64().or_else(|| {
        call["source_location"]
            .as_str()?
            .strip_prefix('L')?
            .split(['-', ':'])
            .next()?
            .parse()
            .ok()
    });
    (file, line)
}

/// What `calls` (unresolved calls whose callee ends in `name`) say: how many
/// (a repeated call counts its `occurrences`), how many per reason, and the
/// first [`MAX_SITES`] sites by file and line.
pub fn summary(name: &str, calls: &[&Value]) -> Value {
    let mut reasons = BTreeMap::<&str, u64>::new();
    let mut count = 0;
    for call in calls {
        let n = call["occurrences"].as_u64().unwrap_or(1);
        count += n;
        *reasons
            .entry(call["reason"].as_str().unwrap_or("unknown"))
            .or_default() += n;
    }
    let mut sites: Vec<_> = calls.iter().map(|call| (site(call), *call)).collect();
    sites.sort_by(|(a, _), (b, _)| a.cmp(b));
    let listed: Vec<Value> = sites
        .iter()
        .take(MAX_SITES)
        .map(|((file, line), call)| {
            json!({"file":file,"line":line,"callee":call["callee"],"reason":call["reason"]})
        })
        .collect();
    json!({"name":name,"count":count,"reasons":reasons,
        "sites_omitted":sites.len() - listed.len(),"sites":listed})
}

/// What `episode`'s unresolved calls (`gf_unresolved_calls`) say about an
/// answer for `names` that read `files`: a [`summary`] per name some
/// unresolved call could name, and the files among `files` with no type
/// information ([`file_incomplete`]). `None` when there is nothing to say,
/// also once retention has dropped the episode's unresolved calls.
pub fn episode(
    db: &Connection,
    repository: &str,
    episode: &str,
    names: &[String],
    files: &BTreeSet<String>,
) -> Result<Option<Value>> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for name in names {
        let name = last_segment(name);
        if name.is_empty() || !seen.insert(name) {
            continue;
        }
        // instr() keeps the rows whose callee contains the name (exact and
        // case-sensitive, unlike LIKE); by_name keeps those it ends.
        let calls = rows(
            db,
            "SELECT call FROM gf_unresolved_calls WHERE repository_id=?1 AND episode=?2 AND instr(callee,?3)>0",
            params![repository, episode, name],
        )?
        .iter()
        .map(|row| Ok(serde_json::from_str(s(row, "call"))?))
        .collect::<Result<Vec<Value>>>()?;
        if let Some(matched) = by_name(&calls).get(name) {
            found.push(summary(name, matched));
        }
    }
    let mut incomplete = BTreeMap::<String, BTreeMap<String, i64>>::new();
    if !files.is_empty() {
        for row in rows(
            db,
            "SELECT json_extract(call,'$.source_file') AS file,json_extract(call,'$.reason') AS reason,count(*) AS calls FROM gf_unresolved_calls WHERE repository_id=?1 AND episode=?2 GROUP BY 1,2",
            params![repository, episode],
        )? {
            let (file, reason) = (s(&row, "file").replace('\\', "/"), s(&row, "reason"));
            if file_incomplete(reason) && files.contains(&file) {
                incomplete
                    .entry(file)
                    .or_default()
                    .insert(reason.to_string(), row["calls"].as_i64().unwrap_or(0));
            }
        }
    }
    if found.is_empty() && incomplete.is_empty() {
        return Ok(None);
    }
    let listed: Vec<Value> = incomplete
        .iter()
        .take(MAX_FILES)
        .map(|(file, reasons)| json!({"file":file,"reasons":reasons}))
        .collect();
    Ok(Some(json!({"unresolved_calls":found,
        "incomplete_files_omitted":incomplete.len() - listed.len(),"incomplete_files":listed})))
}

/// The report in one line for an answer's text, e.g. `Possibly incomplete:
/// 3 unresolved calls name RunAthenaServer (type information unavailable:
/// 3) at cmd/main.go:31, cmd/other.go:12, 1 more.`
pub fn line(report: &Value) -> Option<String> {
    let list = |key: &str| report[key].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let more = |key: &str, items: &mut Vec<String>| {
        if let Some(n) = report[key].as_u64().filter(|n| *n > 0) {
            items.push(format!("{n} more"));
        }
    };
    let mut parts = Vec::new();
    for found in list("unresolved_calls") {
        let count = found["count"].as_u64().unwrap_or(0);
        let reasons: Vec<String> = found["reasons"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(reason, n)| format!("{}: {n}", reason.replace('_', " ")))
            .collect();
        let mut sites: Vec<String> = found["sites"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|site| match site["line"].as_u64() {
                Some(line) => format!("{}:{line}", s(site, "file")),
                None => s(site, "file").to_string(),
            })
            .collect();
        if let Some(n) = found["sites_omitted"].as_u64().filter(|n| *n > 0) {
            sites.push(format!("{n} more"));
        }
        let (noun, verb) = if count == 1 {
            ("call", "names")
        } else {
            ("calls", "name")
        };
        parts.push(format!(
            "{count} unresolved {noun} {verb} {} ({}) at {}",
            s(found, "name"),
            reasons.join(", "),
            sites.join(", ")
        ));
    }
    let mut files: Vec<String> = list("incomplete_files")
        .iter()
        .map(|file| s(file, "file").to_string())
        .collect();
    more("incomplete_files_omitted", &mut files);
    if !files.is_empty() {
        parts.push(format!("no type information for {}", files.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("Possibly incomplete: {}.", parts.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_names_the_last_segment_of_its_callee() {
        for (callee, name) in [
            ("RunAthenaServer", "RunAthenaServer"),
            ("server.RunAthenaServer", "RunAthenaServer"),
            ("self.run()", "run"),
            ("crate::tax::pay", "pay"),
            ("obj->m", "m"),
            ("Map[int]", "Map"),
            ("x.Get().Do", "Do"),
            (".size()", "size"),
        ] {
            assert_eq!(last_segment(callee), name, "{callee}");
        }
    }

    #[test]
    fn a_summary_counts_by_reason_and_lists_the_first_sites() {
        let calls = [
            json!({"callee":"a.Run","reason":"type_information_unavailable","source_file":"b.go","source_location":"L9"}),
            json!({"callee":"Run","reason":"ambiguous","source_file":"a.go","source_location":"L3","occurrences":2}),
            json!({"callee":"Running","reason":"ambiguous","source_file":"a.go","source_location":"L4"}),
            json!({"callee":"c.Run","reason":"external_or_dynamic_call","path":"c.py","line":7}),
            json!({"callee":"d.Run","reason":"external_or_dynamic_call","path":"d.py","line":1}),
        ];
        let grouped = by_name(&calls);
        let found = summary("Run", &grouped["Run"]);
        assert_eq!(found["count"], 5);
        assert_eq!(
            found["reasons"],
            json!({"ambiguous":2,"external_or_dynamic_call":2,"type_information_unavailable":1})
        );
        assert_eq!(
            found["sites"][0],
            json!({"file":"a.go","line":3,"callee":"Run","reason":"ambiguous"})
        );
        assert_eq!(found["sites"].as_array().unwrap().len(), MAX_SITES);
        assert_eq!(found["sites_omitted"], 1);
        let report = json!({"unresolved_calls":[found],
            "incomplete_files":[{"file":"b.go"}],"incomplete_files_omitted":0});
        assert_eq!(
            line(&report).unwrap(),
            "Possibly incomplete: 5 unresolved calls name Run (ambiguous: 2, external or dynamic call: 2, type information unavailable: 1) at a.go:3, b.go:9, c.py:7, 1 more; no type information for b.go."
        );
        assert_eq!(
            line(&json!({"unresolved_calls":[],"incomplete_files":[]})),
            None
        );
        assert!(file_incomplete("go_analysis_timeout"));
        assert!(!file_incomplete("ambiguous"));
    }
}
