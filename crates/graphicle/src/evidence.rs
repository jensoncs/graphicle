//! Exact source windows shared by retrieval, reading and the answer controller.
//! Context expansion never changes an original or infers a timestamp.
use crate::{Error, Result, store::s};
use graphicle_memory::text;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn interval(item: &Value) -> Result<(usize, usize)> {
    let number = |key| item[key].as_u64().and_then(|n| usize::try_from(n).ok());
    match (number("byte_start"), number("byte_end")) {
        (Some(a), Some(b)) if a < b => Ok((a, b)),
        _ => Err(Error::bad("Evidence needs a nonempty source byte interval")),
    }
}

pub fn from_range(doc: &Value, start: usize, end: usize) -> Result<Value> {
    let content = s(doc, "content");
    let quote = content
        .get(start..end)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| Error::bad("Invalid evidence coordinates"))?;
    let mut result = json!({"id":doc["id"],"sha256":doc["sha256"],"title":doc["title"],
        "recorded_at":doc["recorded_at"],"text":quote,"byte_start":start,"byte_end":end,
        "total_bytes":content.len(),"next_byte":if end<content.len(){Some(end)}else{None}});
    // Keep a short original header beside later passages. It may contain a
    // conversation date, author or section title; it is not a typed event time.
    let header_end = content
        .find("\n\n")
        .unwrap_or_else(|| content.find('\n').unwrap_or(0));
    if header_end > 0 && header_end <= start {
        let header = text::prefix(&content[..header_end], 128, 512);
        if !header.trim().is_empty() {
            result["source_context"] = json!({"text":header,"byte_start":0,"byte_end":header.len(),"authority":"original_header"});
        }
    }
    Ok(result)
}

/// The interpretation contract accepts one window per source ID. Include all
/// cited ranges in that window, including a separate date/header citation.
/// Callers rebuild this from their original base when trimming optional claims;
/// otherwise dropping a claim could leave its large source range behind.
pub(crate) fn claim_context(
    base: &[Value],
    claims: &[Value],
    documents: &[Value],
    scope: &str,
) -> Result<Vec<Value>> {
    let docs: BTreeMap<_, _> = documents.iter().map(|d| (s(d, "id"), d)).collect();
    let mut ranges: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut order = Vec::new();
    let mut include = |id: &str, quote: &str, offset: Option<usize>| -> Result<()> {
        let doc = docs
            .get(id)
            .ok_or_else(|| Error::conflict("Prior claim source is unavailable"))?;
        let text = s(doc, "content");
        if quote.is_empty() {
            return Err(Error::bad("Prior claim has an empty source quote"));
        }
        let start = offset
            .filter(|a| text.get(*a..a.saturating_add(quote.len())) == Some(quote))
            .or_else(|| text.find(quote))
            .ok_or_else(|| Error::conflict("Prior claim source quote changed"))?;
        let end = start + quote.len();
        if let Some((a, b)) = ranges.get_mut(id) {
            *a = (*a).min(start);
            *b = (*b).max(end);
        } else {
            ranges.insert(id.to_string(), (start, end));
            order.push(id.to_string());
        }
        Ok(())
    };
    for source in base {
        include(
            s(source, "id"),
            s(source, "text"),
            source["byte_start"].as_u64().map(|v| v as usize),
        )?;
    }
    for claim in claims {
        for quote in claim["data"]["evidence"].as_array().into_iter().flatten() {
            include(s(quote, "id"), s(quote, "quote"), None)?;
        }
    }
    order
        .into_iter()
        .map(|id| {
            let (a, b) = ranges[&id];
            let mut window = from_range(docs[id.as_str()], a, b)?;
            window["scope"] = json!(scope);
            if !base.iter().any(|v| s(v, "id") == id) {
                window["context_role"] = json!("prior_claim_quote");
            }
            // These windows exist only to be read by the model.
            window["title"] = crate::prompt_context::title(&window["title"]);
            Ok(window)
        })
        .collect()
}

/// Expand to a nearby paragraph boundary without spending an unbounded source
/// window. Merge overlapping windows from the same original and preserve rank.
pub fn source_hits(documents: &[Value], hits: &[Value], expand: bool) -> Result<Vec<Value>> {
    let docs: BTreeMap<_, _> = documents.iter().map(|d| (s(d, "id"), d)).collect();
    let mut out: Vec<Value> = Vec::new();
    for hit in hits {
        let source = &hit["source"];
        let doc = docs
            .get(s(source, "id"))
            .ok_or_else(|| Error::bad("Source is unavailable"))?;
        if source["sha256"] != doc["sha256"] {
            return Err(Error::conflict("Source changed while packing evidence"));
        }
        let content = s(doc, "content");
        let (mut a, mut b) = interval(source)?;
        content
            .get(a..b)
            .ok_or_else(|| Error::bad("Invalid source window"))?;
        if expand {
            // A requested declaration may have an attached rationale comment.
            // Preserve that bounded comment paragraph, but do not enlarge the
            // range with surrounding code that packing could focus on instead.
            let whole = source["keep_whole"] == true;
            let left = content[..a].trim_end_matches('\n');
            let previous = left.rfind("\n\n").map_or(0, |n| n + 2);
            let before = content[previous..a].trim();
            let comment = !before.is_empty()
                && (before.lines().all(|line| {
                    let line = line.trim();
                    line.starts_with("//") || line.starts_with('#')
                }) || before
                    .strip_prefix("/*")
                    .and_then(|s| s.strip_suffix("*/"))
                    .is_some_and(|inner| !inner.contains("*/")));
            if (!whole || comment)
                && a - previous <= 1600
                && text::count(&content[previous..a]) <= 256
            {
                a = previous;
            }
            if !whole {
                let rest = &content[b..];
                let next = rest
                    .find("\n\n")
                    .map(|n| b + n + 2)
                    .unwrap_or(content.len());
                if next - b <= 1600 && text::count(&content[b..next]) <= 256 {
                    b = next;
                }
            }
        }
        let mut item = hit.clone();
        item["text"] = json!(&content[a..b]);
        item["source"]["byte_start"] = json!(a);
        item["source"]["byte_end"] = json!(b);
        if let Some(prior) = out.iter_mut().find(|p| {
            source["keep_whole"] != true
                && p["source"]["keep_whole"] != true
                && p["source"]["id"] == source["id"]
                && p["source"]["sha256"] == source["sha256"]
                && interval(&p["source"]).is_ok_and(|(x, y)| {
                    x <= b && a <= y && text::count(&content[x.min(a)..y.max(b)]) <= 2000
                })
        }) {
            let (x, y) = interval(&prior["source"])?;
            let (a, b) = (a.min(x), b.max(y));
            prior["text"] = json!(&content[a..b]);
            prior["source"]["byte_start"] = json!(a);
            prior["source"]["byte_end"] = json!(b);
        } else {
            out.push(item);
        }
    }
    for item in &mut out {
        let doc = docs[s(&item["source"], "id")];
        let (a, b) = interval(&item["source"])?;
        let content = s(doc, "content");
        let reference = from_range(doc, a, b)?;
        for key in ["source_context", "next_byte", "total_bytes"] {
            item["source"][key] = reference[key].clone();
        }
        item["source"]["line"] = json!(content[..a].matches('\n').count() + 1);
        item["source"]["end_line"] =
            json!(content[..b].trim_end_matches('\n').matches('\n').count() + 1);
        item["source"]["complete"] = json!(a == 0 && b == content.len());
        item["source"]["truncated"] = json!(b < content.len());
    }
    Ok(out)
}

/// Round-robin search arms before packing, so one expansion cannot consume the
/// entire budget. Every byte comes from a currently authorized snapshot.
pub fn pack(documents: &[Value], groups: &[Vec<Value>], max_tokens: usize) -> Result<Value> {
    let docs: BTreeMap<_, _> = documents.iter().map(|d| (s(d, "id"), d)).collect();
    let mut ordered: Vec<Value> = Vec::new();
    let max = groups.iter().map(Vec::len).max().unwrap_or(0).min(200);
    for rank in 0..max {
        for group in groups {
            let Some(item) = group.get(rank) else {
                continue;
            };
            let doc = docs
                .get(s(item, "id"))
                .ok_or_else(|| Error::bad("Evidence source is unavailable"))?;
            if item["sha256"] != doc["sha256"] {
                return Err(Error::conflict("Evidence source changed"));
            }
            let (a, b) = interval(item)?;
            let checked = from_range(doc, a, b)?;
            if checked["text"] != item["text"] {
                return Err(Error::bad("Evidence differs from original source"));
            }
            if let Some(prior) = ordered
                .iter_mut()
                .find(|p| p["id"] == item["id"] && interval(p).is_ok_and(|(x, y)| x <= b && a <= y))
            {
                let (x, y) = interval(prior)?;
                // Overlapping windows are one interval, never duplicated tokens.
                *prior = from_range(doc, a.min(x), b.max(y))?;
            } else {
                ordered.push(checked);
            }
        }
    }
    // A new window can bridge two earlier windows; collapse the transitive
    // overlap as well as the first match, retaining the earliest rank.
    let mut i = 0;
    while i < ordered.len() {
        let mut j = i + 1;
        while j < ordered.len() {
            let (a, b) = interval(&ordered[i])?;
            let (x, y) = interval(&ordered[j])?;
            if ordered[i]["id"] == ordered[j]["id"] && a <= y && x <= b {
                let doc = docs[s(&ordered[i], "id")];
                ordered[i] = from_range(doc, a.min(x), b.max(y))?;
                ordered.remove(j);
                j = i + 1;
            } else {
                j += 1;
            }
        }
        i += 1;
    }
    let candidates = ordered.len();
    let mut selected = Vec::new();
    let mut truncated = 0;
    for (i, item) in ordered.iter().enumerate().take(40) {
        let remaining = max_tokens.saturating_sub(text::count(&json!(selected).to_string()) + 8);
        let slots = (candidates - i).clamp(1, 3);
        let allowance = remaining / slots;
        let doc = docs[s(item, "id")];
        let (a, _) = interval(item)?;
        let mut take = s(item, "text").to_string();
        let mut value = item.clone();
        while text::count(&value.to_string()) > allowance {
            let overhead = text::count(&value.to_string()).saturating_sub(text::count(&take));
            take = text::prefix(&take, allowance.saturating_sub(overhead + 8), 64000);
            if take.trim().is_empty() {
                break;
            }
            value = from_range(doc, a, a + take.len())?;
        }
        if take.trim().is_empty() {
            continue;
        }
        if take != s(item, "text") {
            truncated += 1;
        }
        selected.push(value);
    }
    while text::count(&json!(selected).to_string()) > max_tokens {
        if selected.pop().is_none() {
            break;
        }
    }
    Ok(
        json!({"tokens":text::count(&json!(selected).to_string()),"max_tokens":max_tokens,
        "omitted_windows":candidates-selected.len(),"truncated_windows":truncated,"evidence":selected}),
    )
}

#[cfg(test)]
mod requested_range_tests {
    use super::*;

    #[test]
    fn requested_range_keeps_attached_comment_but_not_preceding_code() {
        for prefix in [
            "// Keep uncertainty.\n// Events repair it.\n",
            "/** Keep uncertainty. */\n",
        ] {
            let content = format!("previous_code();\n\n{prefix}fn keep() {{ return 1; }}");
            let start = content.find("fn keep").unwrap();
            let doc = json!({"id":"file:a.rs","sha256":"pin","content":content});
            let hit = json!({"source":{"id":"file:a.rs","sha256":"pin","byte_start":start,
                "byte_end":content.len(),"keep_whole":true}});
            let result = source_hits(&[doc.clone()], &[hit.clone()], true).unwrap();
            assert!(s(&result[0], "text").starts_with(prefix));
            assert!(!s(&result[0], "text").contains("previous_code"));
            let exact = source_hits(&[doc], &[hit], false).unwrap();
            assert_eq!(exact[0]["source"]["byte_start"], start);
        }
    }

    #[test]
    fn requested_range_is_not_expanded_or_merged_into_a_neighbor() {
        let content = "old context\n\nfn keep() { return 1; }\n\nnext context";
        let a = content.find("fn keep").unwrap();
        let b = content.find("\n\nnext").unwrap();
        let doc = json!({"id":"file:a.rs","path":"a.rs","sha256":"pinned","content":content});
        let requested = json!({"identity":"named","text":&content[a..b],"source":{
            "id":"file:a.rs","sha256":"pinned","byte_start":a,"byte_end":b,"keep_whole":true}});
        let neighbor = json!({"identity":"chunk","source":{
            "id":"file:a.rs","sha256":"pinned","byte_start":0,"byte_end":b}});
        let hits = source_hits(&[doc.clone()], &[requested.clone(), neighbor], true).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["text"], &content[a..b]);
        assert_eq!(hits[0]["source"]["byte_start"], a);
        assert_eq!(hits[0]["source"]["byte_end"], b);
        let mut stale = requested;
        stale["source"]["sha256"] = json!("old");
        assert!(source_hits(&[doc], &[stale], true).is_err());
    }
}
