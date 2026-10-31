#!/usr/bin/env python3
"""Calibrate a linked-memory cutoff from development-only instrumented CLI runs.

This scores raw logits, not probabilities. The runtime computes blended order
before filtering. Replaying that order allows deterministic threshold selection;
the chosen threshold must subsequently be verified through the real CLI.
"""
import argparse
import json
import math
from pathlib import Path

from retrieval_recall import digest, save
from symbol_link_precision import validate

MEMORY_LIMIT = 4


def returned(observation, cutoff):
    if cutoff is not None and not math.isfinite(cutoff):
        raise ValueError("Cutoff must be finite")
    eligible = [c["memory"] for c in observation["candidates"]
                if cutoff is None or c["score"] >= cutoff]
    return observation["fixed"] + eligible[:MEMORY_LIMIT]


def load_observations(dataset, run):
    validate(dataset)
    results = json.loads((run / "RESULTS.json").read_text())
    if results["counted_content_mismatches"] or any(
            v["failed"] for v in results["summary"]["variants"].values()):
        raise ValueError("Only complete, valid CLI runs can calibrate a cutoff")
    projects = {p["id"]: p for p in dataset["projects"]}
    observations = []
    for case in results["cases"]:
        project = projects[case["project"]]
        titles = {m["title"]: m["id"] for m in project["memories"]}
        if len(titles) != len(project["memories"]):
            raise ValueError("Diagnostic title mapping requires unique titles within each project")
        packet = json.loads((run / "cases" / case["project"] / case["id"] / "on.packet.json").read_text())
        actual = case["variants"]["on"]["returned"]
        if len(actual) != len(packet["memories"]):
            raise ValueError("Packet and scored memory list disagree")
        fixed = [mid for mid, memory in zip(actual, packet["memories"])
                 if memory.get("via") != "link"]
        usage = packet["symbol_links"].get("reranker", {})
        if usage.get("cutoff") is not None:
            raise ValueError("Calibration requires a no-cutoff run")
        candidates = []
        for rank, c in enumerate(usage.get("scored_candidates", []), 1):
            if c["rank"] != rank or not math.isfinite(c["score"]):
                raise ValueError("Invalid candidate score or blended rank")
            candidates.append({"memory": titles[c["title"]], "score": c["score"]})
        identities = [c["memory"] for c in candidates]
        if len(set(identities)) != len(identities) or set(fixed) & set(identities):
            raise ValueError("Candidate identities must be unique and additional to ordinary retrieval")
        observation = {"project": case["project"], "query": case["id"],
                       "gold": case["relevant_memories"], "fixed": fixed,
                       "candidates": candidates, "baseline": actual}
        if returned(observation, None) != actual:
            raise ValueError("No-cutoff replay differs from the actual packet; packing may bind")
        observations.append(observation)
    return observations


def outcomes(observations, cutoff):
    rows = []
    for o in observations:
        found, gold = set(returned(o, cutoff)), set(o["gold"])
        rows.append({"project": o["project"], "query": o["query"],
                     "returned": returned(o, cutoff), "gold": o["gold"],
                     "tp": len(found & gold), "fp": len(found - gold),
                     "fn": len(gold - found)})
    return rows


def summary(rows):
    tp, fp = sum(r["tp"] for r in rows), sum(r["fp"] for r in rows)
    positive = [r for r in rows if r["gold"]]
    negative = [r for r in rows if not r["gold"]]
    return {"queries": len(rows), "tp": tp, "fp": fp,
            "precision": tp / (tp + fp) if tp + fp else None,
            "macro_recall": sum(r["tp"] / len(r["gold"]) for r in positive) / len(positive) if positive else None,
            "empty_accuracy": sum(not r["returned"] for r in negative) / len(negative) if negative else None}


def calibrate(observations):
    if not observations or not any(o["gold"] for o in observations):
        raise ValueError("Calibration needs development queries with relevant-memory labels")
    scores = sorted({c["score"] for o in observations for c in o["candidates"]})
    if not all(math.isfinite(s) for s in scores):
        raise ValueError("Candidate scores must be finite")
    thresholds = [None] + scores + ([math.nextafter(scores[-1], math.inf)] if scores else [])
    baseline = outcomes(observations, None)
    before = summary(baseline)
    best, chosen = before, None
    sweep = []
    for threshold in thresholds:
        rows = outcomes(observations, threshold)
        metrics = summary(rows)
        losses = [{"project": r["project"], "query": r["query"], "lost_tp": b["tp"] - r["tp"]}
                  for b, r in zip(baseline, rows) if r["tp"] < b["tp"]]
        sweep.append({"cutoff": threshold, **metrics, "recall_losses": losses})
        key = (metrics["precision"] or 0, metrics["tp"])
        if not losses and key > (best["precision"] or 0, best["tp"]):
            best, chosen = metrics, threshold
    # A cutoff is justified only by a strict precision gain, not by a tie.
    if (best["precision"] or 0) <= (before["precision"] or 0):
        chosen, best = None, before
    return {"cutoff": chosen, "baseline": before, "selected": best,
            "selection_rule": "Maximize precision with no per-query TP loss; tie: more TP, then lower cutoff.",
            "sweep": sweep, "observations": observations,
            "selected_outcomes": outcomes(observations, chosen)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        parser.error("Selection output already exists; do not overwrite a frozen threshold")
    data = json.loads(args.dataset.read_text())
    protocol = json.loads((args.run / "PROTOCOL.json").read_text())
    if protocol["split"] != "development" or protocol["dataset_sha256"] != digest(args.dataset):
        parser.error("Calibration must use the matching development run")
    result = calibrate(load_observations(data, args.run))
    result["protocol"] = {"dataset_sha256": digest(args.dataset),
                          "run_results_sha256": digest(args.run / "RESULTS.json"),
                          "calibrator_sha256": digest(__file__), "run_protocol": protocol}
    save(args.out, result)
    print(json.dumps({k: result[k] for k in ("cutoff", "baseline", "selected")}, indent=2))


if __name__ == "__main__":
    main()
