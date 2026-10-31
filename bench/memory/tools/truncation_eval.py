#!/usr/bin/env python3
"""Controlled local reranker ablation on frozen, already-delivered passages.

This measures model input visibility and conditional ranking, not retrieval or
answer quality. Labels never enter the Rust scorer's input. No remote calls.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

from retrieval_recall import digest, save


def scored_visibility(packet, query, files, repositories):
    """Measure actual scored inputs, never substitute delivered excerpts.

    Validate byte/hash provenance before comparing evaluator-only labels.
    Tokenizer gaps count as visible only when the source gap is whitespace.
    """
    inverse = {rid: project for project, rid in repositories.items()}
    reports = ([(query["projects"][0], packet)] if len(query["projects"]) == 1 else
               [(inverse.get(p.get("repository_id")), p) for p in packet.get("projects", [])])
    inputs = []
    for project, report in reports:
        if project not in query["projects"]:
            raise ValueError("Reranker diagnostics from an unauthorized project")
        for item in report.get("reranker_inputs_debug", []):
            if not isinstance(item.get("id"), str) or not item["id"].startswith("file:"):
                continue  # Memory candidates are not original source evidence.
            path = item["id"][5:]
            raw = files[project][path]
            a, b = item["byte_start"], item["byte_end"]
            if not isinstance(a, int) or not isinstance(b, int) or not 0 <= a < b <= len(raw):
                raise ValueError("Invalid reranker source range")
            if hashlib.sha256(raw[a:b]).hexdigest() != item["passage_sha256"]:
                raise ValueError("Reranker input differs from pinned source")
            ranges = item["visible_token_byte_ranges"]
            if any(not 0 <= x < y <= b - a for x, y in ranges):
                raise ValueError("Invalid tokenizer range")
            inputs.append((project, path, a, b, ranges))
    visible_required = []
    for gold in query["required"]:
        found = False
        for project, path, a, b, ranges in inputs:
            start, end = gold["byte_start"], gold["byte_end"]
            if (project, path) != (gold["project"], gold["file"]) or not a <= start < end <= b:
                continue
            remaining = bytearray(files[project][path][start:end])
            for x, y in ranges:
                lo, hi = max(start, a + x), min(end, a + y)
                if lo < hi:
                    remaining[lo - start:hi - start] = b" " * (hi - lo)
            found |= not remaining.strip()
        visible_required.append(found)
    return {"required_visible": sum(visible_required), "required_total": len(visible_required),
            "visible": visible_required, "scored_source_candidates": len(inputs),
            "basis": "actual_tokenizer_input_offsets"}


def visible(run, start, end):
    spans = run["visible"]
    return bool(spans) and min(a for a, _ in spans) <= start and end <= max(b for _, b in spans)


def summarize(cases):
    out = {}
    for arm in ("full", "window"):
        rows = [c["arms"][arm] for c in cases]
        out[arm] = {"queries": len(rows),
                    "required_visible": sum(r["required_visible"] for r in rows),
                    "required_total": sum(r["required_total"] for r in rows),
                    "inferences": sum(r["inferences"] for r in rows),
                    "tokens": sum(r["tokens"] for r in rows),
                    "seconds": sum(r["seconds"] for r in rows),
                    "supporting_at_3": sum(r["supporting_at_3"] for r in rows),
                    "returned_at_3": sum(r["returned_at_3"] for r in rows)}
    return out


def evaluate(query, evidence, result):
    arms = {}
    for arm in ("full", "window"):
        chosen, calls = [], []
        for runs in result["results"]:
            candidates = [r for r in runs if r["kind"] == arm]
            calls.extend(candidates)
            chosen.append(max(candidates, key=lambda r: r["score"]))
        delivered = []
        relevant = [False] * len(evidence)
        for gold in query["required"]:
            found = False
            for i, (e, run) in enumerate(zip(evidence, chosen)):
                if e["repository_id"] != gold["repository_id"] or e["id"] != "file:" + gold["file"]:
                    continue
                a, b = gold["byte_start"] - e["byte_start"], gold["byte_end"] - e["byte_start"]
                raw = e["text"].encode()
                if not (0 <= a < b <= len(raw)):
                    continue
                relevant[i] = True
                # Tokenizers omit whitespace; do not score that as lost code.
                piece = raw[a:b]
                a += len(piece) - len(piece.lstrip())
                b -= len(piece) - len(piece.rstrip())
                found |= visible(run, a, b)
            delivered.append(found)
        order = sorted(range(len(chosen)), key=lambda i: chosen[i]["score"], reverse=True)[:3]
        arms[arm] = {"required_visible": sum(delivered), "required_total": len(delivered),
                     "delivered": delivered, "supporting_at_3": sum(relevant[i] for i in order),
                     "returned_at_3": len(order), "order": order,
                     "inferences": len(calls), "tokens": sum(r["tokens"] for r in calls),
                     "seconds": sum(r["seconds"] for r in calls)}
    return {"query": query["id"], "arms": arms}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("scorer", "dataset", "packets", "model", "ort", "out"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        parser.error("Output must be empty")
    dataset = json.loads(args.dataset.read_text())
    protocol = json.loads((args.packets / "PROTOCOL.json").read_text())
    profile = protocol["local"]["profile"]
    if profile["protocol"] != "local_onnx":
        raise ValueError("Only pinned local inference is allowed")
    profile["base_url"] = args.model.resolve().as_uri() + "/"
    inputs, rows = [], []
    for q in dataset["queries"]:
        variant = "cross_on" if len(q["projects"]) > 1 else "single"
        packet_path = args.packets / "cases" / q["id"] / (variant + ".packet.json")
        packet = json.loads(packet_path.read_text())
        evidence = packet["evidence"]
        if len(q["projects"]) == 1:
            evidence = [{**e, "repository_id": protocol["repositories"][q["projects"][0]]} for e in evidence]
        query = {**q, "required": [{**g, "repository_id": protocol["repositories"][g["project"]]} for g in q["required"]]}
        inputs.append({"profile": profile, "question": q["question"], "passages": [e["text"] for e in evidence]})
        rows.append((query, evidence, digest(packet_path)))
    save(args.out / "INPUT.json", inputs)
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "TMPDIR", "TMP", "TEMP")}
    env["ORT_DYLIB_PATH"] = str(args.ort.resolve())
    proc = subprocess.run([str(args.scorer.resolve())], input="".join(json.dumps(x) + "\n" for x in inputs),
                          capture_output=True, text=True, env=env, timeout=1800)
    (args.out / "scorer.stderr").write_text(proc.stderr)
    if proc.returncode:
        raise RuntimeError(proc.stderr[-1600:])
    results = [json.loads(line) for line in proc.stdout.splitlines()]
    if len(results) != len(rows):
        raise ValueError("Missing scorer results")
    save(args.out / "SCORES.json", results)
    cases = [evaluate(q, e, result) for (q, e, _), result in zip(rows, results)]
    receipt = {"scope": __doc__, "scorer_sha256": digest(args.scorer),
               "dataset_sha256": digest(args.dataset), "runner_sha256": digest(__file__),
               "model_sha256": digest(args.model / "MODEL.json"), "ort_sha256": digest(args.ort),
               "packet_hashes": {q["id"]: h for q, _, h in rows}, "remote_calls": 0,
               "summary": summarize(cases), "cases": cases}
    save(args.out / "RESULTS.json", receipt)
    print(json.dumps(receipt["summary"], indent=2))


if __name__ == "__main__":
    main()
