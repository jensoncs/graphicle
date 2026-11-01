use regex::Regex;
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

fn tokenizer() -> &'static CoreBPE {
    static TOKENIZER: OnceLock<CoreBPE> = OnceLock::new();
    TOKENIZER.get_or_init(|| tiktoken_rs::cl100k_base().expect("bundled reference tokenizer"))
}

pub fn count(text: &str) -> usize {
    tokenizer().encode_ordinary(text).len()
}

/// Exact count when it fits, otherwise stop at a stable tokenizer boundary.
/// Unlike counting arbitrary prefixes, these segment counts cannot shrink
/// when the rest of the text is appended.
pub fn count_within(text: &str, limit: usize) -> Option<usize> {
    let mut pieces = PrefixCounts::new(text, SEGMENT);
    for i in 1..pieces.starts.len() {
        if pieces.total(i) > limit {
            return None;
        }
    }
    let total = pieces.count(text.len());
    (total <= limit).then_some(total)
}

/// Token counts of prefixes of one text, each exactly `count(&text[..end])`.
///
/// `count` is additive over the pieces of cl100k's pre-tokenizer, and the
/// pattern has no look-behind, so a prefix can be split at any position `p`
/// that is a piece boundary of every longer prefix, provided the pieces before
/// `p` never look at text from `p` on. Three kinds of position qualify:
/// - an ASCII space after a non-whitespace character: the piece holding that
///   character (letters, digits, punctuation or a contraction) cannot take a
///   following space, and every earlier piece stopped at or before it;
/// - just after a line break followed by spaces or tabs and then a printable
///   ASCII character, once the prefix reaches that character: the piece
///   holding the break ends right after it (a punctuation run takes line
///   breaks only at its end; a whitespace run holding a break ends at its last
///   break unless it runs to the end of the input);
/// - an ASCII character that is not a letter after an ASCII letter, or not a
///   digit after an ASCII digit: a letter run, a contraction and a digit
///   group end before it, and no earlier piece looks past them, so long runs
///   without spaces or breaks (base64 in an SVG, a lock file's hashes) are
///   split too.
///
/// A prefix's count is then the counts of whole segments between kept
/// positions plus the count of its partial last segment. Segments are counted
/// once and reused, so each probe of `prefix` tokenizes a few dozen bytes
/// instead of the whole prefix. `tests/native_contracts.rs` checks the
/// equality against direct counts on adversarial and random texts.
struct PrefixCounts<'a> {
    text: &'a str,
    /// Kept split positions, increasing, `0` first.
    starts: Vec<usize>,
    /// `anchors[i]`: the split at `starts[i]` is exact for an end at it or past
    /// this position (the printable character after a line break's
    /// indentation; the position itself for a space split).
    anchors: Vec<usize>,
    /// `totals[i]` is `count(&text[..starts[i]])`, filled in order on demand.
    totals: Vec<usize>,
}

/// Fewest bytes between kept split positions: a probe tokenizes about this
/// much plus one word, and the whole text is tokenized once in segments of
/// about this size.
const SEGMENT: usize = 64;

impl<'a> PrefixCounts<'a> {
    fn new(text: &'a str, segment: usize) -> Self {
        let bytes = text.as_bytes();
        let (mut starts, mut anchors) = (vec![0], vec![0]);
        let mut previous: Option<char> = None;
        for (i, c) in text.char_indices() {
            let far = i >= starts[starts.len() - 1] + segment;
            if far && c == ' ' && previous.is_some_and(|p| !p.is_whitespace()) {
                starts.push(i);
                anchors.push(i);
            } else if far && c == '\n' {
                let mut j = i + 1;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                // Only ASCII bytes are compared, so every offset is a char boundary.
                if j < bytes.len() && (0x21..=0x7e).contains(&bytes[j]) {
                    starts.push(i + 1);
                    anchors.push(j);
                }
            } else if far
                && c.is_ascii()
                && previous.is_some_and(|p| {
                    (p.is_ascii_alphabetic() && !c.is_ascii_alphabetic())
                        || (p.is_ascii_digit() && !c.is_ascii_digit())
                })
            {
                starts.push(i);
                anchors.push(i);
            }
            previous = Some(c);
        }
        Self {
            text,
            starts,
            anchors,
            totals: vec![0],
        }
    }

    fn total(&mut self, index: usize) -> usize {
        while self.totals.len() <= index {
            let i = self.totals.len();
            let line = &self.text[self.starts[i - 1]..self.starts[i]];
            self.totals.push(self.totals[i - 1] + count(line));
        }
        self.totals[index]
    }

    fn count(&mut self, end: usize) -> usize {
        let mut index = self.starts.partition_point(|&start| start <= end) - 1;
        // An end inside the indentation after a break: the break and those
        // spaces are one piece, so split at the line before instead.
        if self.starts[index] < end && self.anchors[index] >= end {
            index -= 1;
        }
        let start = self.starts[index];
        let tail = if start < end {
            count(&self.text[start..end])
        } else {
            0
        };
        self.total(index) + tail
    }
}

/// Always return an exact UTF-8 prefix within both limits. No replacement characters.
pub fn prefix(text: &str, tokens: usize, characters: usize) -> String {
    if tokens == 0 || characters == 0 {
        return String::new();
    }
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .take(characters.saturating_add(1))
        .collect();
    let last = *boundaries.last().unwrap_or(&0);
    // Same probes and comparisons as counting each prefix from scratch; only
    // the cost of each count changes, so the result is identical.
    let mut counts = PrefixCounts::new(&text[..last], SEGMENT);
    if counts.count(last) <= tokens {
        return text[..last].to_owned();
    }
    let (mut low, mut high) = (0usize, boundaries.len() - 1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if counts.count(boundaries[mid]) <= tokens {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    // Token boundaries can merge after appending text. Correctness requires a
    // final count; maximizing every last token is not a semantic guarantee.
    while low > 0 && counts.count(boundaries[low]) > tokens {
        low -= 1;
    }
    text[..boundaries[low]].to_owned()
}

/// `count(&text[..end])` for each end, computed as `prefix` does (see
/// `PrefixCounts`) with split positions at least `segment` bytes apart;
/// exposed so the equivalence can be tested with every candidate split.
pub fn prefix_counts(text: &str, ends: &[usize], segment: usize) -> Vec<usize> {
    let mut counts = PrefixCounts::new(text, segment);
    ends.iter().map(|&end| counts.count(end)).collect()
}

pub fn words(value: &str) -> Vec<String> {
    static CAMEL: OnceLock<Regex> = OnceLock::new();
    static WORDS: OnceLock<Regex> = OnceLock::new();
    let split = CAMEL
        .get_or_init(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap())
        .replace_all(value, "$1 $2");
    WORDS
        .get_or_init(|| Regex::new(r"[\p{L}\p{N}]+").unwrap())
        .find_iter(&split.to_lowercase())
        .map(|m| m.as_str().to_owned())
        .collect()
}

pub fn sha256(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}

/// Unambiguous, nonnegative duration phrases, normalized to milliseconds.
/// Supports digits, English integers below 100, and explicit halves/quarters.
/// Unsupported quantities are ignored, never approximated; this is a matching signal, not a duration parser
/// for configuration or execution. Bare numbers have no inferred unit.
pub fn duration_literals(value: &str) -> std::collections::BTreeSet<u64> {
    const SMALL: &[&str] = &["zero", "one", "two", "three", "four", "five", "six", "seven",
        "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen",
        "sixteen", "seventeen", "eighteen", "nineteen"];
    const TENS: &[&str] = &["twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
    static LITERALS: OnceLock<Regex> = OnceLock::new();
    let rule = LITERALS.get_or_init(|| Regex::new(concat!(
        r"(?i)\b(half(?: of)? (?:a|an)|(?:a|one) quarter of (?:a|an)|three quarters of (?:a|an)|[0-9]+|(?:zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety)",
        r"(?:[ -](?:one|two|three|four|five|six|seven|eight|nine))?)[ \t-]+(milliseconds?|seconds?|minutes?|hours?|days?)\b"
    )).expect("exact duration literals"));
    rule.captures_iter(value).filter_map(|found| {
        let start = found.get(0)?.start();
        let before = value[..start].trim_end();
        // Do not read the tail of a decimal, fraction, signed quantity or a
        // larger spelled number as a separate positive integer.
        if before.ends_with(['.', ',', '/', '-', '+']) {
            return None;
        }
        let previous = before.split_whitespace().next_back().unwrap_or("").to_ascii_lowercase();
        if previous.parse::<u64>().is_ok() || SMALL.contains(&previous.as_str())
            || TENS.contains(&previous.as_str())
            || ["hundred", "thousand", "million", "billion", "and", "minus", "negative", "point"].contains(&previous.as_str()) {
            return None;
        }
        let amount = found[1].to_ascii_lowercase();
        let pieces: Vec<&str> = amount.split([' ', '-']).collect();
        let fraction = match amount.as_str() {
            "half a" | "half an" | "half of a" | "half of an" => Some((1u64, 2u64)),
            "a quarter of a" | "a quarter of an" | "one quarter of a" | "one quarter of an" => Some((1, 4)),
            "three quarters of a" | "three quarters of an" => Some((3, 4)),
            _ => None,
        };
        let integer = match pieces.as_slice() {
            [single] => single.parse::<u64>().ok()
                .or_else(|| SMALL.iter().position(|v| v == single).map(|i| i as u64))
                .or_else(|| TENS.iter().position(|v| v == single).map(|i| (i as u64 + 2) * 10)),
            [tens, ones] => TENS.iter().position(|v| v == tens).and_then(|t|
                SMALL[..10].iter().position(|v| v == ones).map(|o| (t as u64 + 2) * 10 + o as u64)),
            _ => None,
        };
        let (number, divisor) = fraction.or_else(|| integer.map(|n| (n, 1)))?;
        let unit = found[2].to_ascii_lowercase();
        let factor = match unit.trim_end_matches('s') {
            "millisecond" => 1, "second" => 1000, "minute" => 60_000,
            "hour" => 3_600_000, "day" => 86_400_000, _ => return None,
        };
        let millis = number.checked_mul(factor)?;
        (millis % divisor == 0).then_some(millis / divisor)
    }).collect()
}

/// Stable hashes independent of a model's JSON field ordering. Array order is
/// meaningful (procedure steps, citations) and is retained.
pub fn canonical(value: &serde_json::Value) -> String {
    fn ordered(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => {
                let sorted: std::collections::BTreeMap<_, _> = fields.iter().collect();
                let object = sorted
                    .into_iter()
                    .map(|(key, value)| (key.clone(), ordered(value)))
                    .collect();
                serde_json::Value::Object(object)
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(ordered).collect())
            }
            _ => value.clone(),
        }
    }
    serde_json::to_string(&ordered(value)).expect("JSON values are serializable")
}

#[cfg(test)]
mod bounded_count_tests {
    use super::*;
    #[test]
    fn equivalent_durations_match_without_inventing_units() {
        assert_eq!(duration_literals("wait sixty seconds"), duration_literals("lease expires after one minute"));
        assert_eq!(duration_literals("timeout after 120 seconds"), duration_literals("timeout after two minutes"));
        assert_eq!(duration_literals("wait forty-five seconds"), duration_literals("wait 45000 milliseconds"));
        for (phrase, equivalent) in [
            ("three quarters of a minute", "45 seconds"),
            ("half a minute", "30 seconds"),
            ("one quarter of an hour", "15 minutes"),
            ("a quarter of a second", "250 milliseconds"),
        ] { assert_eq!(duration_literals(phrase), duration_literals(equivalent), "{phrase}"); }
        for unsupported in ["half a millisecond", "two and three quarters of a minute", "negative half a minute", "three quarters of a month", "cap at 60", "wait 1.5 seconds", "wait -5 seconds", "wait 1/2 seconds",
            "wait one hundred twenty seconds", "wait one hundred and one minute", "wait negative two seconds",
            "wait 18446744073709551615 days"] {
            assert!(duration_literals(unsupported).is_empty(), "{unsupported}");
        }
    }
    #[test]
    fn bounded_counts_match_exact_counts_at_unicode_and_whitespace_boundaries() {
        let fragments = ["hello world", "\n    next", "can't I'LL", "你好🙂", "a12345!", "\r\n\t  ", "<|endoftext|>", "\"{}[]\\", "é\u{301}"];
        let mut seed = 13_u64;
        for size in [0, 1, 10, 64, 200] {
            for _ in 0..20 {
                let mut value = String::new();
                for _ in 0..size {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    value.push_str(fragments[(seed >> 32) as usize % fragments.len()]);
                }
                let expected = count(&value);
                for limit in [0, 1, expected.saturating_sub(1), expected, expected + 1] {
                    assert_eq!(count_within(&value, limit), (expected <= limit).then_some(expected), "limit={limit}, value={value:?}");
                }
            }
        }
    }
}
