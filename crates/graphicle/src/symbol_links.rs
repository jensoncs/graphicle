//! Reviewed memory and code symbols, read in both directions.
//!
//! A memory's links to code symbols are its `data.symbols` and the quotes
//! pinned after it was written (`gf_symbol_pins`; see [`crate::symbols`]).
//! This module reads them the other way round, without a table of its own:
//!
//! - **Back-links** ([`attach`]): for the code symbols a graph tool's
//!   `get_node` answer names, the current reviewed memories that link them
//!   and the documents the reader may read that mention them (`linked`). A
//!   connection to one project shows only that project's ([`confine`]).
//! - **The links step of a query** ([`lift`]), switched by `symbol_links`:
//!   after the fusion, each selected code window lifts the current reviewed
//!   memories linked to a symbol inside it (`code_to_memory`), and each
//!   selected memory lifts the current windows of the symbols it links
//!   (`memory_to_code`). One hop: a lifted item lifts nothing. At most
//!   [`MEMORY_LIMIT`] memories for focused questions (project-wide inventories
//!   use the request's bounded result allowance) and [`WINDOW_LIMIT`] windows, after the fused
//!   results, the code arm's anchors and the document links' windows, never
//!   one already there. A link counts only while the live-symbol table
//!   describes the bytes the reader reads (the file's sha256) and the symbol
//!   keeps the revision the link recorded, and only memories the query may
//!   serve are lifted (reviewed, current, allowed by its filters, within the
//!   recall bound).
//!
//! The `symbol_links` switch: left out, the links of the selected items are
//! counted (`withheld`) and nothing is lifted, since whether lifted items
//! help answers has not been measured; `true` follows them; `false` reads
//! none. A store whose memories carry no code links reads nothing here, so
//! its packets keep their evidence. A query only reads.
use crate::{
    Error, Result,
    query::Snapshot,
    store::{Principal, rows, s},
    symbols::{self, Live},
};
use graphicle_memory::graph::span;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The switch, the packet field and the facts of the links followed.
pub const ARM: &str = "symbol_links";
/// Linked memories for a focused query; the floor for project-wide inventories.
pub const MEMORY_LIMIT: usize = 4;
/// Bounded model-free candidate prefix, scored before the delivery cap.
pub const MEMORY_CANDIDATES: usize = 32;
/// Code windows one query lifts through memories, as the document links'
/// `LIFT_LIMIT` bounds theirs.
pub const WINDOW_LIMIT: usize = 6;
/// Memories and documents a back-link listing names per symbol.
pub const BACK_LINK_LIMIT: usize = 20;

/// Exact duration agreement can connect a numeric question to a constraint
/// expressed in another unit (seconds versus minutes). It only changes order;
/// lack of a literal match is never evidence that a note is irrelevant.
pub(crate) fn prioritize_literals(question: &str, memories: &mut [Value]) {
    let wanted = graphicle_memory::text::duration_literals(question);
    if wanted.is_empty() {
        return;
    }
    memories.sort_by_cached_key(|row| {
        let values = graphicle_memory::text::duration_literals(&format!(
            "{} {}",
            s(row, "title"),
            s(row, "text")
        ));
        std::cmp::Reverse(values.intersection(&wanted).count())
    });
}

/// An explicitly grounded note outside a validated declaration/dependency scope
/// is not evidence about that requested implementation. Unanchored or stale grounds cannot
/// justify exclusion. Values-file notes retain their declared file granularity.
pub(crate) fn outside_scope(
    row: &Value,
    scope: &BTreeSet<(String, String)>,
    documents: &HashMap<&str, &Value>,
) -> bool {
    let data = &row["memory"]["data"];
    let Some(dependencies) = data["dependencies"].as_object().filter(|v| !v.is_empty()) else {
        return false;
    };
    if dependencies.iter().any(|(path, sha)| {
        documents
            .get(format!("file:{path}").as_str())
            .is_none_or(|doc| sha != &doc["sha256"])
    }) {
        return false;
    }
    if let Some(links) = data["symbols"].as_array().filter(|v| !v.is_empty()) {
        if links.iter().any(|link| link["state"] != "resolved") {
            return false;
        }
        return links.iter().all(|link| {
            !scope.contains(&(
                s(&link["identity"], "relpath").to_string(),
                s(&link["identity"], "qualified_name").to_string(),
            ))
        });
    }
    dependencies
        .keys()
        .all(|path| !scope.iter().any(|(file, _)| file == path))
}

/// A model-free note focus backed by an already retrieved note which names
/// the requested topic or repeats a rationale question's premise. It is deliberately narrower than relevance
/// classification: unmatched paraphrases and unsupported multi-part questions stay broad.
/// Notes on the same symbol can supply additional reasons without repeating the
/// premise. A separately quantified policy with a disjoint topic does not gain
/// relevance just from that link; ungrounded notes cannot justify exclusion.
pub(crate) struct NoteFocus {
    anchors: BTreeSet<String>,
    symbols: BTreeSet<(String, String)>,
    topic: BTreeSet<String>,
    pub basis: &'static str,
}

fn note_symbols(row: &Value) -> BTreeSet<(String, String)> {
    row["memory"]["data"]["symbols"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| link["state"] == "resolved")
        .map(|link| {
            (
                s(&link["identity"], "relpath").into(),
                s(&link["identity"], "qualified_name").into(),
            )
        })
        .collect()
}

fn content_sequence(text: &str) -> Vec<String> {
    graphicle_memory::text::words(text)
        .iter()
        .flat_map(|word| {
            // Counts and negation belong to a premise even though ordinary note
            // ranking treats some of them as stop words.
            if matches!(word.as_str(), "one" | "two" | "not" | "no" | "never") {
                BTreeSet::from([word.clone()])
            } else {
                crate::notes_first::stems(word)
            }
        })
        .collect()
}

/// A separately quantified time policy is positive evidence of a distinct
/// topic only when its remaining words are disjoint from the supported premise.
/// Calendar units are used as text, never converted into an assumed duration.
fn policy_topic(text: &str) -> (bool, BTreeSet<String>) {
    static QUANTITY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
        r"(?i)\b(?:[0-9]+|zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety)",
        r"(?:[ -](?:one|two|three|four|five|six|seven|eight|nine))?[ \t-]+(?:milliseconds?|seconds?|minutes?|hours?|days?|weeks?|months?|years?)\b"
    )).expect("quantified note topic")
    });
    (
        QUANTITY.is_match(text),
        crate::notes_first::stems(&QUANTITY.replace_all(text, " ")),
    )
}

fn focus_topic(
    question: &str,
    anchors: &BTreeSet<String>,
    candidates: &[Value],
) -> BTreeSet<String> {
    let mut topic = policy_topic(question).1;
    for row in candidates
        .iter()
        .filter(|r| anchors.contains(s(r, "identity")))
    {
        topic.extend(policy_topic(&format!("{} {}", s(row, "title"), s(row, "text"))).1);
    }
    topic
}

fn premise_anchors(
    premise: &[String],
    durations: &BTreeSet<u64>,
    candidates: &[Value],
    phrase_len: usize,
) -> BTreeSet<String> {
    candidates
        .iter()
        .filter(|row| row["kind"] == "memory")
        .filter_map(|row| {
            let text = format!("{} {}", s(row, "title"), s(row, "text"));
            let supported = if !durations.is_empty() {
                !graphicle_memory::text::duration_literals(&text).is_disjoint(durations)
            } else {
                let terms = content_sequence(&text);
                premise
                    .windows(phrase_len)
                    .any(|phrase| terms.windows(phrase_len).any(|other| other == phrase))
            };
            supported.then(|| s(row, "identity").into())
        })
        .collect()
}

impl NoteFocus {
    /// A field explicitly named in a YAML file can anchor file-grounded notes
    /// without treating every other setting in that file as the same topic.
    fn field(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        static FIELD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(
            r"(?i)^\s*(?:which|what)\s+constraints?\s+governs?\s+`?(?P<field>[a-z_][a-z0-9_.]*)`?\s+in\s+`?(?P<path>[a-z0-9_./-]+\.ya?ml)`?\s*\??\s*$"
        ).expect("named YAML field topic")
        });
        let found = FIELD.captures(question)?;
        let topic = crate::notes_first::stems(&found["field"]);
        if topic.is_empty() {
            return None;
        }
        let anchors: BTreeSet<String> = candidates
            .iter()
            .filter(|row| row["kind"] == "memory")
            .filter(|row| {
                row["memory"]["data"]["dependencies"]
                    .as_object()
                    .is_some_and(|deps| {
                        deps.keys().any(|path| {
                            path == &found["path"]
                                || path.ends_with(&format!("/{}", &found["path"]))
                        })
                    })
            })
            .filter(|row| {
                topic.is_subset(&crate::notes_first::stems(&format!(
                    "{} {}",
                    s(row, "title"),
                    s(row, "text")
                )))
            })
            .map(|row| s(row, "identity").into())
            .collect();
        selected
            .iter()
            .any(|row| anchors.contains(s(row, "identity")))
            .then_some(Self {
                anchors,
                symbols: BTreeSet::new(),
                topic,
                basis: "named_field_topic",
            })
    }

    /// Factual expiry questions name their subject directly. A shared symbol
    /// may add qualifications about that subject, not an unrelated policy.
    fn expiry(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        static EXPIRY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(
                r"(?i)^\s*when\s+does\s+(?P<subject>[a-z][a-z -]*?)\s+expire\s*\??\s*$",
            )
            .expect("expiry fact subject")
        });
        let found = EXPIRY.captures(question)?;
        if graphicle_memory::text::words(&found["subject"])
            .iter()
            .any(|w| matches!(w.as_str(), "and" | "or" | "also" | "not" | "never"))
            || !crate::identifier_arm::explicit_names(&found["subject"])
                .names
                .is_empty()
        {
            return None;
        }
        let premise = content_sequence(&found["subject"]);
        if premise.len() < 3 {
            return None;
        }
        let anchors = premise_anchors(&premise, &BTreeSet::new(), candidates, 3);
        let symbols = candidates
            .iter()
            .filter(|row| anchors.contains(s(row, "identity")))
            .flat_map(note_symbols)
            .collect();
        let topic = crate::notes_first::stems(&found["subject"]);
        selected
            .iter()
            .any(|row| anchors.contains(s(row, "identity")))
            .then_some(Self {
                anchors,
                symbols,
                topic,
                basis: "factual_subject_phrase",
            })
    }

    /// A two-part rationale can narrow only after BOTH premises have retrieved
    /// support. Keep the union of their notes/symbols; never focus on one half.
    fn compound(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        static QUESTION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(
            r"(?i)^\s*why\s+does\s+`?(?P<target>[a-z_][a-z0-9_.:]*)`?\s+(?P<body>[^?;\n]+)\??\s*$"
        ).expect("two-premise rationale")
        });
        static AND: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\s+and\s+").unwrap());
        let found = QUESTION.captures(question)?;
        if crate::identifier_arm::explicit_names(&found["target"])
            .names
            .is_empty()
            || !crate::identifier_arm::explicit_names(&found["body"])
                .names
                .is_empty()
            || graphicle_memory::text::words(question).iter().any(|w| {
                matches!(
                    w.as_str(),
                    "or" | "also" | "not" | "no" | "never" | "without"
                )
            })
        {
            return None;
        }
        let clauses: Vec<_> = AND.split(&found["body"]).collect();
        if clauses.len() != 2 {
            return None;
        }
        let mut anchors = BTreeSet::new();
        for clause in clauses {
            let durations = graphicle_memory::text::duration_literals(clause);
            if durations.len() > 1 {
                return None;
            }
            // Two consecutive content words (e.g. "field order") support an
            // explicit subtopic after removing the shared declaration name.
            let supported = premise_anchors(&content_sequence(clause), &durations, candidates, 2);
            if !selected
                .iter()
                .any(|row| supported.contains(s(row, "identity")))
            {
                return None;
            }
            anchors.extend(supported);
        }
        let symbols = candidates
            .iter()
            .filter(|row| anchors.contains(s(row, "identity")))
            .flat_map(note_symbols)
            .collect();
        let topic = focus_topic(question, &anchors, candidates);
        Some(Self {
            anchors,
            symbols,
            topic,
            basis: "rationale_compound_agreement",
        })
    }

    /// A narrowly named retry-timing question can focus on an explicitly timed
    /// waiting policy. Bare inventories and compound topics keep broad recall.
    fn retry_timing(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        static TIMING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new(concat!(
            r"(?i)^\s*(?:which|what)\s+constraints\s+govern\s+",
            r"(?P<target>`?[a-z_][a-z0-9_.:]*`?)\s+(?:retry timing|backoff timing|waiting interval)\s*\??\s*$"
        )).expect("focused retry timing question")
        });
        let matched = TIMING.captures(question)?;
        if crate::identifier_arm::explicit_names(&matched["target"])
            .names
            .is_empty()
        {
            return None;
        }
        let waiting = crate::notes_first::stems("wait pause retry backoff delay timeout");
        let anchors: BTreeSet<String> = candidates
            .iter()
            .filter(|row| {
                let text = format!("{} {}", s(row, "title"), s(row, "text"));
                row["kind"] == "memory"
                    && !graphicle_memory::text::duration_literals(&text).is_empty()
                    && !crate::notes_first::stems(&text).is_disjoint(&waiting)
            })
            .map(|row| s(row, "identity").into())
            .collect();
        let symbols = candidates
            .iter()
            .filter(|row| anchors.contains(s(row, "identity")))
            .flat_map(note_symbols)
            .collect();
        let topic = focus_topic(question, &anchors, candidates);
        selected
            .iter()
            .any(|row| anchors.contains(s(row, "identity")))
            .then_some(Self {
                anchors,
                symbols,
                topic,
                basis: "retry_timing_policy",
            })
    }

    /// Exact named-topic requests can use the same grounded-note focus as
    /// rationale questions. Unlinked ranked evidence leaves prose scope broad.
    fn title(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        let lower = question.trim().trim_end_matches('?').to_ascii_lowercase();
        let topic = lower
            .strip_prefix("what is the ")
            .or_else(|| lower.strip_prefix("what is "))?;
        if topic
            .split_whitespace()
            .any(|w| matches!(w, "and" | "or" | "also"))
            || question.contains([';', '\n'])
        {
            return None;
        }
        let named = crate::identifier_arm::explicit_names(question);
        if !named.names.is_empty() || !named.paths.is_empty() {
            return None;
        }
        if selected.iter().any(|r| {
            r["memory"]["data"]["dependencies"]
                .as_object()
                .is_none_or(|d| d.is_empty())
                || r["memory"]["data"]["symbols"]
                    .as_array()
                    .is_some_and(|links| links.iter().any(|l| l["state"] != "resolved"))
        }) {
            return None;
        }
        let (title, qualifier) = topic
            .split_once(" for ")
            .map_or((topic, None), |(title, qualifier)| (title, Some(qualifier)));
        let terms = crate::notes_first::stems(title);
        let full_terms = crate::notes_first::stems(topic);
        if full_terms.len() < 2 {
            return None;
        }
        let matching: Vec<_> = candidates
            .iter()
            .filter(|r| {
                r["kind"] == "memory"
                    && (crate::notes_first::stems(s(r, "title")) == full_terms
                        || (terms.len() >= 2
                            && crate::notes_first::stems(s(r, "title")) == terms
                            && qualifier.is_some_and(|q| {
                                let terms = crate::notes_first::stems(q);
                                terms.len() >= 2
                                    && terms.is_subset(&crate::notes_first::stems(s(r, "text")))
                            })))
            })
            .collect();
        let anchors: BTreeSet<String> = matching.iter().map(|r| s(r, "identity").into()).collect();
        let symbols = matching.iter().flat_map(|r| note_symbols(r)).collect();
        let topic = focus_topic(question, &anchors, candidates);
        selected
            .iter()
            .any(|r| anchors.contains(s(r, "identity")))
            .then_some(Self {
                anchors,
                symbols,
                topic,
                basis: "exact_note_title",
            })
    }

    pub fn filter_reason(&self) -> &'static str {
        match self.basis {
            "exact_note_title" => "note_title_mismatch",
            "named_field_topic" | "factual_subject_phrase" => "note_topic_mismatch",
            _ => "rationale_premise_mismatch",
        }
    }

    pub fn new(question: &str, selected: &[Value], candidates: &[Value]) -> Option<Self> {
        if let Some(focus) = Self::title(question, selected, candidates) {
            return Some(focus);
        }
        if let Some(focus) = Self::field(question, selected, candidates) {
            return Some(focus);
        }
        if let Some(focus) = Self::expiry(question, selected, candidates) {
            return Some(focus);
        }
        if let Some(focus) = Self::retry_timing(question, selected, candidates) {
            return Some(focus);
        }
        if let Some(focus) = Self::compound(question, selected, candidates) {
            return Some(focus);
        }
        let words = graphicle_memory::text::words(question);
        if words.first().map(String::as_str) != Some("why")
            || words
                .iter()
                .any(|word| matches!(word.as_str(), "and" | "or" | "also"))
            || question.matches('?').count() > 1
            || question.contains([';', '\n'])
        {
            return None;
        }
        let durations = graphicle_memory::text::duration_literals(question);
        if durations.len() > 1 {
            return None;
        }
        let premise = content_sequence(question);
        // A single rationale still requires three consecutive content words.
        let anchors = premise_anchors(&premise, &durations, candidates, 3);
        let symbols = candidates
            .iter()
            .filter(|row| anchors.contains(s(row, "identity")))
            .flat_map(note_symbols)
            .collect();
        let topic = focus_topic(question, &anchors, candidates);
        selected
            .iter()
            .any(|row| anchors.contains(s(row, "identity")))
            .then_some(Self {
                anchors,
                symbols,
                topic,
                basis: if durations.is_empty() {
                    "rationale_premise_phrase"
                } else {
                    "rationale_duration_agreement"
                },
            })
    }

    pub fn keep(&self, row: &Value) -> bool {
        self.anchors.contains(s(row, "identity"))
            || (!note_symbols(row).is_disjoint(&self.symbols) && {
                let text = format!("{} {}", s(row, "title"), s(row, "text"));
                if self.basis == "factual_subject_phrase" {
                    !crate::notes_first::stems(&text).is_disjoint(&self.topic)
                } else {
                    let (quantified, topic) = policy_topic(&text);
                    !quantified || !topic.is_disjoint(&self.topic)
                }
            })
            || row["memory"]["data"]["dependencies"]
                .as_object()
                .is_none_or(|d| d.is_empty())
            || row["memory"]["data"]["symbols"]
                .as_array()
                .is_some_and(|links| links.iter().any(|link| link["state"] != "resolved"))
    }

    pub fn anchors(&self) -> usize {
        self.anchors.len()
    }
}

/// The `symbol_links` switch of a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Left out: the links of the selected items are counted, not followed.
    Counted,
    /// `true`: followed one hop.
    Follow,
    /// `false`: no link is read.
    Off,
}

impl Mode {
    pub fn parse(args: &Value) -> Result<Self> {
        match args.get(ARM) {
            None | Some(Value::Null) => Ok(Self::Counted),
            Some(Value::Bool(true)) => Ok(Self::Follow),
            Some(Value::Bool(false)) => Ok(Self::Off),
            Some(_) => Err(Error::bad("symbol_links must be true or false")),
        }
    }
    /// Whether links are followed (lifted into the packet).
    pub fn enabled(self) -> bool {
        self == Self::Follow
    }
}

fn resolved(entries: &[Value]) -> impl Iterator<Item = &Value> {
    entries
        .iter()
        .filter(|entry| entry["state"] == "resolved" && entry["stable_id"].is_string())
}

/// The resolved links of each memory that has code links: its own
/// (`data.symbols`) and its pinned quotes', one per `stable_id` (the first
/// recorded).
fn links_of<'m>(
    db: &Connection,
    repository: &str,
    memories: impl Iterator<Item = &'m Value>,
) -> Result<HashMap<String, Vec<Value>>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for memory in memories {
        let entries = memory["data"]["symbols"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !entries.is_empty() {
            out.entry(s(memory, "id").to_string())
                .or_default()
                .extend(resolved(entries).cloned());
        }
    }
    if out.is_empty() {
        return Ok(out);
    }
    let ids: BTreeSet<&String> = out.keys().collect();
    for row in rows(
        db,
        "SELECT memory_id,symbols FROM gf_symbol_pins WHERE repository_id=?1 AND state='pinned' \
         AND memory_id IN (SELECT value FROM json_each(?2)) ORDER BY memory_id,relpath,byte_start",
        params![repository, json!(ids).to_string()],
    )? {
        let pinned: Vec<Value> = serde_json::from_str(s(&row, "symbols")).unwrap_or_default();
        if let Some(links) = out.get_mut(s(&row, "memory_id")) {
            links.extend(resolved(&pinned).cloned());
        }
    }
    for links in out.values_mut() {
        let mut seen = BTreeSet::new();
        links.retain(|link| seen.insert(s(link, "stable_id").to_string()));
    }
    out.retain(|_, links| !links.is_empty());
    Ok(out)
}

/// Use every ranked note's live symbol or file dependencies as prose anchors,
/// or none. Unlinked notes, old revisions and missing source bytes keep the
/// window-based fallback rather than inferring a scope from partial evidence.
fn ranked_memory_scope(
    db: &Connection,
    repository: &str,
    question: &str,
    selected: &[Value],
    links: &HashMap<String, Vec<Value>>,
    documents: &HashMap<&str, &Value>,
) -> Option<BTreeSet<(String, String)>> {
    if selected.is_empty() {
        return None;
    }
    // One recalled topic must not hide an additional topic requested in prose.
    // Require complete lexical coverage before inferring any scope; unmatched
    // wording (including paraphrases) retains the broad expansion. Coverage is
    // a conservative prerequisite, not a semantic relevance score.
    let required = crate::notes_first::stems(question);
    let covered: BTreeSet<_> = selected
        .iter()
        .flat_map(|row| {
            crate::notes_first::stems(&format!("{} {}", s(row, "title"), s(row, "text")))
        })
        .collect();
    if required.is_empty() || !required.is_subset(&covered) {
        return None;
    }
    let mut wanted = BTreeMap::new();
    let mut seeds = BTreeSet::new();
    for row in selected {
        if row["memory"]["data"]["symbols"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|link| link["state"] != "resolved")
        {
            return None;
        }
        let Some(symbol_links) = links.get(s(&row["memory"], "id")) else {
            // A deliberately file-grounded note has the file's scope, not
            // that of every source window returned alongside it. Validate
            // hashes just as for symbol links before inferring that scope.
            let data = &row["memory"]["data"];
            if data["symbols"]
                .as_array()
                .is_some_and(|links| !links.is_empty())
            {
                return None;
            }
            let dependencies = data["dependencies"].as_object()?;
            if dependencies.is_empty() {
                return None;
            }
            for (path, sha) in dependencies {
                let document = documents.get(format!("file:{path}").as_str())?;
                if sha.as_str()? != s(document, "sha256") {
                    return None;
                }
                for symbol in symbols::in_file(db, repository, path).ok()? {
                    if symbol.sha256 != s(document, "sha256") {
                        return None;
                    }
                    seeds.insert((symbol.identity.relpath, symbol.identity.qualified_name));
                }
            }
            continue;
        };
        for link in symbol_links {
            let revision = s(link, "revision");
            if wanted
                .insert(s(link, "stable_id").to_string(), revision)
                .is_some_and(|previous| previous != revision)
            {
                return None;
            }
        }
    }
    let ids = wanted.keys().cloned().collect();
    let live = symbols::by_ids(db, repository, &ids).ok()?;
    if live.len() != wanted.len() {
        return None;
    }
    for symbol in live {
        let document = documents.get(format!("file:{}", symbol.identity.relpath).as_str())?;
        if wanted.get(&symbol.stable_id).copied() != Some(symbol.revision.as_str())
            || symbol.sha256 != s(document, "sha256")
        {
            return None;
        }
        seeds.insert((symbol.identity.relpath, symbol.identity.qualified_name));
    }
    // These notes already cover the prose topic. Their live symbols can
    // anchor related notes, but a caller/callee relationship alone does not
    // extend that topic. Explicit code questions keep dependency expansion.
    Some(seeds)
}

/// What the `symbol_links` step added to a query.
pub struct Lifted {
    /// Code windows, as candidates no passage covers.
    pub windows: Vec<Value>,
    /// Memory rows as `rank` takes them, marked `via: link`, the link
    /// followed under `link`.
    pub memories: Vec<Value>,
    pub report: Value,
}

#[derive(Default)]
struct Counts {
    considered: usize,
    withheld: usize,
    covered: usize,
    pending: usize,
    changed: usize,
    unavailable: usize,
    over_limit: usize,
    outside_scope: usize,
    literal_value: usize,
}

/// One file's live symbols and line starts, when the table describes the
/// bytes the reader reads.
struct File<'a> {
    document: &'a Value,
    live: Vec<Live>,
    starts: Vec<usize>,
}

/// The links step of a query (see the module documentation). `rows` are the
/// query's candidate rows (its memory rows are the memories it may serve),
/// `sources` and `selected_memories` what the fusion selected, in rank
/// order, and `candidates` the windows already taken. `memory_candidates` is
/// the caller's bounded ranking prefix; the query applies its delivery allowance
/// after literal matching or configured local reranking.
#[allow(clippy::too_many_arguments)]
pub fn lift(
    db: &Connection,
    repository: &str,
    mode: Mode,
    question: &str,
    memory_candidates: usize,
    snapshot: &Snapshot,
    rows: &[Value],
    sources: &[Value],
    selected_memories: &[Value],
    candidates: &[Value],
    mut trace: Option<&mut crate::memory_diagnostics::Trace>,
) -> Result<Lifted> {
    let servable: Vec<&Value> = rows.iter().filter(|row| row["kind"] == "memory").collect();
    let links = if mode == Mode::Off {
        HashMap::new()
    } else {
        links_of(db, repository, servable.iter().map(|row| &row["memory"]))?
    };
    if links.is_empty() {
        return Ok(Lifted {
            windows: Vec::new(),
            memories: Vec::new(),
            report: json!({"status": if mode == Mode::Off { "disabled" } else { "no_links" }}),
        });
    }
    let follow = mode.enabled();
    let documents: HashMap<&str, &Value> =
        snapshot.documents.iter().map(|d| (s(d, "id"), d)).collect();
    let literal_value = follow && crate::code_question::asks_for_literal_call_value(question);
    let mut scope = (follow && !literal_value)
        .then(|| crate::identifier_arm::memory_symbol_scope(db, repository, question, &documents))
        .flatten();
    let mut scope_basis = scope.as_ref().map(|_| "explicit_symbols");
    if follow
        && !literal_value
        && scope.is_none()
        && !crate::code_question::asks_for_project_decisions(question)
    {
        let named = crate::identifier_arm::explicit_names(question);
        // An unresolved explicit request must not be replaced by an inferred
        // scope, nor should a project inventory be narrowed to ranked notes.
        if named.names.is_empty() && named.paths.is_empty() {
            scope = ranked_memory_scope(
                db,
                repository,
                question,
                selected_memories,
                &links,
                &documents,
            );
            scope_basis = scope.as_ref().map(|_| "ranked_memories");
        }
    }
    // stable_id → the servable memories linking it, in recall order.
    let mut index: HashMap<&str, Vec<(&Value, &Value)>> = HashMap::new();
    for row in servable.iter().copied() {
        for link in links.get(s(&row["memory"], "id")).into_iter().flatten() {
            index
                .entry(s(link, "stable_id"))
                .or_default()
                .push((row, link));
        }
    }
    let mut counts = Counts::default();
    let mut observe = |row: &Value, reason: &str| {
        if let Some(trace) = trace.as_deref_mut() {
            trace.link(row, reason);
        }
    };
    // A code window: the memories linked to a symbol inside it.
    let mut taken: BTreeSet<String> = selected_memories
        .iter()
        .map(|row| s(row, "identity").to_string())
        .collect();
    let mut memories = Vec::new();
    let mut files: HashMap<String, Option<File<'_>>> = HashMap::new();
    for hit in sources {
        let source = &hit["source"];
        let id = s(source, "id");
        let (Some(relpath), Some(a), Some(b)) = (
            id.strip_prefix("file:"),
            source["byte_start"].as_u64(),
            source["byte_end"].as_u64(),
        ) else {
            continue;
        };
        let (a, b) = (a as usize, b as usize);
        if !files.contains_key(relpath) {
            let file = match documents.get(id) {
                Some(document) => Some(File {
                    document,
                    live: symbols::in_file(db, repository, relpath)?,
                    starts: span::line_starts(s(document, "content")),
                }),
                None => None,
            };
            files.insert(relpath.to_string(), file);
        }
        let Some(file) = files[relpath].as_ref() else {
            continue;
        };
        let sha = s(file.document, "sha256");
        let length = s(file.document, "content").len();
        for live in &file.live {
            let Some(linked) = index.get(live.stable_id.as_str()) else {
                continue;
            };
            // The table describes other bytes of this file: wait for the
            // projection.
            if live.sha256 != sha {
                counts.pending += linked.len();
                for (row, _) in linked {
                    observe(row, "pending_projection");
                }
                continue;
            }
            let range = live.range(&file.starts, length);
            if !(range.start < b && a < range.end) {
                continue;
            }
            if scope.as_ref().is_some_and(|scope| {
                !scope.contains(&(relpath.to_string(), live.identity.qualified_name.clone()))
            }) {
                counts.outside_scope += linked.len();
                for (row, _) in linked {
                    observe(row, "outside_symbol_scope");
                }
                continue;
            }
            for (row, link) in linked {
                counts.considered += 1;
                if s(link, "revision") != live.revision {
                    counts.changed += 1;
                    observe(row, "changed_revision");
                    continue;
                }
                // Already selected, or lifted through another window.
                if taken.contains(s(row, "identity")) {
                    counts.covered += 1;
                    observe(row, "already_seen");
                    continue;
                }
                if !follow {
                    counts.withheld += 1;
                    observe(row, "counted_only");
                    continue;
                }
                if literal_value {
                    counts.literal_value += 1;
                    observe(row, "literal_value_question");
                    continue;
                }
                taken.insert(s(row, "identity").to_string());
                if memories.len() >= memory_candidates {
                    counts.over_limit += 1;
                    observe(row, "candidate_limit");
                    continue;
                }
                // `link`, not `symbol_links`: a memory record with links
                // already carries its own `symbol_links` summary.
                let mut lifted = (*row).clone();
                observe(row, "candidate");
                lifted["memory"]["via"] = json!("link");
                lifted["memory"]["link"] = json!({"direction":"code_to_memory","from":hit["identity"],
                    "stable_id":live.stable_id,"revision":live.revision,
                    "qualified_name":live.identity.qualified_name,"rung":link["rung"]});
                memories.push(lifted);
            }
        }
    }
    // A memory: the current windows of the symbols it links.
    let mut ranges: Vec<(String, usize, usize)> = candidates
        .iter()
        .filter_map(|c| {
            Some((
                s(&c["source"], "id").to_string(),
                c["source"]["byte_start"].as_u64()? as usize,
                c["source"]["byte_end"].as_u64()? as usize,
            ))
        })
        .collect();
    let wanted: BTreeSet<String> = selected_memories
        .iter()
        .filter_map(|row| links.get(s(&row["memory"], "id")))
        .flatten()
        .map(|link| s(link, "stable_id").to_string())
        .collect();
    let live: BTreeMap<String, Live> = symbols::by_ids(db, repository, &wanted)?
        .into_iter()
        .map(|row| (row.stable_id.clone(), row))
        .collect();
    let mut windows = Vec::new();
    for row in selected_memories {
        for link in links.get(s(&row["memory"], "id")).into_iter().flatten() {
            counts.considered += 1;
            let Some(symbol) = live.get(s(link, "stable_id")) else {
                counts.changed += 1;
                continue;
            };
            if symbol.revision != s(link, "revision") {
                counts.changed += 1;
                continue;
            }
            let id = format!("file:{}", symbol.identity.relpath);
            let Some(document) = documents.get(id.as_str()) else {
                counts.unavailable += 1;
                continue;
            };
            if s(document, "sha256") != symbol.sha256 {
                counts.pending += 1;
                continue;
            }
            let content = s(document, "content");
            let range = symbol.range(&span::line_starts(content), content.len());
            // A window already taken covers it.
            if ranges
                .iter()
                .any(|(other, x, y)| *other == id && *x < range.end && range.start < *y)
            {
                counts.covered += 1;
                continue;
            }
            if !follow {
                counts.withheld += 1;
                continue;
            }
            if windows.len() >= WINDOW_LIMIT {
                counts.over_limit += 1;
                continue;
            }
            let facts = json!({"via":"link","direction":"memory_to_code","from":row["identity"],
                "stable_id":symbol.stable_id,"revision":symbol.revision,
                "qualified_name":symbol.identity.qualified_name,"rung":link["rung"]});
            let identity = format!("{id}:{}#{ARM}", range.start);
            if let Some(window) = crate::doc_links::window_with(
                ARM,
                document,
                range.start,
                range.end,
                identity,
                facts,
            ) {
                ranges.push((id, range.start, range.end));
                windows.push(window);
            }
        }
    }
    let mut report = json!({"status": if follow { "followed" } else { "counted" },
        "considered":counts.considered,"lifted_memories":memories.len(),"lifted_windows":windows.len(),
        "withheld":counts.withheld,"covered":counts.covered,"pending":counts.pending,
        "changed":counts.changed,"unavailable":counts.unavailable,"over_limit":counts.over_limit});
    if follow {
        report["query_symbol_scope"] = json!({"applied":scope.is_some(),
            "basis":scope_basis,
            "symbols":scope.as_ref().map_or(0, BTreeSet::len),"excluded_links":counts.outside_scope});
        report["literal_call_value"] =
            json!({"applied":literal_value,"withheld_links":counts.literal_value});
    }
    Ok(Lifted {
        windows,
        memories,
        report,
    })
}

/// Back-links: each node fact (a graph tool's citation) with a
/// `stable_id` gains `linked`, the current reviewed memories of the project
/// that link that symbol (`memories`: id, kind, title, the memory's
/// revision, and the link's rung and recorded symbol revision) and the
/// documents the reader may read that mention it (`documents`, as
/// `graph.doc_links` lists them), at most [`BACK_LINK_LIMIT`] of each. A
/// memory the reader's view withholds (stale, a draft, retired) is never
/// listed. Read-only.
pub fn attach(
    db: &Connection,
    p: &Principal,
    repository: &Value,
    nodes: &mut [Value],
) -> Result<()> {
    let project = s(repository, "id");
    let ids: BTreeSet<String> = nodes
        .iter()
        .filter_map(|node| node["stable_id"].as_str())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    // Which reviewed memories name these symbols at all: only then is the
    // reader's view of the project's memories read.
    let named: BTreeSet<String> = rows(
        db,
        "SELECT m.id AS id FROM repo_memories m, json_each(m.data,'$.symbols') j \
         WHERE m.repository_id=?1 AND m.state='reviewed' \
         AND json_extract(j.value,'$.stable_id') IN (SELECT value FROM json_each(?2)) \
         UNION SELECT g.memory_id AS id FROM gf_symbol_pins g, json_each(g.symbols) j \
         WHERE g.repository_id=?1 AND g.state='pinned' \
         AND json_extract(j.value,'$.stable_id') IN (SELECT value FROM json_each(?2))",
        params![project, json!(ids).to_string()],
    )?
    .iter()
    .map(|row| s(row, "id").to_string())
    .collect();
    // One view of what the reader may read for every node: each project's
    // documents are listed at most once per call (the project's own once,
    // shared with its memory listing).
    let mut naming = crate::doc_links::Naming::new(db, p);
    let memories: Vec<Value> = if named.is_empty() {
        Vec::new()
    } else {
        let documents = crate::memory::documents(db, repository, p)?;
        naming = naming.knowing(project, &documents);
        crate::memory::list_of(db, repository, &documents, false)?
            .into_iter()
            .filter(|m| {
                named.contains(s(m, "id")) && m["state"] == "reviewed" && m["stale"] != true
            })
            .collect()
    };
    let links = links_of(db, project, memories.iter())?;
    for node in nodes.iter_mut() {
        let Some(stable) = node["stable_id"].as_str().map(str::to_string) else {
            continue;
        };
        let listed: Vec<Value> = memories
            .iter()
            .filter_map(|memory| {
                let link = links
                    .get(s(memory, "id"))?
                    .iter()
                    .find(|link| s(link, "stable_id") == stable)?;
                Some(json!({"id":memory["id"],"kind":memory["kind"],"title":memory["title"],
                    "revision":memory["revision"],"rung":link["rung"],"symbol_revision":link["revision"]}))
            })
            .take(BACK_LINK_LIMIT)
            .collect();
        let documents = naming.documents(project, &stable, BACK_LINK_LIMIT)?;
        node["linked"] = json!({"memories":listed,"documents":documents});
    }
    Ok(())
}

/// Back-links as a connection to one project shows them: such a connection
/// keeps memory and documents on its project (`own`), while a graph tool's
/// `repo` may read another project's code graph. A graph tool's `reply`
/// keeps the reviewed memories of its nodes only when it read `own`'s graph,
/// and only `own`'s documents; the citations text, built from the reply,
/// follows.
pub fn confine(reply: &mut Value, own: &str) {
    let own_graph = reply["repo"] == own;
    for node in reply["nodes"].as_array_mut().into_iter().flatten() {
        let Some(linked) = node.get_mut("linked").and_then(Value::as_object_mut) else {
            continue;
        };
        if !own_graph {
            linked.insert("memories".into(), json!([]));
        }
        if let Some(documents) = linked.get_mut("documents").and_then(Value::as_array_mut) {
            documents.retain(|document| document["repository_id"] == own);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rationale_note(id: &str, text: &str, symbol: Option<&str>) -> Value {
        let symbols: Vec<Value> = symbol
            .into_iter()
            .map(|name| {
                json!({"state":"resolved",
            "identity":{"relpath":"policy.py","qualified_name":name}})
            })
            .collect();
        json!({"kind":"memory","identity":id,"title":"","text":text,
            "memory":{"data":{"dependencies":{"policy.py":"sha"},"symbols":symbols}}})
    }

    #[test]
    fn rationale_focus_preserves_equivalent_units_and_additional_reasons_on_the_same_symbol() {
        let limit = rationale_note(
            "limit",
            "The reservation lasts five minutes.",
            Some("limit"),
        );
        let reason = rationale_note(
            "reason",
            "Slow peers need room to acknowledge the batch.",
            Some("limit"),
        );
        let floor = rationale_note(
            "floor",
            "The minimum waiting interval is sixty seconds.",
            Some("floor"),
        );
        let linked = rationale_note(
            "linked",
            "An outstanding request expires after 300 seconds.",
            Some("lease"),
        );
        let all = vec![limit.clone(), reason.clone(), floor.clone(), linked.clone()];
        let focus = NoteFocus::new(
            "Why does the queue cap its waiting interval at 300 seconds?",
            &[limit],
            &all,
        )
        .unwrap();
        assert!(focus.keep(&reason));
        assert!(focus.keep(&linked));
        assert!(!focus.keep(&floor));
    }

    #[test]
    fn factual_subjects_keep_qualifications_without_other_same_symbol_policies() {
        let lease = rationale_note(
            "lease",
            "Our upstream's connection lease expires after one minute.",
            Some("retry_delay"),
        );
        let extra = rationale_note(
            "extra",
            "The connection lease may end early during maintenance.",
            Some("retry_delay"),
        );
        let jitter = rationale_note(
            "jitter",
            "Randomized scheduling is deferred until the gateway migration finishes.",
            Some("retry_delay"),
        );
        let all = vec![lease.clone(), extra.clone(), jitter.clone()];
        let focus = NoteFocus::new(
            "When does our upstream connection lease expire?",
            std::slice::from_ref(&lease),
            &all,
        )
        .unwrap();
        assert!(focus.keep(&lease) && focus.keep(&extra) && !focus.keep(&jitter));
        let mut unknown = jitter.clone();
        unknown["memory"]["data"]["dependencies"] = json!({});
        assert!(focus.keep(&unknown));
        assert!(
            NoteFocus::new(
                "When does our upstream connection lease expire and why is scheduling deferred?",
                &all,
                &all
            )
            .is_none()
        );
        assert!(NoteFocus::new("When does the reservation expire?", &all, &all).is_none());
        assert!(NoteFocus::new("Why is retry_delay capped at 60?", &all, &all).is_none());
    }

    #[test]
    fn named_titles_with_qualifiers_require_support_for_the_whole_qualifier() {
        let mut contract = rationale_note(
            "contract",
            "Preserve fractional minor units at the boundary.",
            Some("total"),
        );
        contract["title"] = json!("Cent settlement contract");
        let all = vec![contract.clone()];
        assert!(
            NoteFocus::new(
                "What is the cent settlement contract for fractional minor units?",
                &all,
                &all
            )
            .is_some()
        );
        assert!(
            NoteFocus::new(
                "What is the cent settlement contract for unknown currencies?",
                &all,
                &all
            )
            .is_none()
        );
        assert!(
            NoteFocus::new(
                "What is the cent settlement contract for fractional minor units and refunds?",
                &all,
                &all
            )
            .is_none()
        );
        contract["title"] = json!("Policy for account closure");
        let all = vec![contract];
        assert!(NoteFocus::new("What is the policy for account closure?", &all, &all).is_some());
    }

    #[test]
    fn compound_rationale_requires_retrieved_support_for_both_parts() {
        let wait = rationale_note(
            "wait",
            "The receiver expires after half a minute. Longer waits lose the notice.",
            Some("pauseFor"),
        );
        let wire = rationale_note(
            "wire",
            "Downstream readers require the original field order.",
            Some("Sender.send"),
        );
        let owner = rationale_note(
            "owner",
            "Repeated shutdown must be harmless.",
            Some("Sender"),
        );
        let extra = rationale_note(
            "extra",
            "Older decoders depend on this representation.",
            Some("Sender.send"),
        );
        let all = vec![wait.clone(), wire.clone(), owner.clone(), extra.clone()];
        let q = "Why does Sender.send wait at most thirty seconds and preserve field order?";
        assert!(NoteFocus::new(q, std::slice::from_ref(&wait), &all).is_none());
        assert!(NoteFocus::new(q, std::slice::from_ref(&wire), &all).is_none());
        let focus = NoteFocus::new(q, &[wait.clone(), wire.clone()], &all).unwrap();
        assert_eq!(focus.basis, "rationale_compound_agreement");
        assert!(focus.keep(&wait) && focus.keep(&wire) && focus.keep(&extra));
        assert!(!focus.keep(&owner));
        let mut unknown = owner.clone();
        unknown["memory"]["data"]["symbols"][0]["state"] = json!("pending");
        assert!(focus.keep(&unknown));
        for q in [
            "Why does Sender.send wait at most thirty seconds and encrypt payloads?",
            "Why does Sender.send wait at most thirty seconds and not preserve field order?",
            "Why does Sender.send wait at most thirty seconds or preserve field order?",
            "Why does Sender.send wait at most thirty seconds and preserve field order and allow shutdown?",
            "Why does Sender.send wait at most thirty seconds and Other.send preserve field order?",
        ] {
            assert!(NoteFocus::new(q, &all, &all).is_none(), "{q}");
        }
    }

    #[test]
    fn same_symbol_policies_need_more_than_a_shared_link_for_a_narrow_premise() {
        let rounded = rationale_note(
            "round",
            "Preserve two decimal places at the outbound boundary.",
            Some("total"),
        );
        let audit = rationale_note(
            "audit",
            "Finance retains original line items for seven years in an archive.",
            Some("total"),
        );
        let extra = rationale_note(
            "extra",
            "Card networks reject fractional minor units.",
            Some("total"),
        );
        let quantified_extra = rationale_note(
            "other",
            "A two day reconciliation window also requires decimal rounding.",
            Some("total"),
        );
        let all = vec![
            rounded.clone(),
            audit.clone(),
            extra.clone(),
            quantified_extra.clone(),
        ];
        let focus = NoteFocus::new(
            "Why does total round to two decimal places?",
            &[rounded],
            &all,
        )
        .unwrap();
        assert!(!focus.keep(&audit));
        assert!(focus.keep(&extra) && focus.keep(&quantified_extra));
        let mut unknown = audit.clone();
        unknown["memory"]["data"]["dependencies"] = json!({});
        assert!(focus.keep(&unknown));
        unknown["memory"]["data"]["symbols"][0]["state"] = json!("pending");
        unknown["memory"]["data"]["dependencies"] = json!({"policy.py":"sha"});
        assert!(focus.keep(&unknown));
        assert!(
            NoteFocus::new(
                "Why round to two decimal places and retain original line items?",
                &all,
                &all
            )
            .is_none()
        );
        assert!(NoteFocus::new("Why is retry_delay capped at 60?", &all, &all).is_none());
    }

    #[test]
    fn rationale_focus_does_not_guess_unmatched_or_compound_premises() {
        let note = rationale_note(
            "a",
            "Keep at least two live replicas during normal operation.",
            None,
        );
        let other = rationale_note("b", "Reserve one core per replica.", None);
        let all = vec![note.clone(), other.clone()];
        let focus = NoteFocus::new(
            "Why must the chart keep at least two live replicas?",
            &all,
            &all,
        )
        .unwrap();
        assert!(focus.keep(&note));
        assert!(!focus.keep(&other));
        for question in [
            "Why must replacements remain available?",
            "Why must the chart keep two replicas and reserve one core?",
            "Why must the chart keep at least two live replicas; explain capacity too?",
            "Which constraints must the chart preserve?",
            "Why wait between 60 seconds and five minutes?",
        ] {
            assert!(NoteFocus::new(question, &all, &all).is_none(), "{question}");
        }
    }

    #[test]
    fn the_switch_counts_when_left_out_follows_when_true_and_reads_none_when_false() {
        assert_eq!(Mode::parse(&json!({})).unwrap(), Mode::Counted);
        assert_eq!(
            Mode::parse(&json!({"symbol_links":null})).unwrap(),
            Mode::Counted
        );
        assert_eq!(
            Mode::parse(&json!({"symbol_links":true})).unwrap(),
            Mode::Follow
        );
        assert_eq!(
            Mode::parse(&json!({"symbol_links":false})).unwrap(),
            Mode::Off
        );
        assert!(Mode::parse(&json!({"symbol_links":"yes"})).is_err());
        assert!(Mode::Follow.enabled());
        assert!(!Mode::Counted.enabled() && !Mode::Off.enabled());
    }

    #[test]
    fn a_project_connection_keeps_back_links_on_its_project() {
        let linked = || {
            json!({"memories":[{"id":"m1"}],"documents":[
                {"repository_id":"own","document_id":"file:a.md"},
                {"repository_id":"other","document_id":"file:b.md"}]})
        };
        let reply = |repo: &str| json!({"repo":repo,"nodes":[{"stable_id":"s1","linked":linked()},{"stable_id":"s2"}]});
        // The connection's own graph: its memories stay, other projects'
        // documents go.
        let mut own = reply("own");
        confine(&mut own, "own");
        assert_eq!(own["nodes"][0]["linked"]["memories"], json!([{"id":"m1"}]));
        assert_eq!(
            own["nodes"][0]["linked"]["documents"],
            json!([{"repository_id":"own","document_id":"file:a.md"}])
        );
        assert!(own["nodes"][1].get("linked").is_none());
        // Another project's graph (`repo`): no memory, its own documents only.
        let mut other = reply("other");
        confine(&mut other, "own");
        assert_eq!(other["nodes"][0]["linked"]["memories"], json!([]));
        assert_eq!(
            other["nodes"][0]["linked"]["documents"],
            json!([{"repository_id":"own","document_id":"file:a.md"}])
        );
    }

    #[test]
    fn only_resolved_links_with_a_stable_id_count() {
        let entries = vec![
            json!({"state":"resolved","stable_id":"s1","rung":"named"}),
            json!({"state":"pending","relpath":"a.py"}),
            json!({"state":"resolved"}),
        ];
        let found: Vec<&str> = resolved(&entries)
            .map(|entry| s(entry, "stable_id"))
            .collect();
        assert_eq!(found, ["s1"]);
    }
}
