#!/usr/bin/env python3
"""Run existing synthetic recall safeguards for routing or note-context trials."""
import argparse
import json
from pathlib import Path

from symbol_link_precision import digest, evaluate_project, local_reranker, save, validate

DATASETS = ["symbol_link_precision.json", "symbol_link_exact_scope.json",
            "symbol_link_priority.json", "symbol_link_diversity.json",
            "symbol_link_cutoff_holdout.json"]
VARIANTS = {
    "baseline": {"baseline": {"symbol_links": True, "identifier_route": False},
                 "routed": {"symbol_links": True, "identifier_route": True}},
    "tight": {"tight": {"symbol_links": True, "identifier_route": False}},
    "paired": {"paired": {"symbol_links": True, "identifier_route": False}},
    "named": {"named": {"symbol_links": True, "identifier_route": False}},
}


def summarize(cases, variants):
    result = {}
    for variant in variants:
        values = [c["variants"][variant] for c in cases]
        good = [v for v in values if "error" not in v]
        positives = [c for c in cases if c["relevant_memories"]]
        tp, fp = sum(v["tp"] for v in good), sum(v["fp"] for v in good)
        result[variant] = {
            "planned": len(values), "completed": len(good), "failed": len(values) - len(good),
            "tp": tp, "fp": fp, "precision": tp / (tp + fp) if tp + fp else None,
            "macro_recall": sum(c["variants"][variant].get("recall", 0) for c in positives) / len(positives),
            "mean_tokens": sum(v["tokens"] for v in good) / len(good) if good else None,
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--arm", choices=VARIANTS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--reranker", type=Path, required=True)
    parser.add_argument("--ort", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        parser.error("Output must be empty")
    binary = args.binary.resolve(strict=True)
    local = local_reranker(args.reranker, args.ort, "blend")
    variants = VARIANTS[args.arm]
    datasets = [Path(__file__).resolve().parents[1] / "fixtures" / name for name in DATASETS]
    protocol = {"binary_sha256": digest(binary), "variants": variants, "local": local,
                "datasets": {str(p): digest(p) for p in datasets}, "runner_sha256": digest(__file__),
                "helper_sha256": digest(Path(__file__).with_name("symbol_link_precision.py")),
                "scope": "Synthetic regression data, including prior heldouts; labels never enter requests"}
    save(args.out / "PROTOCOL.json", protocol)
    cases = []
    for path in datasets:
        dataset = json.loads(path.read_text())
        validate(dataset)
        for project in dataset["projects"]:
            rows = evaluate_project(project, binary, args.out / "cases", local, variants)
            for row in rows:
                row["dataset"] = path.name
            cases.extend(rows)
            save(args.out / "RESULTS.json", {"protocol": protocol, "summary": summarize(cases, variants), "cases": cases})
            print(args.arm, project["id"], len(rows), flush=True)
    print(json.dumps(summarize(cases, variants)), flush=True)
    return int(any("error" in v for c in cases for v in c["variants"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
