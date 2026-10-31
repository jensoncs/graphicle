#!/usr/bin/env python3
"""Evaluate source completeness in pinned Atlas/Nexus trees and workspace scope.

Prepare once, then reuse the same --prepared-root for matched binary comparisons.
Labels are used only by the evaluator, never sent to Graphicle. No answer model.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from full_repository_eval import indexed_sources, materialize
from real_query_eval import safe_path, score, validate_questions
from symbol_link_precision import REQUEST, digest, local_reranker, save
from truncation_eval import scored_visibility


def make_request(query, local, cross, project_count, defaults=False):
    workspace = len(query["projects"]) > 1
    settings = {key: REQUEST[key] for key in ("max_tokens", "max_chars")} if defaults else {
        **REQUEST, "symbol_links": True, "cross_repo": cross}
    request = {**settings, **local["request"], "question": query["question"],
               "scope": "workspace" if workspace else "project"}
    if workspace and not defaults:
        request["max_projects"] = project_count
    return request


def score_scoped(packet, query, files, repositories):
    """Require project-qualified evidence; validate even non-gold excerpts."""
    if packet.get("remote_model_calls", 0) != 0:
        raise ValueError("Remote inference forbidden")
    if not isinstance(packet.get("evidence"), list):
        raise ValueError("Missing evidence")
    if len(packet.get("markdown", "")) > REQUEST["max_chars"]:
        raise ValueError("Character budget exceeded")
    inverse = {rid: pid for pid, rid in repositories.items()}
    if len(inverse) != len(repositories):
        raise ValueError("Repository identities must be unique")
    per_project = {pid: [] for pid in files}
    workspace = len(query["projects"]) > 1
    for evidence in packet["evidence"]:
        if workspace:
            pid = inverse.get(evidence.get("repository_id"))
        else:
            pid = query["projects"][0]
            if evidence.get("repository_id", repositories[pid]) != repositories[pid]:
                raise ValueError("Wrong project in single-project evidence")
        if pid not in query["projects"]:
            raise ValueError("Evidence from an unknown or out-of-scope repository")
        per_project[pid].append(evidence)
    results = {}
    for pid in query["projects"]:
        required = [{k: v for k, v in gold.items() if k != "project"}
                    for gold in query["required"] if gold["project"] == pid]
        if not required:
            raise ValueError("Every requested project needs evidence labels")
        # Shared scorer checks hashes and exact source bytes. All cases here
        # require behavior bytes, so symbols/definition metadata cannot pass.
        part = {**packet, "evidence": per_project[pid], "symbols": []}
        results[pid] = score(part, {**query, "required": required}, files[pid])
    return {"complete": all(r["complete"] for r in results.values()),
            "per_project": results,
            "required_delivered": sum(sum(r["required_delivered"]) for r in results.values()),
            "required_total": len(query["required"]),
            "tokens": packet["tokens"],
            "excerpts": sum(r["excerpts"] for r in results.values()),
            "supporting_excerpts": sum(r["supporting_excerpts"] for r in results.values()),
            "workspace": packet.get("workspace"), "omitted": packet.get("omitted"),
            "model_calls": packet.get("model_calls", 0)}


def validate(dataset, files):
    ids = [p["id"] for p in dataset["projects"]]
    if len(set(ids)) != len(ids) or set(ids) != set(files):
        raise ValueError("Invalid project set")
    seen = set()
    for query in dataset["queries"]:
        if query["id"] in seen or len(safe_path(query["id"]).parts) != 1:
            raise ValueError("Duplicate or unsafe query ID")
        seen.add(query["id"])
        if (not query["projects"] or len(set(query["projects"])) != len(query["projects"])
                or not set(query["projects"]) <= set(ids) or query["intent"] != "behavior"):
            raise ValueError("Invalid query scope or intent")
        if set(g["project"] for g in query["required"]) != set(query["projects"]):
            raise ValueError("Each query project needs required evidence")
        for pid in query["projects"]:
            required = [{k: v for k, v in g.items() if k != "project"}
                        for g in query["required"] if g["project"] == pid]
            validate_questions({"queries": [{**query, "required": required}]}, files[pid])


def summarize(cases):
    result = {}
    for scope in ("single", "workspace"):
        selected = [c for c in cases if c["scope"] == scope]
        result[scope] = {}
        for variant in sorted({v for c in selected for v in c["variants"]}):
            rows = [c["variants"][variant] for c in selected]
            result[scope][variant] = {
                "queries": len(rows), "complete": sum(r.get("complete", False) for r in rows),
                "errors": sum("error" in r for r in rows),
                "required_delivered": sum(r.get("required_delivered", 0) for r in rows),
                "required_total": sum(c["required_total"] for c in selected),
                "tokens": sum(r.get("tokens", 0) for r in rows),
                "supporting_excerpts": sum(r.get("supporting_excerpts", 0) for r in rows),
                "excerpts": sum(r.get("excerpts", 0) for r in rows)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("dataset", "binary", "out", "prepared-root", "reranker", "ort"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--routes", action="store_true", help="Enable the existing opt-in HTTP extractor when preparing graphs")
    parser.add_argument("--defaults", action="store_true",
                        help="Use retrieval defaults with the same explicit local model and packet budget")
    parser.add_argument("--diagnose-reranker", action="store_true",
                        help="Validate actual scored source ranges; labels remain evaluator-only")
    parser.add_argument("--repos-dir", type=Path)
    args = parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        parser.error("Output must be empty")
    if args.prepare and not args.repos_dir:
        parser.error("--prepare requires --repos-dir")
    dataset = json.loads(args.dataset.read_text())
    if dataset["format"] != "graphicle.multi-repo-eval.v1":
        parser.error("Unsupported format")
    binary, prepared = args.binary.resolve(strict=True), args.prepared_root.resolve()
    local = local_reranker(args.reranker, args.ort, "blend")
    config = args.out / "models.json"
    save(config, {"models": {"reranker": local["profile"]}})
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "TMPDIR", "TMP", "TEMP", "RUST_MIN_STACK", "GRAPHICLE_PHASE_TIMINGS")}
    env.update(GRAPHICLE_NO_UPDATE_NOTIFIER="1", ORT_DYLIB_PATH=local["ort"])
    if args.routes:
        env["GRAPHICLE_ROUTES"] = "1"

    call_index = 0

    def call(root, *command, timeout=300):
        nonlocal call_index
        call_index += 1
        log = args.out / "calls" / f"{call_index:03d}.stderr.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        with log.open("w") as stderr:
            proc = subprocess.run([str(binary), "--data-dir", str(prepared / "store"),
                                   "--config", str(config.resolve()), *map(str, command)],
                                  cwd=root, env=env, stdout=subprocess.PIPE, stderr=stderr,
                                  text=True, timeout=timeout)
        if proc.returncode:
            raise RuntimeError(log.read_text()[-1800:] or proc.stdout[-1800:])
        return json.loads(proc.stdout)

    files, repositories, roots, inventories = {}, {}, {}, {}
    for project in dataset["projects"]:
        pid = project["id"]
        if len(safe_path(pid).parts) != 1 or len(safe_path(project["repo"]).parts) != 1:
            parser.error("Unsafe project name")
        folder = prepared / pid
        root = folder / "checkout"
        roots[pid] = root
        if args.prepare:
            folder.mkdir(parents=True)
            save(folder / "SOURCE-MANIFEST.json", materialize(args.repos_dir / project["repo"], project["commit"], root))
            save(folder / "SYNC.json", call(root, "sync", root, timeout=2400))
        manifest = json.loads((folder / "SOURCE-MANIFEST.json").read_text())
        synced = json.loads((folder / "SYNC.json").read_text())
        if manifest["commit"] != project["commit"]:
            raise ValueError("Wrong prepared commit")
        repositories[pid] = synced["repository_id"]
        files[pid] = indexed_sources(prepared / "store", repositories[pid])
        for name, sha in project["files"].items():
            if (manifest["files"][name]["sha256"] != sha
                    or hashlib.sha256(files[pid][name]).hexdigest() != sha):
                raise ValueError("Gold source changed")
        index = {name: {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)}
                 for name, raw in files[pid].items()}
        save(args.out / pid / "INDEX.json", index)
        inventories[pid] = {"commit": project["commit"], "indexed_files": len(index),
                            "manifest_sha256": digest(folder / "SOURCE-MANIFEST.json"),
                            "sync_sha256": digest(folder / "SYNC.json"),
                            "index_sha256": digest(args.out / pid / "INDEX.json")}
    validate(dataset, files)
    # Sync normally links the workspace; force an explicit receipt so an empty
    # cross-repository result cannot be explained by a never-run link job.
    link = call(roots[dataset["projects"][0]["id"]], "call", "graph.link", timeout=600)
    save(args.out / "LINK.json", link)
    protocol = {"dataset_sha256": digest(args.dataset), "binary_sha256": digest(binary),
                "runner_sha256": digest(__file__), "scorer_sha256": digest(Path(__file__).with_name("real_query_eval.py")),
                "local": local, "inventory": inventories, "repositories": repositories,
                "link_sha256": digest(args.out / "LINK.json"), "retrieval_defaults": args.defaults, "routes_enabled": args.routes, "worker_stack_bytes": env.get("RUST_MIN_STACK"), "phase_timings": env.get("GRAPHICLE_PHASE_TIMINGS"),
                "scope": "Full pinned trees; source delivery only. No independent production queries or answer-quality labels."}
    save(args.out / "PROTOCOL.json", protocol)
    cases = []
    for i, query in enumerate(dataset["queries"]):
        workspace = len(query["projects"]) > 1
        case = {"query": query["id"], "scope": "workspace" if workspace else "single",
                "split": query["split"], "required_total": len(query["required"]), "variants": {}}
        variants = [("cross_off", False), ("cross_on", True)] if workspace else [("single", False)]
        if args.defaults:
            variants = [("defaults", None)]
        if i % 2:
            variants.reverse()
        for variant, cross in variants:
            request = make_request(query, local, cross, len(repositories), args.defaults)
            if args.diagnose_reranker:
                request["debug_candidates"] = 400
            path = args.out / "cases" / query["id"] / variant
            save(path.with_suffix(".request.json"), request)
            root = roots[query["projects"][0]]
            try:
                start = time.monotonic()
                packet = call(root, "call", "context", "--project", root,
                              "--arguments", path.with_suffix(".request.json").resolve())
                save(path.with_suffix(".packet.json"), packet)
                case["variants"][variant] = {**score_scoped(packet, query, files, repositories),
                                             "seconds": time.monotonic() - start}
                if args.diagnose_reranker:
                    case["variants"][variant]["reranker_visibility"] = scored_visibility(packet, query, files, repositories)
            except Exception as error:
                case["variants"][variant] = {"error": str(error), "complete": False}
        cases.append(case)
        save(args.out / "RESULTS.json", {"protocol": protocol, "summary": summarize(cases), "cases": cases})
        print(query["id"], {v: r.get("error", r["complete"]) for v, r in case["variants"].items()}, flush=True)
    print(json.dumps(summarize(cases), indent=2))
    return int(any("error" in r for c in cases for r in c["variants"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
