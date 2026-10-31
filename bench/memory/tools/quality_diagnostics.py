"""Local evaluation diagnostics; no telemetry and no changes to served context."""
import re
from collections import Counter


def memory_diagnostics(packet, known, relevant):
    """Join observed runtime stages to evaluator-only labels, without guessing
    why an ID is absent from the authorized/bounded candidate trace.
    """
    gold = set(relevant)
    returned = [known[m["id"]] for m in packet["memories"]]
    trace = packet.get("memory_candidates_debug")
    available = isinstance(trace, dict) and trace.get("version") == 1
    candidates = {known[c["id"]]: c for c in trace["candidates"]} if available else {}
    missing, false_positives = [], []
    for identity in sorted(gold - set(returned)):
        candidate = candidates.get(identity)
        reasons = candidate.get("link_outcomes", {}) if candidate else {}
        if not available:
            stage = "diagnostics_unavailable"
        elif candidate is None:
            stage = "not_observed_in_candidates"
        elif candidate.get("pack_rank") is not None:
            stage = "packing"
        elif (candidate.get("link_final_rank") or 0) > trace["linked_memory_limit"]:
            stage = "linked_final_cap"
        elif reasons.get("candidate_limit"):
            stage = "linked_candidate_cap"
        elif any(reasons.get(r) for r in ("outside_symbol_scope", "literal_value_question",
                                         "changed_revision", "pending_projection", "counted_only")):
            stage = "link_withheld"
        else:
            stage = "eligible_not_selected"
        missing.append({"id": identity, "stage": stage,
                        "link_outcomes": reasons, "candidate": candidate})
    for rank, (identity, memory) in enumerate(zip(returned, packet["memories"]), 1):
        if identity not in gold:
            false_positives.append({"id": identity, "rank": rank,
                "route": "symbol_link" if memory.get("via") == "link" else "ranked",
                "linked_symbol": memory.get("link", {}).get("qualified_name"),
                "candidate": candidates.get(identity)})
    at_k = {}
    for k in (1, 4, 8):
        prefix = returned[:k]
        tp = len(set(prefix) & gold)
        at_k[str(k)] = {"returned": len(prefix), "tp": tp,
                       "precision_returned": tp / len(prefix) if prefix else None,
                       "recall": tp / len(gold) if gold else None}
    observed = set(candidates)
    return {"trace_available": available,
            "trace_truncated": trace["truncated"] if available else None,
            "missing": missing, "false_positives": false_positives, "at_k": at_k,
            "reciprocal_rank": next((1 / rank for rank, identity in enumerate(returned, 1)
                                     if identity in gold), 0) if gold else None,
            "observed_candidate_relevant": len(observed & gold) if available else None,
            "gold_count": len(gold),
            "observed_packing_relevant": sum(identity in gold and c.get("pack_rank") is not None
                                              for identity, c in candidates.items()) if available else None}


def summarize_memory_diagnostics(cases):
    """Report coverage with diagnostic metrics; failed calls stay unmeasured."""
    measured = [c["memory_diagnostics"] for c in cases
                if not c.get("error") and c.get("memory_diagnostics")]
    traced = [d for d in measured if d["trace_available"]]
    stages = Counter(m["stage"] for d in measured for m in d["missing"])
    routes = Counter(fp["route"] for d in measured for fp in d["false_positives"])
    def mean(values):
        values = [v for v in values if v is not None]
        return {"value": sum(values) / len(values) if values else None, "n": len(values)}
    gold = sum(d["gold_count"] for d in traced)
    return {"planned": len(cases), "measured": len(measured), "traced": len(traced),
            "truncated_traces": sum(d["trace_truncated"] for d in traced),
            "unmeasured": len(cases) - len(measured),
            "missing_by_observed_stage": dict(sorted(stages.items())),
            "false_positives_by_route": dict(sorted(routes.items())),
            "observed_candidate_recall_lower_bound":
                sum(d["observed_candidate_relevant"] for d in traced) / gold if gold else None,
            "observed_packing_recall_lower_bound":
                sum(d["observed_packing_relevant"] for d in traced) / gold if gold else None,
            "traced_gold_occurrences": gold,
            "mean_reciprocal_rank": mean(d["reciprocal_rank"] for d in measured),
            "at_k": {str(k): {metric: mean(d["at_k"][str(k)][metric] for d in measured)
                               for metric in ("precision_returned", "recall")} for k in (1, 4, 8)}}


def diagnostics(packet, stderr, required=()):
    # A case/variant's artifact path is its correlation ID. Preserve repeated
    # phases; these are nested timings and must never be summed as total latency.
    phases = []
    for line in stderr.splitlines():
        match = re.fullmatch(r"phase=(\S+) ms=(\d+) rss_mb=([\d.]+) peak_rss_mb=([\d.]+)", line)
        if match:
            name, ms, rss, peak = match.groups()
            phases.append({"phase": name, "ms": int(ms), "rss_mb": float(rss),
                           "peak_rss_mb": float(peak)})
    result = {"phases": phases}
    for key in ("route", "mechanisms", "omitted", "source_state", "source_age",
                "local_reranker", "symbol_links", "code_graph", "retrieval_defaults",
                "memory_candidates_debug"):
        if key in packet:
            result[key] = packet[key]
    if required:
        # Debug output is bounded to the requested top candidates. Absence here
        # is NOT proof that a source was unindexed or absent from every arm.
        candidates = packet.get("candidates_debug")
        result["required_spans"] = []
        for gold in required:
            visible = None if candidates is None else any(
                c.get("id") == "file:" + gold["file"]
                and isinstance(c.get("byte_start"), int)
                and isinstance(c.get("byte_end"), int)
                and c["byte_start"] <= gold["byte_start"]
                and c["byte_end"] >= gold["byte_end"] for c in candidates)
            result["required_spans"].append({"file": gold["file"],
                "byte_start": gold["byte_start"], "byte_end": gold["byte_end"],
                "contained_in_observed_candidate": visible})
    return result
