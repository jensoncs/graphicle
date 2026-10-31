//! Bounded observations of authorized memory candidates, never retrieval input.
//! Enabled by the existing `debug_candidates` option. No bodies or titles are
//! copied, and absence from this trace is not proof of absence from the store.
use crate::store::s;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub struct Trace {
    limit: usize,
    eligible: usize,
    linked_limit: usize,
    entries: BTreeMap<String, Value>,
}

impl Trace {
    pub fn new(limit: usize, rows: &[Value], ranked: &Value) -> Self {
        let mut entries = BTreeMap::new();
        let mut eligible = 0;
        for row in rows.iter().filter(|r| r["kind"] == "memory") {
            eligible += 1;
            if entries.len() < limit {
                entries.insert(
                    s(row, "identity").to_string(),
                    json!({
                    "id":row["memory"]["id"], "fusion":null,
                    "retrieval_arms":ranked["reasons"].get(s(row,"identity")).cloned().unwrap_or(json!([])),
                    "link_outcomes":{}, "link_candidate_rank":null,
                    "link_final_rank":null, "reranker":null,
                    "selected_by_rank":false, "pack_rank":null,
                    "delivered":false, "delivery_route":null}),
                );
            }
        }
        for rank in ranked["memory_fused_debug"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(entry) = entries.get_mut(s(rank, "identity")) {
                entry["fusion"] = json!({"rank":rank["rank"], "score":rank["score"]});
            }
        }
        Self {
            limit,
            eligible,
            linked_limit: crate::symbol_links::MEMORY_LIMIT,
            entries,
        }
    }

    pub fn link(&mut self, row: &Value, reason: &str) {
        if let Some(entry) = self.entries.get_mut(s(row, "identity")) {
            let count = entry["link_outcomes"][reason].as_u64().unwrap_or(0);
            entry["link_outcomes"][reason] = json!(count + 1);
        }
    }

    pub fn linked_candidates(&mut self, rows: &[Value]) {
        for (rank, row) in rows.iter().enumerate() {
            if let Some(entry) = self.entries.get_mut(s(row, "identity")) {
                entry["link_candidate_rank"] = json!(rank + 1);
            }
        }
    }

    pub fn reranked(&mut self, row: &Value, score: f64, truncated: bool) {
        if let Some(entry) = self.entries.get_mut(s(row, "identity")) {
            entry["reranker"] = json!({"score":score, "input_truncated":truncated});
        }
    }

    /// Read the final order before applying the separate linked-note cap.
    pub fn linked_order(&mut self, rows: &[Value], limit: usize) {
        self.linked_limit = limit;
        for (rank, row) in rows.iter().enumerate() {
            if let Some(entry) = self.entries.get_mut(s(row, "identity")) {
                entry["link_final_rank"] = json!(rank + 1);
            }
        }
    }

    pub fn offered(&mut self, selected: &[Value], linked: &[Value]) {
        for (rank, row) in selected.iter().chain(linked).enumerate() {
            if let Some(entry) = self.entries.get_mut(s(row, "identity")) {
                entry["selected_by_rank"] = json!(rank < selected.len());
                entry["pack_rank"] = json!(rank + 1);
            }
        }
    }

    pub fn finish(mut self, packet: &Value) -> Value {
        let delivered: BTreeMap<&str, &Value> = packet["memories"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| (s(m, "id"), m))
            .collect();
        for entry in self.entries.values_mut() {
            if let Some(memory) = delivered.get(s(entry, "id")) {
                entry["delivered"] = json!(true);
                entry["delivery_route"] = json!(if memory["via"] == "link" {
                    "symbol_link"
                } else {
                    "ranked"
                });
            }
        }
        json!({"version":1, "limit":self.limit, "eligible_candidates":self.eligible,
            "recorded_candidates":self.entries.len(),
            "truncated":self.eligible > self.entries.len(),
            "scope":"authorized servable candidate rows; fusion is a bounded prefix",
            "linked_memory_limit":self.linked_limit,
            "candidates":self.entries.into_values().collect::<Vec<_>>()})
    }
}
