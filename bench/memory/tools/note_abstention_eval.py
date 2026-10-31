#!/usr/bin/env python3
"""Measure note over-returning and the recall cost of fixed output caps.

Caps are post-hoc diagnostics, not production filters or latency measurements.
Captured evaluation data must identify its capture and independent label owner;
authored negative controls must not be presented as independent real queries.
"""
import argparse
import json
from pathlib import Path

from symbol_link_precision import evaluate_project, local_reranker, validate
from retrieval_recall import digest, save


def provenance(dataset, require_captured=False):
    validate(dataset)
    kind = dataset.get("provenance_kind")
    if kind not in ("authored_control", "captured"):
        raise ValueError("Declare captured data or authored controls")
    if require_captured and kind != "captured":
        raise ValueError("Independent evaluation requires captured queries")
    if kind == "captured":
        for project in dataset["projects"]:
            for q in project["queries"]:
                if not q.get("capture_reference") or not q.get("labelled_by") or q.get("independent_labels") is not True:
                    raise ValueError("Captured queries require capture reference and independently supplied labels")


def curve(cases):
    result = {}
    for limit in range(5):
        tp = fp = fn = negative = empty = 0
        lost, safeguards = [], {}
        for c in cases:
            gold = set(c["relevant_memories"])
            original = c["returned"]
            got = set(original[:limit])
            tp += len(got & gold)
            fp += len(got - gold)
            fn += len(gold - got)
            if not gold:
                negative += 1
                empty += not got
            key = c["project"] + "/" + c["id"]
            if (set(original) & gold) - got:
                lost.append(key)
            if key in ("callee/callee-rationale", "callee-pressure/lease-rationale",
                       "caller-pressure/lease-rationale", "direct-contracts/all-direct"):
                safeguards[key] = gold <= got
        result[str(limit)] = {"tp": tp, "fp": fp, "fn": fn,
                              "precision": tp / (tp + fp) if tp + fp else None,
                              "recall": tp / (tp + fn) if tp + fn else None,
                              "no_answer_cases": negative, "empty_correct": empty,
                              "lost_relevant_cases": lost, "safeguards": safeguards}
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("dataset", "binary", "out", "guard_results", "reranker", "ort"):
        p.add_argument("--" + name.replace("_", "-"), type=Path, required=True)
    p.add_argument("--require-captured", action="store_true")
    args = p.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        p.error("Output must be empty")
    dataset = json.loads(args.dataset.read_text())
    provenance(dataset, args.require_captured)
    local = local_reranker(args.reranker, args.ort, "blend")
    rows = []
    for project in dataset["projects"]:
        rows.extend(evaluate_project(project, args.binary.resolve(), args.out / "cases", local,
                                     {"baseline": {"symbol_links": True, "identifier_route": False}}))
        save(args.out / "RAW.json", rows)
    if any("error" in c["variants"]["baseline"] for c in rows):
        raise ValueError("Incomplete trial; inspect RAW.json")
    guards = json.loads(args.guard_results.read_text())["cases"]
    # Keep newly queried cases separate from the frozen regression guard rows.
    combined = {(c["project"], c["id"]): {**c, "returned": next(iter(c["variants"].values()))["returned"]} for c in guards}
    combined.update({(c["project"], c["id"]): {**c, "returned": c["variants"]["baseline"]["returned"]} for c in rows})
    receipt = {"scope": __doc__, "provenance_kind": dataset["provenance_kind"],
               "dataset_sha256": digest(args.dataset), "binary_sha256": digest(args.binary),
               "guard_results_sha256": digest(args.guard_results), "local": local,
               "new_queries": len(rows), "combined_queries": len(combined),
               "curve": curve(list(combined.values())), "remote_calls": 0,
               "production_filter_changed": False}
    save(args.out / "RESULTS.json", receipt)
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
