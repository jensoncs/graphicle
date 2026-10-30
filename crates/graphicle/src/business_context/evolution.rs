//! Bounded, caller-attested change evidence. Scores describe observed history;
//! they neither infer business importance nor authorize reduced review scope.
use super::*;
use graphicle_memory::structure;

mod bounded;
mod collect;
pub(super) use bounded::bound_blame;
pub use collect::collect;
const MAX_UNITS: usize = 64;
const HALF_LIFE_SECONDS: f64 = 180.0 * 86400.0;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Evolution {
    pub format: String,
    pub anchor_time: i64,
    pub units: Vec<Unit>,
    pub functions: Vec<Function>,
    pub omissions: BTreeMap<String, usize>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub commit: String,
    pub parent: Option<String>,
    pub timestamp: i64,
    pub author_id: String,
    pub subject: String,
    pub files_touched: usize,
    pub changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted_changes: usize,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub added: usize,
    pub deleted: usize,
    pub patch_terms: Vec<String>,
    pub patch_excerpt: String,
    pub patch_truncated: bool,
    pub before_line: Option<usize>,
    pub after_line: Option<usize>,
    pub cause_candidates: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub path: String,
    pub file_sha256: String,
    pub name: String,
    pub kind: String,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_start: usize,
    pub line_end: usize,
    pub complexity: Option<u64>,
    pub terms: Vec<String>,
}
fn is_zero(value: &usize) -> bool {
    *value == 0
}

fn identity(path: &str, name: &str, kind: &str) -> String {
    crate::digest(&json!([path, name, kind]))
}
impl Function {
    fn id(&self) -> String {
        identity(&self.path, &self.name, &self.kind)
    }
}
impl Change {
    fn id(&self) -> String {
        identity(&self.path, &self.name, &self.kind)
    }
}
fn weight(unit: &Unit, anchor: i64) -> f64 {
    2_f64.powf(-((anchor - unit.timestamp).max(0) as f64) / HALF_LIFE_SECONDS)
        / unit.files_touched.max(1) as f64
}
impl Evolution {
    pub(super) fn validate(
        &self,
        history: &History,
        source: &impl Fn(&Pin, &str) -> Option<(String, usize)>,
    ) -> Result<()> {
        if self.format != "graphicle.history-analysis.v1"
            || !(0..=4_102_444_800).contains(&self.anchor_time)
            || self.units.len() > MAX_UNITS
            || self.functions.len() > 512
            || self.omissions.len() > 16
        {
            return Err(Error::bad(
                "Historical analysis exceeds supported format or bounds",
            ));
        }
        let mut commits = BTreeSet::new();
        for unit in &self.units {
            if !commit_id(&unit.commit)
                || unit.parent.as_ref().is_some_and(|p| !commit_id(p))
                || !commits.insert(&unit.commit)
                || !(0..=4_102_444_800).contains(&unit.timestamp)
                || unit.subject.len() > 512
                || unit.subject.chars().any(char::is_control)
                || !(1..=32).contains(&unit.files_touched)
                || unit.changes.len() > 128
                || unit.omitted_changes > 128
                || unit.changes.len() + unit.omitted_changes > 128
            {
                return Err(Error::bad("Invalid historical change unit"));
            }
            store::hash(&unit.author_id)?;
            let mut changed = BTreeSet::new();
            for change in &unit.changes {
                admitted_path(&change.path)?;
                if !changed.insert(change.id())
                    || change.name.len() > 512
                    || change.kind.len() > 64
                    || change.added > 1_000_000
                    || change.deleted > 1_000_000
                    || change.patch_excerpt.len() > 2048
                    || change
                        .before_line
                        .is_some_and(|line| line == 0 || line > 1_000_000)
                    || change
                        .after_line
                        .is_some_and(|line| line == 0 || line > 1_000_000)
                    || change.cause_candidates.len() > 4
                    || change.cause_candidates.iter().any(|id| !commit_id(id))
                    || change.before.is_none() && change.after.is_none()
                {
                    return Err(Error::bad("Invalid historical function change"));
                }
                for hash in change
                    .before
                    .iter()
                    .chain(&change.after)
                    .chain(&change.patch_terms)
                {
                    store::hash(hash)?;
                }
                if change.patch_terms.len() > 128 {
                    return Err(Error::bad("Historical patch terms exceed bound"));
                }
            }
        }
        let mut functions = BTreeSet::new();
        for function in &self.functions {
            admitted_path(&function.path)?;
            store::hash(&function.file_sha256)?;
            let Some((sha, lines)) = source(&history.pin, &function.path) else {
                return Err(Error::conflict("Historical function source unavailable"));
            };
            if sha != function.file_sha256
                || function.line_start == 0
                || function.line_start > function.line_end
                || function.line_end > lines
                || function.byte_start >= function.byte_end
                || function.byte_end > 1024 * 1024
                || function.name.len() > 512
                || function.kind.len() > 64
                || !functions.insert(function.id())
                || function.terms.len() > 128
                || function.complexity.is_some_and(|n| n == 0 || n > 2_000_001)
            {
                return Err(Error::conflict(
                    "Historical function differs from pinned source or bounds",
                ));
            }
            for hash in &function.terms {
                store::hash(hash)?;
            }
        }
        Ok(())
    }

    fn function(&self, entry: &Value) -> Option<&Function> {
        let matches: Vec<_> = self
            .functions
            .iter()
            .filter(|f| {
                entry["file"] == f.path
                    && entry["file_sha256"] == f.file_sha256
                    && entry["byte_start"] == f.byte_start
                    && entry["byte_end"] == f.byte_end
            })
            .collect();
        (matches.len() == 1).then(|| matches[0])
    }

    pub(super) fn evidence(&self, entry: &Value, shallow: bool) -> Option<Value> {
        let function = self.function(entry)?;
        let target = function.id();
        let mut total = 0.0;
        let mut a = 0.0;
        let mut count_a = 0;
        let mut churn = 0.0;
        let mut marginals: BTreeMap<String, f64> = BTreeMap::new();
        let mut joint: BTreeMap<String, (f64, usize, Vec<String>)> = BTreeMap::new();
        let mut history = vec![];
        let mut similar = vec![];
        let mut retired = BTreeSet::new();
        let mut sampled = 0;
        for unit in &self.units {
            sampled += 1;
            let w = weight(unit, self.anchor_time);
            let complete = unit.omitted_changes == 0;
            if complete {
                total += w;
            }
            let ids: BTreeSet<_> = unit
                .changes
                .iter()
                .map(Change::id)
                .filter(|id| !retired.contains(id))
                .collect();
            for id in ids.iter().filter(|_| complete) {
                *marginals.entry(id.clone()).or_default() += w;
            }
            if ids.contains(&target) {
                if complete {
                    a += w;
                    count_a += 1;
                }
                for id in ids.iter().filter(|_| complete) {
                    if id != &target {
                        let pair = joint.entry(id.clone()).or_default();
                        pair.0 += w;
                        pair.1 += 1;
                        if pair.2.len() < 4 {
                            pair.2.push(unit.commit.clone());
                        }
                    }
                }
                if let Some(change) = unit.changes.iter().find(|c| c.id() == target) {
                    churn += w * (change.added + change.deleted) as f64;
                    history.push(json!({"commit":unit.commit,"parent":unit.parent,"subject":unit.subject,"kind":assertion_kind(&unit.subject),"added":change.added,"deleted":change.deleted,"before":change.before,"after":change.after,"cause_candidates":change.cause_candidates,"patch_excerpt":change.patch_excerpt,"patch_truncated":change.patch_truncated,"patch_scope":"function","unit_omitted_changes":unit.omitted_changes,"before_line":change.before_line,"after_line":change.after_line}));
                }
            } else {
                let wanted: BTreeSet<_> = function.terms.iter().collect();
                for change in unit.changes.iter().filter(|c| c.id() != target) {
                    let observed: BTreeSet<_> = change.patch_terms.iter().collect();
                    let intersection = wanted.intersection(&observed).count();
                    let union = wanted.union(&observed).count();
                    if intersection >= 3 && union > 0 {
                        similar.push(json!({"commit":unit.commit,"path":change.path,"name":change.name,"subject":unit.subject,"similarity":intersection as f64/union as f64,"basis":"lexical_patch_jaccard","causal":false,"patch_excerpt":change.patch_excerpt,"patch_truncated":change.patch_truncated}));
                    }
                }
            }
            if unit
                .changes
                .iter()
                .any(|c| c.id() == target && c.before.is_none() && c.after.is_some())
            {
                break;
            }
            // A missing change could have introduced/reintroduced the target.
            // Do not join older incarnations across such a gap.
            if !complete && !ids.contains(&target) {
                break;
            }
            retired.extend(
                unit.changes
                    .iter()
                    .filter(|c| c.before.is_none() && c.after.is_some())
                    .map(Change::id),
            );
        }
        // Evicted membership can conceal introductions and invalidate lifetime
        // correspondence. Preserve historical records, but withhold coupling.
        let coupling_available = self.units.iter().all(|u| u.omitted_changes == 0)
            && [
                "output_units",
                "unavailable_or_withheld_unit",
                "mass_change",
            ]
            .iter()
            .all(|key| self.omissions.get(*key).copied().unwrap_or(0) == 0);
        let mut coupling = vec![];
        for partner in &self.functions {
            let id = partner.id();
            let Some((ab, support, commits)) = joint.get(&id) else {
                continue;
            };
            if !coupling_available || *support < 3 || count_a < 3 || a <= 0.0 || total <= 0.0 {
                continue;
            }
            let conditional = ab / a;
            let prevalence = marginals[&id] / total;
            let lift = conditional / prevalence;
            if conditional >= 0.7 && lift >= 1.2 {
                coupling.push(json!({"file":partner.path,"file_sha256":partner.file_sha256,"name":partner.name,"byte_start":partner.byte_start,"byte_end":partner.byte_end,"line_start":partner.line_start,"line_end":partner.line_end,"support":support,"target_units":count_a,"conditional_probability":conditional,"lift":lift,"weighted_joint":ab,"weighted_target":a,"weighted_partner":marginals[&id],"weighted_total":total,"commits":commits,"relation":"historical_cochange","causal":false}));
            }
        }
        coupling.sort_by(|a, b| {
            b["lift"]
                .as_f64()
                .unwrap()
                .total_cmp(&a["lift"].as_f64().unwrap())
                .then_with(|| a["file"].as_str().cmp(&b["file"].as_str()))
        });
        similar.sort_by(|a, b| {
            b["similarity"]
                .as_f64()
                .unwrap()
                .total_cmp(&a["similarity"].as_f64().unwrap())
        });
        history.sort_by_key(|h| h["kind"] == "historical_intent");
        let omitted_history = history.len().saturating_sub(4);
        history.truncate(4);
        let omitted_coupling = coupling.len().saturating_sub(3);
        coupling.truncate(3);
        let omitted_similar = similar.len().saturating_sub(2);
        similar.truncate(2);
        Some(
            json!({"authority":"caller_attested","validity":"unverified","function_history":history,"cochange":coupling,"similar_changes":similar,
            "hotspot":{"weighted_changed_lines":churn,"decision_complexity":function.complexity,"churn_times_complexity":function.complexity.map(|c|churn*c as f64),"basis":"ast_decision_proxy","business_criticality":"unknown"},
            "sample":{"units":sampled,"window_units":self.units.len(),"target_units":count_a,"weight":"2^(-age_days/180)/files_touched","unit":"first_parent_integration_commit","coupling_basis":"complete_retained_units_only","coupling_available":coupling_available,"shallow":shallow,"omissions":self.omissions,"complete":false},
            "omitted_history":omitted_history,"omitted_cochange":omitted_coupling,"omitted_similar":omitted_similar}),
        )
    }
}

pub(super) fn ownership(file: &FileHistory, start: usize, end: usize, anchor: i64) -> Value {
    let mut owners: BTreeMap<&str, f64> = BTreeMap::new();
    let mut known = 0;
    for range in &file.attributions {
        let (Some(author), Some(timestamp)) = (range.author_id.as_deref(), range.timestamp) else {
            continue;
        };
        let from = start.max(range.line_start);
        let to = end.min(range.line_end);
        if from > to {
            continue;
        }
        known += to - from + 1;
        *owners.entry(author).or_default() += (to - from + 1) as f64
            * 2_f64.powf(-((anchor - timestamp).max(0) as f64) / HALF_LIFE_SECONDS);
    }
    let total: f64 = owners.values().sum();
    let mut ranked: Vec<_> = owners.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    json!({"basis":"recency_weighted_surviving_line_blame","covered_lines":known,"required_lines":end-start+1,"contributors":ranked.iter().take(3).map(|(id,w)|json!({"author_id":id,"share":if total>0.0{w/total}else{0.0}})).collect::<Vec<_>>(),"omitted_contributors":ranked.len().saturating_sub(3),"purpose":"review_routing_not_author_performance"})
}

pub(super) fn schema() -> Value {
    // The typed decoder and validate enforce nested fields and bounds; this
    // schema exposes the optional bounded analysis without altering v1 blame.
    json!({"type":"object","properties":{"format":{"const":"graphicle.history-analysis.v1"},"anchor_time":{"type":"integer"},"units":{"type":"array","maxItems":64,"items":{"type":"object"}},"functions":{"type":"array","maxItems":512,"items":{"type":"object"}},"omissions":{"type":"object"}},"required":["format","anchor_time","units","functions","omissions"],"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn function(name: &str) -> Function {
        Function {
            path: format!("{name}.go"),
            file_sha256: "a".repeat(64),
            name: name.into(),
            kind: "function".into(),
            byte_start: 0,
            byte_end: 10,
            line_start: 1,
            line_end: 1,
            complexity: Some(5),
            terms: vec!["a".repeat(64), "b".repeat(64), "c".repeat(64)],
        }
    }
    fn change(name: &str) -> Change {
        Change {
            path: format!("{name}.go"),
            name: name.into(),
            kind: "function".into(),
            before: Some("a".repeat(64)),
            after: Some("b".repeat(64)),
            added: 2,
            deleted: 1,
            patch_terms: vec!["a".repeat(64), "b".repeat(64), "c".repeat(64)],
            patch_excerpt: "@@ -1 +1 @@\n-old\n+new\n".into(),
            patch_truncated: false,
            before_line: Some(1),
            after_line: Some(1),
            cause_candidates: vec![],
        }
    }
    fn unit(i: usize, names: &[&str]) -> Unit {
        Unit {
            commit: format!("{i:040x}"),
            parent: Some(format!("{:040x}", i + 1)),
            timestamp: 1_700_000_000,
            author_id: "d".repeat(64),
            subject: "Fix isolation".into(),
            files_touched: names.len(),
            changes: names.iter().map(|n| change(n)).collect(),
            omitted_changes: 0,
        }
    }
    #[test]
    fn coupling_is_conditional_age_weighted_and_requires_support() {
        let f = function("Check");
        let mut analysis = Evolution {
            format: "graphicle.history-analysis.v1".into(),
            anchor_time: 1_700_000_000,
            units: (0..3)
                .map(|i| unit(i, &["Check", "Test"]))
                .chain((3..7).map(|i| unit(i, &["Other"])))
                .collect(),
            functions: vec![f.clone(), function("Test"), function("Other")],
            omissions: BTreeMap::new(),
        };
        let entry = json!({"file":f.path,"file_sha256":f.file_sha256,"byte_start":0,"byte_end":10});
        let evidence = analysis.evidence(&entry, false).unwrap();
        let coupling = &evidence["cochange"][0];
        assert_eq!(coupling["support"], 3);
        assert_eq!(coupling["conditional_probability"], 1.0);
        assert!((coupling["lift"].as_f64().unwrap() - 5.5 / 1.5).abs() < 1e-9);
        assert_eq!(evidence["hotspot"]["weighted_changed_lines"], 4.5);
        assert_eq!(evidence["hotspot"]["churn_times_complexity"], 22.5);
        analysis
            .omissions
            .insert("unavailable_or_withheld_unit".into(), 1);
        let incomplete = analysis.evidence(&entry, false).unwrap();
        assert_eq!(incomplete["sample"]["coupling_available"], false);
        assert!(incomplete["cochange"].as_array().unwrap().is_empty());
        analysis.omissions.clear();
        assert_eq!(
            evidence["similar_changes"][0]["basis"],
            "lexical_patch_jaccard"
        );
        let mut older = unit(50, &["Check", "Test"]);
        older.timestamp -= 180 * 86400;
        assert!((weight(&older, analysis.anchor_time) - 0.25).abs() < 1e-9);
        analysis.units.remove(0);
        assert!(
            analysis.evidence(&entry, false).unwrap()["cochange"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn function_reintroduction_does_not_inherit_an_old_incarnation() {
        let f = function("Check");
        let mut added = unit(1, &["Check"]);
        added.changes[0].before = None;
        let analysis = Evolution {
            format: "graphicle.history-analysis.v1".into(),
            anchor_time: 1_700_000_000,
            units: vec![
                added,
                unit(2, &["Check", "Test"]),
                unit(3, &["Check", "Test"]),
                unit(4, &["Check", "Test"]),
            ],
            functions: vec![f.clone(), function("Test")],
            omissions: BTreeMap::new(),
        };
        let evidence = analysis
            .evidence(
                &json!({"file":f.path,"file_sha256":f.file_sha256,"byte_start":0,"byte_end":10}),
                false,
            )
            .unwrap();
        assert_eq!(evidence["sample"]["target_units"], 1);
        assert_eq!(evidence["sample"]["units"], 1);
        assert!(evidence["cochange"].as_array().unwrap().is_empty());
    }
    #[test]
    fn peer_limit_prioritizes_head_and_reports_omissions() {
        let names = ["A", "B", "C", "D"];
        let analysis = Evolution {
            format: "graphicle.history-analysis.v1".into(),
            anchor_time: 1_700_000_000,
            units: (0..3)
                .map(|i| unit(i, &names))
                .chain((3..7).map(|i| unit(i, &["Other"])))
                .collect(),
            functions: names.iter().map(|n| function(n)).collect(),
            omissions: BTreeMap::new(),
        };
        let pin: Pin = serde_json::from_value(json!({"repository_id":"repo","directory":"app","commit":"a".repeat(40),"snapshot":"b".repeat(64),"policy_sha256":"c".repeat(64)})).unwrap();
        let options = Options {
            history: vec![History {
                format: FORMAT.into(),
                pin: pin.clone(),
                shallow: false,
                files: vec![],
                evolution: Some(analysis),
            }],
        };
        let entries: Vec<_> = ["base", "head"].into_iter().flat_map(|side| ["A", "B"].into_iter().map(move |name| {
            json!({"side":side,"repository_id":"repo","symbol_id":name,"file":format!("{name}.go"),"file_sha256":"a".repeat(64),"byte_start":0,"byte_end":10,"reasons":[{"reason":"change_seed"}]})
        })).collect();
        let (peers, omitted) = suggestions(
            &json!({"entries":entries}),
            &options,
            std::slice::from_ref(&pin),
            std::slice::from_ref(&pin),
        );
        assert_eq!(peers.len(), 6);
        assert_eq!(omitted, 2);
        assert_eq!(peers.iter().filter(|p| p["side"] == "head").count(), 4);
        assert!(
            peers
                .iter()
                .all(|p| p["compiler_dependency"] == false && p["validity"] == "unverified")
        );
    }
    #[test]
    fn ownership_weights_lines_by_age_without_exposing_identity() {
        let file = FileHistory {
            path: "f.go".into(),
            file_sha256: "a".repeat(64),
            line_count: 2,
            omitted_ranges: 0,
            attributions: vec![
                Attribution {
                    commit: "a".repeat(40),
                    line_start: 1,
                    line_end: 1,
                    subject: "first".into(),
                    author_id: Some("b".repeat(64)),
                    timestamp: Some(1_700_000_000),
                },
                Attribution {
                    commit: "c".repeat(40),
                    line_start: 2,
                    line_end: 2,
                    subject: "second".into(),
                    author_id: Some("d".repeat(64)),
                    timestamp: Some(1_700_000_000 - 180 * 86400),
                },
            ],
        };
        let evidence = ownership(&file, 1, 2, 1_700_000_000);
        assert_eq!(evidence["covered_lines"], 2);
        assert!((evidence["contributors"][0]["share"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    }
}
