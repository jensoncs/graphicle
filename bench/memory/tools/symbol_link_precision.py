#!/usr/bin/env python3
"""Paired evaluation of reviewed-memory retrieval through symbol links.

Run labelled projects through the real CLI in disposable stores. Only symbol_links
changes between off, counted (the default), and on. Labels never enter the store.
Model-free by default; an explicit pinned local reranker is optional. Remote model
calls are forbidden. The corpus is synthetic, not a held-out effectiveness study.
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path, PurePosixPath

from retrieval_recall import digest, save

VARIANTS = {"off": {"symbol_links": False}, "counted": {}, "on": {"symbol_links": True}}
REQUEST = {"max_tokens": 3000, "max_chars": 12000, "limit": 8,
           "graph_retrieval": True, "memory_retrieval": True,
           "doc_links": False, "cross_repo": False, "identifier_route": False,
           "notes_first": False}
DEFAULT_DATASET = Path(__file__).resolve().parents[1] / "fixtures/symbol_link_precision.json"


def validate(dataset):
    """Closed-world memory labels; reject ambiguous IDs and unsafe fixture paths."""
    if dataset.get("format") != "graphicle.symbol-link-eval.v1":
        raise ValueError("Unsupported dataset format")
    if not dataset.get("provenance") or not dataset.get("projects"):
        raise ValueError("Dataset needs provenance and projects")
    seen = set()
    for project in dataset["projects"]:
        pid = project["id"]
        if not re.fullmatch(r"[a-zA-Z0-9_-]+", pid) or pid in seen:
            raise ValueError("Project IDs must be unique safe names")
        seen.add(pid)
        if project.get("split") not in ("regression", "development", "held_out"):
            raise ValueError("Each project needs an explicit split")
        if not project.get("files") or not project.get("queries"):
            raise ValueError("Each project needs files and queries")
        for field in ("files", "updates"):
            for name, content in project.get(field, {}).items():
                path = PurePosixPath(name)
                if (not name or path.is_absolute() or ".." in path.parts or "\\" in name
                        or ":" in name or any(p.startswith(".") or p == "secrets" for p in path.parts)):
                    raise ValueError(f"Unsafe fixture path: {name}")
                if not isinstance(content, str):
                    raise ValueError("Fixture contents must be strings")
        memories = project.get("memories", [])
        mids = [m["id"] for m in memories]
        if len(set(mids)) != len(mids) or any(not isinstance(m, str) or not m for m in mids):
            raise ValueError("Memory IDs must be unique nonempty strings")
        for memory in memories:
            if not memory.get("title") or not memory.get("body"):
                raise ValueError("Memories need title and body")
            for symbol in memory.get("symbols", []):
                if symbol.get("path") not in project["files"] or not symbol.get("qualified_name"):
                    raise ValueError("Symbols must name a fixture file and declaration")
        qids = set()
        for query in project["queries"]:
            qid = query["id"]
            if not re.fullmatch(r"[a-zA-Z0-9_-]+", qid) or qid in qids:
                raise ValueError("Query IDs must be unique safe names within a project")
            qids.add(qid)
            gold = query["relevant_memories"]
            if (not isinstance(gold, list) or len(set(gold)) != len(gold)
                    or not set(gold) <= set(mids)):
                raise ValueError("Relevant memories must be unique known IDs")
            if not query.get("question") or not query.get("rationale") or not query.get("category"):
                raise ValueError("Queries need question, category, and label rationale")


def score(packet, known, relevant, allow_local=False):
    """ID-level precision, not proof that a truncated memory answers the question."""
    if not isinstance(packet.get("memories"), list) or not isinstance(packet.get("evidence"), list):
        raise ValueError("Not a context packet: memories and evidence are required")
    tokens = packet.get("tokens")
    if type(tokens) is not int or tokens < 0 or tokens > REQUEST["max_tokens"]:
        raise ValueError("Missing or invalid packet token count")
    if "markdown" in packet and (not isinstance(packet["markdown"], str)
                                 or len(packet["markdown"]) > REQUEST["max_chars"]):
        raise ValueError("Invalid or over-budget packet text")
    calls = packet.get("model_calls", 0)
    if not isinstance(calls, int) or isinstance(calls, bool) or calls < 0:
        raise ValueError("Invalid model call count")
    if packet.get("remote_model_calls", 0) != 0:
        raise ValueError("Remote model calls are forbidden")
    if not allow_local and calls != 0:
        raise ValueError("This evaluation must not call a model")
    local_usage = packet.get("local_reranker") or packet.get("symbol_links", {}).get("reranker")
    if allow_local and calls and not isinstance(local_usage, dict):
        raise ValueError("Model calls need local reranker accounting")
    returned = [known[m["id"]] for m in packet["memories"]]
    if len(set(returned)) != len(returned):
        raise ValueError("Packet contains duplicate memories")
    gold, found = set(relevant), set(returned)
    tp = len(found & gold)
    return {"returned": returned, "tp": tp, "fp": len(found - gold), "fn": len(gold - found),
            "precision": tp / len(found) if found else None,
            "recall": tp / len(gold) if gold else None,
            "empty_correct": not found if not gold else None,
            "tokens": tokens, "source_excerpts": len(packet["evidence"]),
            "symbol_links": packet.get("symbol_links"), "model_calls": calls,
            "local_reranker": packet.get("local_reranker")}


def summarize(cases):
    table = {}
    for variant in VARIANTS:
        items = [c["variants"][variant] for c in cases if "error" not in c["variants"][variant]]
        positives = [c for c in cases if c["relevant_memories"]]
        negatives = [c for c in cases if not c["relevant_memories"]]
        tp, fp = sum(i["tp"] for i in items), sum(i["fp"] for i in items)
        table[variant] = {
            "planned": len(cases), "completed": len(items), "failed": len(cases) - len(items),
            "answerable_queries": len(positives), "empty_memory_queries": len(negatives),
            "micro_precision_completed": tp / (tp + fp) if tp + fp else None,
            "macro_recall": sum(c["variants"][variant].get("recall", 0) for c in positives) / len(positives) if positives else None,
            "empty_memory_accuracy": sum(c["variants"][variant].get("empty_correct", False) for c in negatives) / len(negatives) if negatives else None,
            "false_positive_memories": fp,
            "mean_tokens_completed": statistics.mean(i["tokens"] for i in items) if items else None,
        }
    paired = [c for c in cases if all("error" not in c["variants"][v] for v in VARIANTS)]
    deltas = {}
    for metric in ("tp", "fp", "tokens"):
        values = [c["variants"]["on"][metric] - c["variants"]["off"][metric] for c in paired]
        deltas["mean_" + metric + "_delta"] = statistics.mean(values) if values else None
    return {"variants": table, "on_minus_off": {"paired": len(paired), "excluded": len(cases) - len(paired), **deltas}}


def report(cases):
    return {"summary": summarize(cases),
            "by_category": {category: summarize([c for c in cases if c["category"] == category])
                            for category in sorted({c["category"] for c in cases})},
            "counted_content_mismatches": sum(c.get("counted_preserves_content") is False for c in cases)}


def packet_content(packet):
    """Counted must preserve delivered evidence, excluding diagnostic counters."""
    return {key: packet[key] for key in ("memories", "evidence", "markdown", "tokens")}


def evaluate_project(project, binary, output, local=None, variants=None, diagnose=False):
    variants = VARIANTS if variants is None else variants
    cases = [{"project": project["id"], **q, "variants": {}} for q in project["queries"]]
    with tempfile.TemporaryDirectory(prefix="graphicle-links-") as tmp:
        work = Path(tmp)
        folder = work / "project"
        folder.mkdir()
        config = work / "models.json"
        save(config, {"models": {"reranker": local["profile"]} if local else {}})
        # Keep system execution settings, never provider credentials or Graphicle overrides.
        env = {k: v for k, v in os.environ.items() if k in
               ("PATH", "SystemRoot", "ComSpec", "PATHEXT", "TMPDIR", "TEMP", "TMP")}
        env["GRAPHICLE_NO_UPDATE_NOTIFIER"] = "1"
        if diagnose:
            env["GRAPHICLE_PHASE_TIMINGS"] = "1"
        if local:
            env["ORT_DYLIB_PATH"] = local["ort"]

        last_stderr = ""

        def run(*args):
            nonlocal last_stderr
            proc = subprocess.run([str(binary), "--data-dir", str(work / "store"),
                                   "--config", str(config), *map(str, args)], cwd=folder,
                                  env=env, text=True, capture_output=True, timeout=120)
            last_stderr = proc.stderr
            if proc.returncode:
                raise RuntimeError(proc.stderr[-1500:] or proc.stdout[-1500:])
            return json.loads(proc.stdout)

        def call(operation, args):
            argument_file = work / "arguments.json"
            save(argument_file, args)
            return run("call", operation, "--project", folder, "--arguments", argument_file)

        def sync(files):
            for name, text in files.items():
                target = folder / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(text, encoding="utf-8")
            run("sync", folder)

        try:
            sync(project["files"])
            known = {}
            for memory in project.get("memories", []):
                args = {k: memory[k] for k in ("title", "body", "symbols") if k in memory}
                saved = call("note", {"kind": "decision", **args})
                known[saved["id"]] = memory["id"]
            if project.get("updates"):
                sync(project["updates"])
        except Exception as error:
            for case in cases:
                case["variants"] = {v: {"error": f"setup: {error}"} for v in variants}
            return cases

        for index, case in enumerate(cases):
            packets = {}
            # Rotate execution order to avoid systematically favouring a warm variant.
            order = list(variants)
            order = order[index % len(order):] + order[:index % len(order)]
            for variant in order:
                request = {**REQUEST, **variants[variant], "question": case["question"]}
                if local:
                    request.update(local["request"])
                destination = output / project["id"] / case["id"]
                save(destination / f"{variant}.request.json", request)
                started = time.monotonic()
                try:
                    packet = call("context", request)
                    elapsed = time.monotonic() - started
                    save(destination / f"{variant}.packet.json", packet)
                    case["variants"][variant] = {**score(packet, known, case["relevant_memories"], bool(local)),
                                                  "seconds": round(elapsed, 4)}
                    packets[variant] = packet
                    if diagnose:
                        from quality_diagnostics import diagnostics
                        save(destination / f"{variant}.diagnostics.json", diagnostics(packet, last_stderr))
                except Exception as error:
                    case["variants"][variant] = {"error": str(error),
                                                  "seconds": time.monotonic() - started}
            if "off" in packets and "counted" in packets:
                case["counted_preserves_content"] = packet_content(packets["off"]) == packet_content(packets["counted"])
    return cases


def local_reranker(directory, ort, mode):
    """Freeze and verify assets before any queries; no implicit runtime downloads."""
    directory, ort = directory.resolve(strict=True), ort.resolve(strict=True)
    manifest_path = directory / "MODEL.json"
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("pooling") != "logits":
        raise ValueError("A cross-encoder manifest with logits pooling is required")
    assets = {str(manifest_path): digest(manifest_path), str(ort): digest(ort)}
    for name in ("model.onnx", "tokenizer.json"):
        actual = digest(directory / name)
        if actual != manifest["files"][name]["sha256"]:
            raise ValueError(f"Model digest mismatch: {name}")
        assets[str(directory / name)] = actual
    return {"profile": {"protocol": "local_onnx", "base_url": directory.as_uri() + "/",
                        "model": manifest["model"], "input_limit": 1000},
            "ort": str(ort), "assets": assets,
            "request": {"local_reranker_profile": "reranker", "rerank_mode": mode}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--dataset", type=Path, default=DEFAULT_DATASET)
    parser.add_argument("--split", choices=("regression", "development", "held_out"), default="regression")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--reranker", type=Path, help="Pinned local cross-encoder directory")
    parser.add_argument("--ort", type=Path, help="ONNX Runtime library; required with --reranker")
    parser.add_argument("--rerank-mode", choices=("blend", "replace", "rrf"), default="blend")
    args = parser.parse_args()
    if bool(args.reranker) != bool(args.ort):
        parser.error("--reranker and --ort must be supplied together")
    local = local_reranker(args.reranker, args.ort, args.rerank_mode) if args.reranker else None
    dataset = json.loads(args.dataset.read_text())
    validate(dataset)
    projects = [p for p in dataset["projects"] if p["split"] == args.split]
    if not projects:
        parser.error("No projects in selected split")
    output = args.out.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("Output directory must be empty; use a new directory for each frozen run")
    binary = args.binary.resolve(strict=True)
    # Freeze before querying. This runner does not tune thresholds or change defaults.
    protocol = {"format": dataset["format"], "split": args.split, "provenance": dataset["provenance"],
                "dataset_sha256": digest(args.dataset), "binary_sha256": digest(binary),
                "runner_sha256": digest(__file__), "helpers_sha256": digest(Path(__file__).with_name("retrieval_recall.py")),
                "request": {**REQUEST, **(local["request"] if local else {})}, "variants": VARIANTS,
                "projects": [p["id"] for p in projects], "model_calls": "local_only" if local else 0,
                "local_reranker": local,
                "metric_scope": "Reviewed-memory IDs; empty memory is not empty code context or answer abstention"}
    save(output / "PROTOCOL.json", protocol)
    cases = []
    for project in projects:
        cases.extend(evaluate_project(project, binary, output / "cases", local))
        save(output / "RESULTS.json", {"protocol": protocol, **report(cases), "cases": cases})
    failures = sum("error" in v for c in cases for v in c["variants"].values())
    mismatches = sum(c.get("counted_preserves_content") is False for c in cases)
    print(json.dumps(report(cases), indent=2))
    return 1 if failures or mismatches else 0


if __name__ == "__main__":
    raise SystemExit(main())
