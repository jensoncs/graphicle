#!/usr/bin/env python3
"""Qualify a large common-name code graph, its updates and cited reads.

Uses one candidate, an independently checked synthetic topology, and a fresh
cache rebuild at the same store path. The old quadratic dependency graph is
not constructed. This measures feasibility, not an A/B speedup or language
completeness. Only a new disposable output directory is modified.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
from pathlib import Path

from compare_graph_scaling import copy_store, snapshot
from graph_scaling import inspect, source
from large_repositories import run, save, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--files", type=int, default=100000)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.files < 8 or args.samples < 3:
        parser.error("Use at least eight files and three samples")
    binary, root = args.binary.resolve(strict=True), args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    folder, store, seed = root / "sources", root / "store", root / "admitted-seed"
    folder.mkdir()
    save(root / "models.json", {})
    identity = sha(binary)
    sources = {f"unit_{i:05}.py": i for i in range(args.files)}
    for path, index in sources.items():
        (folder / path).write_text(source("common", index))
    rows = []

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    row, admitted = run(binary, root, store, "admit", ["sync", str(folder), "--no-graph"])
    repository = admitted["repository_id"]
    record(row)
    copy_store(store, seed)
    original_heads = {path: sha(folder / path) for path in sorted(sources)}
    save(root / "original-heads.json", original_heads)

    def sync(label, unchanged=False):
        row, packet = run(binary, root, store, label, ["sync", str(folder)])
        graph = packet["graph"]
        assert packet["repository_id"] == repository
        assert graph["status"] == ("current" if unchanged else "built"), graph
        heads = {path: sha(folder / path) for path in sorted(sources)}
        extraction = inspect(store, repository, graph["episode"], sources, "common", heads)
        if not unchanged:
            resolution = extraction["resolution"]
            assert resolution["fallback"] is None, resolution
            assert resolution["dirty_files"] == len(sources), resolution
            assert resolution["resolved_files"] == len(sources), resolution
            assert resolution["components"] == 1, resolution
            assert resolution["largest_component"] == len(sources), resolution
        data = snapshot(store, repository, graph["episode"])
        digest = hashlib.sha256(data).hexdigest()
        if label in ("bulk_delete", "fresh_rebuild"):
            (root / f"{label}-graph.json").write_bytes(data)
        del data
        row.update(graph=graph, files=len(sources), graph_sha256=digest,
                   extraction=None if unchanged else extraction,
                   source_manifest_sha256=hashlib.sha256(json.dumps(heads, sort_keys=True).encode()).hexdigest())
        record(row)
        return row

    cold = sync("cold_graph")
    for sample in range(args.samples):
        current = sync(f"unchanged-{sample}", unchanged=True)
        assert current["graph_sha256"] == cold["graph_sha256"]
    for sample in range(args.samples):
        (folder / "unit_00000.py").write_text(source("common", 0, revision=sample + 1))
        sync(f"edit-{sample}")
    (folder / "unit_00001.py").rename(folder / "renamed_00001.py")
    sources["renamed_00001.py"] = sources.pop("unit_00001.py")
    sync("rename")
    (folder / "unit_00002.py").unlink()
    sources.pop("unit_00002.py")
    sync("delete")
    batch = max(1, args.files // 100)
    editable = [p for p, i in sources.items() if i not in (0, 1, 2, 7)]
    for path in editable[:batch]:
        (folder / path).write_text(source("common", sources[path], revision=100))
    sync("bulk_edit")
    for path in editable[-batch:]:
        (folder / path).unlink()
        sources.pop(path)
    final = sync("bulk_delete")
    save(root / "final-heads.json", {path: sha(folder / path) for path in sorted(sources)})

    # Retain the incremental store, then rebuild from the admitted, unprojected
    # seed at the same absolute path. IDs and materialization paths stay bound.
    store.rename(root / "incremental-store")
    copy_store(seed, store)
    fresh = sync("fresh_rebuild")
    assert fresh["graph_sha256"] == final["graph_sha256"], "Fresh cache graph differs"
    assert fresh["graph"]["episode"] == final["graph"]["episode"]

    question = {"repository_id": repository, "question": "What does run_7 call?",
                "max_tokens": 3000, "graph_retrieval": True, "memory_retrieval": False}
    save(root / "context.request.json", question)
    row, packet = run(binary, root, store, "context", ["call", "context", "--project", repository,
                        "--arguments", str(root / "context.request.json")])
    assert packet["tokens"] <= question["max_tokens"] and packet["model_calls"] == 0
    assert any(e["id"] == "file:unit_00007.py" for e in packet["evidence"]), "Exact-symbol source missing"
    for evidence in packet["evidence"]:
        path = evidence["id"].removeprefix("file:")
        assert path in sources, path
        raw = (folder / path).read_bytes()
        assert hashlib.sha256(raw).hexdigest() == evidence["sha256"]
        assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
    row.update(tokens=packet["tokens"], evidence=len(packet["evidence"]),
               mechanisms=packet.get("mechanisms"), limitations=packet.get("limitations"))
    record(row)

    navigation = {"repository_id": repository, "tool": "get_neighbors",
                  "node_id": "unit_00007_run_7", "relation_filter": "calls"}
    save(root / "neighbors.request.json", navigation)
    row, packet = run(binary, root, store, "neighbors", ["call", "graph.navigate", "--project", repository,
                        "--arguments", str(root / "neighbors.request.json")])
    assert packet["episode"] == fresh["graph"]["episode"] == packet["current_episode"]
    expected = {"unit_00007_run_7", "unit_00007_process"}
    assert expected <= {n["node_id"] for n in packet["nodes"]}, packet
    assert not packet.get("nodes_omitted") and not packet.get("nodes_unmatched"), packet
    for node in packet["nodes"]:
        assert node["checked"] and node["relpath"] == "unit_00007.py", node
        raw = (folder / node["relpath"]).read_bytes()
        assert hashlib.sha256(raw).hexdigest() == node["sha256"]
        assert 0 <= node["byte_start"] < node["byte_end"] <= len(raw)
    row.update(nodes=len(packet["nodes"]), episode=packet["episode"])
    record(row)
    assert sha(binary) == identity, "Binary changed during qualification"
    save(root / "results.json", {"binary_sha256": identity, "files": args.files,
         "final_files": len(sources), "batch_files": batch, "samples": args.samples, "rows": rows,
         "fresh_cache_equal": True, "platform": platform.platform(), "cpu_count": os.cpu_count(),
         "engine_max_workers_env": 4,
         "harness_sha256": {name: sha(Path(__file__).with_name(name)) for name in
                            ("qualify_large_graph.py", "compare_graph_scaling.py", "graph_scaling.py", "large_repositories.py")},
         "method": "Fresh CLI processes measured by fresh Python workers; warm OS cache; admission separated from cold graph; exact generated topology/source hashes throughout; final incremental graph equals fresh-cache graph at same store path",
         "limitations": "One synthetic Python repository with common local function names. Timing samples establish observed costs, not an A/B speedup, realistic language completeness or a production SLO. Fresh-cache comparison uses the same incremental implementation; independent full-extraction checks are separate."})


if __name__ == "__main__":
    main()
