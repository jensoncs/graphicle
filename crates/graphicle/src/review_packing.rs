//! Selection costs are independent of opaque identifiers. Wire token accounting
//! remains exact, and the original IDs/citations are never rewritten.
use graphicle_memory::text;
use regex::Regex;
use std::{borrow::Cow, sync::OnceLock};

fn encoding(json: &str) -> Cow<'_, str> {
    // Match whole JSON strings, not quoted substrings inside escaped source.
    static STRINGS: OnceLock<Regex> = OnceLock::new();
    let strings = STRINGS.get_or_init(|| Regex::new(r#""(?:[^"\\]|\\.)*""#).unwrap());
    let mut output = None::<String>;
    let mut copied = 0;
    let mut previous = (0, "");
    for token in strings.find_iter(json) {
        let literal = token.as_str();
        let body = &literal[1..literal.len() - 1];
        // Source bodies and these content-addressed fields are identical for
        // identical pinned inputs. Keep their exact cost instead of reserving
        // a worst-case hash allowance for them too.
        let pinned = matches!(
            previous.1,
            "\"text\""
                | "\"hunk\""
                | "\"symbol_id\""
                | "\"parent\""
                | "\"file_sha256\""
                | "\"text_sha256\""
                | "\"signature_revision\""
                | "\"implementation_revision\""
                | "\"commit\""
                | "\"snapshot\""
                | "\"policy_sha256\""
        ) && json[previous.0..token.start()].trim() == ":";
        if !pinned
            && matches!(body.len(), 32 | 40 | 64)
            && body.bytes().all(|b| b.is_ascii_hexdigit())
        {
            let out = output.get_or_insert_with(|| String::with_capacity(json.len()));
            out.push_str(&json[copied..token.start()]);
            out.push('"');
            // Alternating digits/letters prevent hex BPE compression. Charge
            // the same conservative cost for equal-width IDs and fingerprints,
            // including object keys and references in arrays.
            for _ in 0..body.len() / 2 {
                out.push_str("0a");
            }
            out.push('"');
            copied = token.end();
        }
        previous = (token.end(), literal);
    }
    match output {
        Some(mut out) => {
            out.push_str(&json[copied..]);
            Cow::Owned(out)
        }
        None => Cow::Borrowed(json),
    }
}

pub(super) fn count(json: &str) -> usize {
    text::count(&encoding(json))
}

pub(super) fn count_within(json: &str, limit: usize) -> Option<usize> {
    text::count_within(&encoding(json), limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opaque_strings_have_fixed_cost_without_rewriting_source() {
        let literal = "b".repeat(64);
        let mut expected = None;
        for seed in 0..128 {
            let id = format!("{seed:064x}");
            let value = json!({"id":id,"repository_id":format!("{seed:032x}"),
                "revision":format!("{seed:040x}"),"refs":[id],"definitions":{&id:{"ref":id}},
                "text":literal,"hunk":format!("+ const id = \"{literal}\";"),
                "name":"非ASCII context"})
            .to_string();
            let cost = count(&value);
            assert_eq!(*expected.get_or_insert(cost), cost);
            assert!(cost >= text::count(&value));
            let normalized: serde_json::Value = serde_json::from_str(&encoding(&value)).unwrap();
            assert_eq!(normalized["text"], literal);
            assert_eq!(normalized["hunk"], format!("+ const id = \"{literal}\";"));
            assert_eq!(count_within(&value, cost), Some(cost));
            assert_eq!(count_within(&value, cost - 1), None);
        }
        for value in [
            json!({"text":literal}),
            json!({"text":format!("prefix\"{literal}")}),
            json!({"hunk":literal}),
        ] {
            assert_eq!(encoding(&value.to_string()), value.to_string());
        }
        for field in [
            "symbol_id",
            "parent",
            "file_sha256",
            "text_sha256",
            "signature_revision",
            "implementation_revision",
            "commit",
            "snapshot",
            "policy_sha256",
        ] {
            let value = json!({field:literal}).to_string();
            assert_eq!(encoding(&value), value);
        }
    }
}
