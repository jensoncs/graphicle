#!/usr/bin/env python3
"""Replay provenance-labelled questions over hash-pinned real source slices.

This measures source delivery, not answer correctness. Expected spans never enter
requests. Location questions accept definition citations; behavior questions need
the actual source bytes. No remote inference or source downloads are performed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tempfile
import time

from retrieval_recall import digest, save
from symbol_link_precision import REQUEST, local_reranker


def safe_path(name):
    path = PurePosixPath(name)
    if (not name or path.is_absolute() or ".." in path.parts or "\\" in name
            or ":" in name or any(p.startswith(".") or p == "secrets" for p in path.parts)):
        raise ValueError("Unsafe source path")
    return path


def load_sources(project):
    root = Path(project["source_root"]).resolve(strict=True)
    files = {}
    for name, sha in project["files"].items():
        path = root / safe_path(name)
        if not path.resolve(strict=True).is_relative_to(root):
            raise ValueError("Source escaped root")
        raw = path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != sha:
            raise ValueError(f"Source hash changed: {name}")
        raw.decode("utf-8")
        files[name] = raw
    return files


def validate_questions(project, files):
    seen = set()
    for q in project["queries"]:
        safe_path(q["id"])
        if q["id"] in seen or "/" in q["id"]:
            raise ValueError("Duplicate or nested query ID")
        seen.add(q["id"])
        if q["origin"]["kind"] not in ("captured_tool_query", "authored_real_case"):
            raise ValueError("Explicit query provenance required")
        if q["intent"] not in ("location", "behavior", "rationale") or not q["required"]:
            raise ValueError("Intent and nonempty evidence labels required")
        for span in q["required"]:
            raw = files[span["file"]]
            a, b = span["byte_start"], span["byte_end"]
            if not (0 <= a < b <= len(raw)) or raw[a:b].decode() != span["quote"]:
                raise ValueError("Evidence label does not match pinned source")
            quote = span["quote"].encode()
            if raw.count(quote) > 1:
                anchor = (span.get("after") or "").encode()
                if (not anchor or raw.count(anchor) != 1
                        or raw.index(quote, raw.index(anchor)) != a):
                    raise ValueError("Repeated evidence needs an unambiguous declaration anchor")


def score(packet, query, files, max_tokens=3000):
    if packet.get("remote_model_calls", 0) != 0:
        raise ValueError("Remote inference forbidden")
    if not isinstance(packet.get("evidence"), list) or not 0 <= packet["tokens"] <= max_tokens:
        raise ValueError("Invalid or over-budget packet")
    excerpts = []
    for e in packet["evidence"]:
        name = e["id"].removeprefix("file:")
        raw = files[name]
        a, b = e["byte_start"], e["byte_end"]
        if (hashlib.sha256(raw).hexdigest() != e["sha256"] or not 0 <= a < b <= len(raw)
                or raw[a:b].decode() != e["text"]):
            raise ValueError("Delivered excerpt is not pinned source")
        excerpts.append((name, a, b))
    definitions = [d for s in packet.get("symbols", []) for d in s["definitions"]]
    delivered = []
    for gold in query["required"]:
        # A definition citation is enough only for a location task. It cannot
        # stand in for a callee return, branch, or explanatory comment.
        cited = query["intent"] == "location" and any(
            d["relpath"] == gold["file"] and d["line"] == gold["line"] for d in definitions)
        # Ordinary retrieval also carries definition citations in code_graph,
        # even when packing clips the accompanying source text. The enclosing
        # evidence's file hash was verified above. Require its exact declaration
        # start, file and line; never let this satisfy a behavior question.
        if query["intent"] == "location":
            cited = cited or any(
                e["id"] == "file:" + gold["file"]
                and (e.get("code_graph") or {}).get("relpath") == gold["file"]
                and e["code_graph"].get("line") == gold["line"]
                and e["code_graph"].get("byte_start") == gold["byte_start"]
                and isinstance(e["code_graph"].get("byte_end"), int)
                and gold["byte_end"] <= e["code_graph"]["byte_end"] <= len(files[gold["file"]])
                for e in packet["evidence"])
        spans = sorted((a, b) for name, a, b in excerpts if name == gold["file"])
        end = gold["byte_start"]
        for a, b in spans:
            if a <= end:
                end = max(end, b)
        delivered.append(cited or end >= gold["byte_end"])
    relevant = sum(any(name == g["file"] and a < g["byte_end"] and g["byte_start"] < b
                       for g in query["required"]) for name, a, b in excerpts)
    return {"complete": all(delivered), "required_delivered": delivered,
            "evidence_recall": sum(delivered) / len(delivered),
            "supporting_excerpts": relevant, "excerpts": len(excerpts),
            "supporting_excerpt_precision": relevant / len(excerpts) if excerpts else None,
            "tokens": packet["tokens"], "route": packet.get("route"),
            "model_calls": packet.get("model_calls", 0), "source_verified": True}


def run_project(project, binary, output, local):
    files = load_sources(project)
    validate_questions(project, files)
    results = []
    with tempfile.TemporaryDirectory(prefix="graphicle-real-query-") as tmp:
        tmp = Path(tmp)
        source = tmp / "project"
        source.mkdir()
        for name, raw in files.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(raw)
        config = tmp / "models.json"
        save(config, {"models": {"reranker": local["profile"]} if local else {}})
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "TMPDIR", "TEMP", "TMP")}
        env["GRAPHICLE_NO_UPDATE_NOTIFIER"] = "1"
        if local:
            env["ORT_DYLIB_PATH"] = local["ort"]

        def call(*args):
            proc = subprocess.run([str(binary), "--data-dir", str(tmp / "store"),
                                   "--config", str(config), *map(str, args)], cwd=source,
                                  env=env, capture_output=True, text=True, timeout=240)
            if proc.returncode:
                raise RuntimeError(proc.stderr[-1500:] or proc.stdout[-1500:])
            return json.loads(proc.stdout)

        call("sync", source)
        for index, query in enumerate(project["queries"]):
            row = {"project": project["id"], "query": query["id"], "intent": query["intent"],
                   "origin": query["origin"]["kind"], "split": project["split"], "variants": {}}
            # One existing switch, same eligible source and budget. No classifier
            # labels are supplied to Graphicle.
            order = [False, True] if index % 2 == 0 else [True, False]
            for routed in order:
                variant = "routed" if routed else "baseline"
                args = {**REQUEST, "symbol_links": True, "identifier_route": routed,
                        "question": query["question"], **(local["request"] if local else {})}
                path = output / project["id"] / query["id"] / variant
                save(path.with_suffix(".request.json"), args)
                request = tmp / "arguments.json"
                save(request, args)
                try:
                    start = time.monotonic()
                    packet = call("call", "context", "--project", source, "--arguments", request)
                    elapsed = time.monotonic() - start
                    save(path.with_suffix(".packet.json"), packet)
                    row["variants"][variant] = {**score(packet, query, files), "seconds": elapsed}
                except Exception as error:
                    row["variants"][variant] = {"error": str(error), "complete": False}
            results.append(row)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--reranker", type=Path)
    parser.add_argument("--ort", type=Path)
    parser.add_argument("--source-root", action="append", default=[], metavar="PROJECT=DIR",
                        help="Relocate a historical checkout; pinned file hashes must still match")
    args = parser.parse_args()
    if bool(args.reranker) != bool(args.ort):
        parser.error("Both reranker and runtime are required")
    if args.out.exists() and any(args.out.iterdir()):
        parser.error("Output must be empty")
    dataset = json.loads(args.dataset.read_text())
    projects = {p["id"]: p for p in dataset["projects"]}
    for override in args.source_root:
        name, separator, root = override.partition("=")
        if not separator or name not in projects or not root:
            parser.error("Source override must name a dataset project and directory")
        projects[name]["source_root"] = root
    binary = args.binary.resolve(strict=True)
    local = local_reranker(args.reranker, args.ort, "blend") if args.reranker else None
    for project in dataset["projects"]:
        safe_path(project["id"])
        validate_questions(project, load_sources(project))
    protocol = {"dataset_sha256": digest(args.dataset), "binary_sha256": digest(binary),
                "runner_sha256": digest(__file__), "local": local,
                "source_roots": {p["id"]: p["source_root"] for p in dataset["projects"]},
                "scope": "Pinned source slices; not full-repository retrieval or answer quality"}
    save(args.out / "PROTOCOL.json", protocol)
    results = []
    for project in dataset["projects"]:
        results.extend(run_project(project, binary, args.out / "cases", local))
        save(args.out / "RESULTS.json", {"protocol": protocol, "cases": results})
    print(json.dumps({v: {"complete": sum(c["variants"][v]["complete"] for c in results),
                         "queries": len(results)} for v in ("baseline", "routed")}))
    return int(any("error" in v for c in results for v in c["variants"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
