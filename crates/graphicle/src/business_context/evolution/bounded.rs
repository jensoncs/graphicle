//! Byte-bounded selection. Keep complete declarations and change records, with
//! partial commit membership explicit so it cannot become negative evidence.
use super::*;

pub(super) struct Budget {
    functions: Vec<(bool, Function, usize)>,
    history_closed: bool,
    function_bytes: usize,
    change_bytes: usize,
    function_limit: usize,
    change_limit: usize,
    requested: BTreeSet<String>,
}

fn size<T: Serialize>(value: &T) -> Result<usize> {
    Ok(serde_json::to_vec(value)?.len() + 1)
}

pub(crate) fn bound_blame(history: &mut History, limit: usize) -> Result<()> {
    ensure_safe_value(&serde_json::to_value(&history)?)?;
    let mut bytes = size(history)?;
    while bytes > limit.saturating_sub(1024) {
        let Some(file) = history
            .files
            .iter_mut()
            .filter(|f| !f.attributions.is_empty())
            .max_by_key(|f| f.attributions.len())
        else {
            return Err(Error::bad(
                "History byte budget cannot hold pinned file metadata",
            ));
        };
        let removed = file.attributions.pop().unwrap();
        bytes = bytes.saturating_sub(size(&removed)?);
        file.omitted_ranges += 1;
    }
    Ok(())
}

impl Budget {
    pub(super) fn new(history: &mut History, limit: usize) -> Result<Self> {
        check_history_budget(limit)?;
        bound_blame(history, limit / 3)?;
        // Omission counters, array punctuation and the analysis envelope remain
        // bounded independently of the retained evidence payload.
        let available = limit.saturating_sub(size(history)? + 4096);
        Ok(Self {
            functions: vec![],
            history_closed: false,
            function_bytes: 0,
            change_bytes: 0,
            function_limit: available / 3,
            change_limit: available - available / 3,
            requested: history.files.iter().map(|f| f.path.clone()).collect(),
        })
    }

    pub(super) fn function(
        &mut self,
        mut function: Function,
        analysis: &mut Evolution,
    ) -> Result<()> {
        ensure_safe_value(&serde_json::to_value(&function)?)?;
        *analysis
            .omissions
            .entry("lexical_terms".into())
            .or_default() += function.terms.len().saturating_sub(16);
        function.terms.truncate(16);
        let bytes = size(&function)?;
        self.function_bytes += bytes;
        self.functions
            .push((!self.requested.contains(&function.path), function, bytes));
        self.functions
            .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id().cmp(&b.1.id())));
        while self.function_bytes > self.function_limit || self.functions.len() > 512 {
            // Exact declaration anchors are required to attach history at all.
            // Spend optional similarity sketches only after those anchors fit.
            if self.functions.len() <= 512
                && let Some(index) = self
                    .functions
                    .iter()
                    .rposition(|(_, f, _)| !f.terms.is_empty())
            {
                let (_, function, bytes) = &mut self.functions[index];
                *analysis
                    .omissions
                    .entry("lexical_terms".into())
                    .or_default() += function.terms.len();
                function.terms.clear();
                let reduced = size(function)?;
                self.function_bytes -= *bytes - reduced;
                *bytes = reduced;
                continue;
            }
            let (_, _, bytes) = self.functions.pop().unwrap();
            self.function_bytes -= bytes;
            *analysis
                .omissions
                .entry("output_functions".into())
                .or_default() += 1;
        }
        Ok(())
    }

    pub(super) fn unit(&mut self, mut unit: Unit, analysis: &mut Evolution) -> Result<()> {
        // Scan even records omitted by the output budget. Bounds never bypass
        // credential admission. The collector supplies newest-first units.
        ensure_safe_value(&serde_json::to_value(&unit)?)?;
        for change in &mut unit.changes {
            *analysis
                .omissions
                .entry("lexical_terms".into())
                .or_default() += change.patch_terms.len().saturating_sub(16);
            change.patch_terms.truncate(16);
        }
        if self.history_closed {
            Self::omit_unit(&unit, analysis);
            return Ok(());
        }
        self.change_bytes += size(&unit)?;
        analysis.units.push(unit);
        // Complete recent membership proves that an absent function was not
        // introduced/reintroduced in that unit. Sketches and patch excerpts
        // cannot replace this evidence, however interesting an older fix is.
        for unit in analysis.units.iter_mut().rev() {
            for change in unit.changes.iter_mut().rev() {
                if self.change_bytes <= self.change_limit {
                    break;
                }
                let before = size(change)?;
                *analysis
                    .omissions
                    .entry("lexical_terms".into())
                    .or_default() += change.patch_terms.len();
                change.patch_terms.clear();
                self.change_bytes -= before - size(change)?;
            }
        }
        // Discard optional patches from incidental peers before requested files.
        // Keep hashes, membership, locations, churn and causal candidates intact.
        for requested in [false, true] {
            for unit in analysis.units.iter_mut().rev() {
                for change in unit.changes.iter_mut().rev() {
                    if self.change_bytes <= self.change_limit {
                        break;
                    }
                    if self.requested.contains(&change.path) != requested
                        || change.patch_excerpt.is_empty()
                    {
                        continue;
                    }
                    let before = size(change)?;
                    change.patch_excerpt.clear();
                    change.patch_truncated = true;
                    self.change_bytes -= before - size(change)?;
                    *analysis
                        .omissions
                        .entry("output_patch_excerpts".into())
                        .or_default() += 1;
                }
            }
        }
        if self.change_bytes > self.change_limit {
            let unit = analysis.units.pop().unwrap();
            self.change_bytes -= size(&unit)?;
            Self::omit_unit(&unit, analysis);
            // Never skip an unretained recent unit to spend bytes on an older
            // one: that would conceal an unknown continuity gap.
            self.history_closed = true;
        }
        Ok(())
    }

    fn omit_unit(unit: &Unit, analysis: &mut Evolution) {
        *analysis.omissions.entry("output_units".into()).or_default() += 1;
        *analysis
            .omissions
            .entry("output_changes".into())
            .or_default() += unit.changes.len() + unit.omitted_changes;
    }

    pub(super) fn finish(self, analysis: &mut Evolution) {
        analysis.functions = self.functions.into_iter().map(|(_, f, _)| f).collect();
        // Preserve explicit partial/empty barriers supplied by the collector.
        analysis.omissions.retain(|_, count| *count > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> History {
        serde_json::from_value(json!({"format":FORMAT,"pin":{"repository_id":"repo","directory":"app","commit":"a".repeat(40),"snapshot":"b".repeat(64),"policy_sha256":"c".repeat(64)},"shallow":false,
            "files":[{"path":"wanted.go","file_sha256":"d".repeat(64),"line_count":100,"omitted_ranges":0,"attributions":[]}]})).unwrap()
    }
    fn analysis() -> Evolution {
        Evolution {
            format: "graphicle.history-analysis.v1".into(),
            anchor_time: 1_700_000_000,
            units: vec![],
            functions: vec![],
            omissions: BTreeMap::new(),
        }
    }
    fn unit(index: usize) -> Unit {
        let changes:Vec<_> = (0..32).map(|i| json!({"path":if i<8 {"wanted.go"}else{"peer.go"},"name":format!("F{i}"),"kind":"function","before":"a".repeat(64),"after":"b".repeat(64),"added":1,"deleted":1,
            "patch_terms":(0..128).map(|n|format!("{n:064x}")).collect::<Vec<_>>(),"patch_excerpt":"ordinary source text ".repeat(80),"patch_truncated":false,"before_line":1,"after_line":1,"cause_candidates":[]})).collect();
        serde_json::from_value(json!({"commit":format!("{index:040x}"),"parent":null,"timestamp":1_700_000_000,"author_id":"a".repeat(64),"subject":"Fix invariant","files_touched":2,"changes":changes})).unwrap()
    }
    fn bounded(limit: usize) -> History {
        let mut history = history();
        let mut budget = Budget::new(&mut history, limit).unwrap();
        let mut analysis = analysis();
        for i in 0..64 {
            let function:Function=serde_json::from_value(json!({"path":if i<8 {"wanted.go"}else{"peer.go"},"file_sha256":"d".repeat(64),"name":format!("F{i}"),"kind":"function","byte_start":i*10,"byte_end":i*10+9,"line_start":i+1,"line_end":i+1,"complexity":2,
                "terms":(0..128).map(|n|format!("{n:064x}")).collect::<Vec<_>>()})).unwrap();
            budget.function(function, &mut analysis).unwrap();
        }
        for i in 0..16 {
            budget.unit(unit(i), &mut analysis).unwrap();
        }
        budget.finish(&mut analysis);
        history.evolution = Some(analysis);
        history
    }
    #[test]
    fn production_sized_samples_fit_both_sides_without_slicing_evidence() {
        let source: Vec<_> = (0..16).map(unit).collect();
        assert!(serde_json::to_vec(&source).unwrap().len() > 2 * 1024 * 1024);
        // Serialized envelopes are not source text: scan decoded fields instead.
        ensure_safe_value(&serde_json::to_value(&source).unwrap()).unwrap();
        let base = bounded(DEFAULT_HISTORY_BYTES);
        let mut head = base.clone();
        head.pin.commit = "e".repeat(40);
        let options = Options {
            history: vec![base.clone(), head.clone()],
        };
        assert!(serde_json::to_vec(&options).unwrap().len() <= MAX_HISTORY_BYTES);
        validate(
            &options,
            std::slice::from_ref(&base.pin),
            &[head.pin],
            |_, _| Some(("d".repeat(64), 100)),
        )
        .unwrap();
        let evolution = base.evolution.unwrap();
        assert!(evolution.omissions["output_changes"] > 0);
        assert!(evolution.units.iter().all(|u| u.omitted_changes == 0));
        assert_eq!(
            evolution
                .functions
                .iter()
                .filter(|f| f.path == "wanted.go")
                .count(),
            8
        );
        assert!(
            evolution
                .units
                .iter()
                .flat_map(|u| &u.changes)
                .all(|c| c.patch_excerpt == "ordinary source text ".repeat(80)
                    || (c.patch_excerpt.is_empty() && c.patch_truncated))
        );
        let entry =
            json!({"file":"wanted.go","file_sha256":"d".repeat(64),"byte_start":0,"byte_end":9});
        let evidence = evolution.evidence(&entry, false).unwrap();
        assert_eq!(evidence["sample"]["coupling_available"], false);
        assert!(!evidence["function_history"].as_array().unwrap().is_empty());
        assert!(evidence["cochange"].as_array().unwrap().is_empty());
        assert!(serde_json::to_vec(&bounded(16 * 1024)).unwrap().len() <= 16 * 1024);
    }
    #[test]
    fn complete_recent_membership_keeps_older_evidence_without_crossing_unknown_gaps() {
        let mut history = history();
        let mut budget = Budget::new(&mut history, 16 * 1024).unwrap();
        let mut analysis = analysis();
        let f:Function=serde_json::from_value(json!({"path":"wanted.go","file_sha256":"d".repeat(64),"name":"F6","kind":"function","byte_start":0,"byte_end":9,"line_start":1,"line_end":1,"complexity":2,"terms":[]})).unwrap();
        budget.function(f, &mut analysis).unwrap();
        let mut recent = unit(0);
        recent.changes.truncate(5);
        let mut older = unit(1);
        older.changes = vec![older.changes.remove(6)];
        budget.unit(recent, &mut analysis).unwrap();
        budget.unit(older, &mut analysis).unwrap();
        budget.finish(&mut analysis);
        assert_eq!(analysis.units.len(), 2);
        assert_eq!(analysis.units[0].changes.len(), 5);
        assert_eq!(analysis.units[0].omitted_changes, 0);
        let entry =
            json!({"file":"wanted.go","file_sha256":"d".repeat(64),"byte_start":0,"byte_end":9});
        let evidence = analysis.evidence(&entry, false).unwrap();
        assert_eq!(
            evidence["function_history"][0]["commit"],
            format!("{:040x}", 1)
        );
        // A genuine unknown membership gap must still stop correspondence.
        analysis.units[0].changes.pop();
        analysis.units[0].omitted_changes = 1;
        assert!(
            analysis.evidence(&entry, false).unwrap()["function_history"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_unaffordable_recent_unit_cannot_be_skipped_for_a_small_old_fix() {
        let mut history = history();
        let mut budget = Budget::new(&mut history, 16 * 1024).unwrap();
        budget.change_limit = 1024;
        let mut analysis = analysis();
        budget.unit(unit(0), &mut analysis).unwrap();
        let mut old = unit(1);
        old.changes.truncate(1);
        budget.unit(old, &mut analysis).unwrap();
        budget.finish(&mut analysis);
        assert!(analysis.units.is_empty());
        assert_eq!(analysis.omissions["output_units"], 2);
    }

    #[test]
    fn credential_checks_apply_before_selection_and_to_decoded_fields() {
        let mut history = history();
        let mut budget = Budget::new(&mut history, 16 * 1024).unwrap();
        let mut analysis = analysis();
        let mut bad = unit(0);
        // A low-priority record that would not fit must still be scanned.
        bad.changes.last_mut().unwrap().patch_excerpt = format!("ghp_{}", "q".repeat(36));
        assert!(budget.unit(bad, &mut analysis).is_err());
        let encoded = format!("{{\"subject\":\"ghp_{}\"}}", "\\u0071".repeat(36));
        assert!(ensure_safe_value(&serde_json::from_str(&encoded).unwrap()).is_err());
        assert!(ensure_safe_value(&json!({"subject":"ordinary text ".repeat(100_000)})).is_err());
    }
}
