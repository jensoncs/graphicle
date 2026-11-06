#!/usr/bin/env python3
"""Qualify expanded and reviewed-memory context on a completed graph fixture.

Clones the store and query caches from qualify_large_graph.py into a new
disposable directory, omitting graph-build caches that these reads do not use.
Tests source symbols near the beginning, middle and end, with three reviewed
notes bound to their sources. No source corpus or original store is mutated.
These are feature costs and correctness checks, not equivalent A/B workloads.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import platform
import shutil
import sqlite3
import statistics
from pathlib import Path

from large_repositories import run, save, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("Use at least three measured samples")
    binary = args.binary.resolve(strict=True)
    fixture = args.fixture.resolve(strict=True)
    result_path = fixture / "results.json"
    prior = json.loads(result_path.read_text())
    assert prior["fresh_cache_equal"] is True, "Graph fixture did not finish qualification"
    identity = sha(binary)
    assert identity == prior["binary_sha256"], "Use the binary that qualified this fixture"
    provenance = {name: sha(Path(__file__).with_name(name)) for name in
                  ("qualify_context.py", "large_repositories.py")}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    store = root / "store"
    original_store = fixture / "store"

    def skip_build_caches(directory, names):
        # This fixture only reads context and authors notes. Keep SQLite,
        # passage indexes and saved query graphs; parsing, resolution and
        # materialized-source caches are used only by graph builds. Avoid
        # copying hundreds of thousands of unrelated build-cache files.
        if Path(directory).parent == original_store / "gf-cache":
            return set(names) & {"resolution", "sources", "sources.lock"}
        return set()

    shutil.copytree(original_store, store, ignore=skip_build_caches)
    repository = json.loads((fixture / "admit.json").read_text())["repository_id"]
    heads = json.loads((fixture / "final-heads.json").read_text())
    paths = sorted(p for p in heads if p.startswith("unit_"))
    probes = [paths[0], paths[len(paths) // 2], paths[-1]]
    assert len(set(probes)) == 3 and all(p in heads for p in probes)
    sources = fixture / "sources"
    rows, notes = [], {}

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    def call(label, operation, request):
        path = root / f"{label}.request.json"
        save(path, request)
        return run(binary, root, store, label, ["call", operation, "--project", repository,
                                              "--arguments", str(path)])

    episode = next(row["graph"]["episode"] for row in reversed(prior["rows"]) if "graph" in row)
    for path in probes:
        index = int(Path(path).stem.removeprefix("unit_"))
        raw = (sources / path).read_bytes()
        source = raw.decode()
        functions = {node.name: ast.get_source_segment(source, node) for node in ast.parse(source).body
                     if isinstance(node, ast.FunctionDef)}
        prefix = Path(path).stem + "_"
        row, packet = call(f"neighbors-{index}", "graph.navigate", {
            "tool": "get_neighbors", "node_id": prefix + f"run_{index}", "relation_filter": "calls"})
        assert packet["episode"] == packet["current_episode"] == episode
        assert {node["node_id"] for node in packet["nodes"]} == {prefix + f"run_{index}", prefix + "process"}
        assert not packet.get("nodes_omitted") and not packet.get("nodes_unmatched")
        for node in packet["nodes"]:
            assert node["checked"] and node["relpath"] == path and node["sha256"] == heads[path] == hashlib.sha256(raw).hexdigest()
            assert 0 <= node["byte_start"] < node["byte_end"] <= len(raw)
            name = node["node_id"].removeprefix(prefix)
            assert raw[node["byte_start"]:node["byte_end"]].decode().strip() == functions[name].strip(), "Navigation span names the wrong declaration"
        row.update(source=path, nodes=len(packet["nodes"]), episode=episode)
        record(row)

    for path in probes:
        index = int(Path(path).stem.removeprefix("unit_"))
        assert sha(sources / path) == heads[path]
        row, note = call(f"note-{index}", "note", {
            "title": f"run_{index} source policy",
            "body": f"run_{index} calls the local process function in {path}. Check this source before changing its behavior.",
            "dependencies": [path]})
        assert note["state"] == "reviewed"
        notes[path] = note
        row.update(memory_id=note["id"], source=path)
        record(row)

    # The public observer workflow calls a model. Seed three deterministic,
    # exactly quoted observer records only in this disposable clone so the
    # real note_keys retrieval path can be exercised without model calls.
    # This qualifies reading these records, not their authoring or invalidation.
    observer_notes = []
    with sqlite3.connect(store / "reviews.sqlite3") as db:
        assert db.execute("SELECT count(*) FROM am_observer_notes WHERE repository_id=?", (repository,)).fetchone()[0] == 0
        for position, path in enumerate(probes):
            index = int(Path(path).stem.removeprefix("unit_"))
            raw = (sources / path).read_bytes()
            assert hashlib.sha256(raw).hexdigest() == heads[path]
            observed = [{"text": f"run_{index} invokes the local process function in {path}.",
                         "evidence": [{"quote": raw.decode(), "byte_start": 0, "byte_end": len(raw)}]}]
            db.execute("INSERT INTO am_observer_notes(repository_id,source_id,source_sha256,source_version,step_key,model_key,principal_hash,job_id,source_order,session_date,notes,dropped,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                       (repository, "file:" + path, heads[path], heads[path], "synthetic-scale-fixture",
                        "no-model", "synthetic-fixture", "synthetic-fixture", f"{position:05}", None,
                        json.dumps(observed), 0, "2026-11-06T00:00:00Z"))
            observer_notes.append({"source": path, "sha256": heads[path], "notes": observed})
    save(root / "observer-fixture.json", observer_notes)

    workloads = {
        "source": {"graph_retrieval": False, "memory_retrieval": False},
        "graph": {"graph_retrieval": True, "memory_retrieval": False},
        "memory": {"graph_retrieval": False, "memory_retrieval": True},
        "memory_graph": {"graph_retrieval": True, "memory_retrieval": True},
        "variants": {"graph_retrieval": True, "memory_retrieval": True},
        "key_expansion": {"graph_retrieval": True, "memory_retrieval": True, "note_keys": "merge"},
        "filtered": {"graph_retrieval": True, "memory_retrieval": True},
    }
    expected = {}
    for sample in range(args.samples + 1):
        order = list(workloads)
        if sample % 2:
            order.reverse()
        for path in probes:
            index = int(Path(path).stem.removeprefix("unit_"))
            for workload in order:
                request = {"question": f"What does run_{index} call?", "max_tokens": 3000,
                           **workloads[workload]}
                if workload == "variants":
                    request["query_variants"] = [{"text": f"run_{index}"}, {"text": f"{path} process"}]
                if workload == "filtered":
                    request["include"] = ["file:" + path]
                row, packet = call(f"{workload}-{index}-{sample}", "context", request)
                assert packet["repository_id"] == repository
                assert packet["model_calls"] == 0 and packet["tokens"] <= request["max_tokens"]
                mechanisms = packet["mechanisms"]
                assert mechanisms["reviewed_memory_enabled"] == request["memory_retrieval"]
                assert mechanisms["graph_retrieval_enabled"] == request["graph_retrieval"]
                assert packet.get("graph_source_error") is None
                if request["graph_retrieval"]:
                    assert packet["code_graph"]["status"] == "fused", packet["code_graph"]
                    assert packet["code_graph"]["episode"] == episode and packet["code_graph"]["episode_lag"] == 0
                else:
                    assert not mechanisms["graph_searched"]
                if workload == "key_expansion":
                    assert packet["note_keys"]["mode"] == "merge"
                    assert packet["note_keys"]["keyed_notes"] > 0 and packet["note_keys"]["keyed_passages"] > 0, "No observer keys were applied"
                if workload == "variants":
                    assert len(packet["query_variants"]) == len(request["query_variants"]), "Query variants were not evaluated"
                    assert all(variant["ranked"] > 0 for variant in packet["query_variants"]), "Query variant had no source candidates"
                assert any(e["id"] == "file:" + path for e in packet["evidence"]), (workload, path, "Labelled source missing")
                for evidence in packet["evidence"]:
                    relative = evidence["id"].removeprefix("file:")
                    assert relative in heads, relative
                    raw = (sources / relative).read_bytes()
                    assert hashlib.sha256(raw).hexdigest() == heads[relative] == evidence["sha256"]
                    assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
                    if workload == "filtered":
                        assert relative == path, "Source filter escaped"
                memories = packet["memories"]
                if request["memory_retrieval"]:
                    assert any(m["id"] == notes[path]["id"] for m in memories), (workload, path, "Reviewed note missing")
                    by_id = {note["id"]: note for note in notes.values()}
                    for memory in memories:
                        note = by_id[memory["id"]]
                        assert memory["revision"] == note["revision"]
                        assert memory["title"] == note["title"] and memory["data"] == note["data"]
                        # Structured memory records omit the body; the packed
                        # Markdown must carry its exact text for these notes.
                        assert note["body"] in packet["markdown"], "Reviewed memory body was not delivered intact"
                        assert memory.get("stale") is not True
                    if workload == "filtered":
                        assert {m["id"] for m in memories} == {notes[path]["id"]}
                else:
                    assert not memories, "Memory-disabled request returned reviewed notes"
                    assert not any(note["body"] in packet["markdown"] for note in notes.values())
                key = (workload, path)
                if key in expected:
                    assert packet == expected[key], (key, "Repeated read changed")
                else:
                    expected[key] = packet
                row.update(workload=workload, source=path, sample=sample, warmup=sample == 0,
                           tokens=packet["tokens"], evidence=len(packet["evidence"]), memories=len(memories),
                           coverage=packet.get("coverage"), mechanisms=packet.get("mechanisms"),
                           note_keys=packet.get("note_keys"), query_variants=packet.get("query_variants"),
                           passage_index=packet.get("passage_index"), limitations=packet.get("limitations"))
                record(row)
    summary = {}
    for workload in workloads:
        summary[workload] = {}
        for path in probes:
            measured = [r for r in rows if r.get("workload") == workload and r["source"] == path and not r["warmup"]]
            summary[workload][path] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                                      "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured)}
    assert sha(binary) == identity
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    save(root / "results.json", {"binary_sha256": identity, "harness_sha256": provenance,
         "fixture": str(fixture), "fixture_result_sha256": sha(result_path), "source_files": len(heads),
         "store_copy": "SQLite and all query caches preserved; only gf-cache/<project>/{resolution,sources,sources.lock} omitted; no graph build is measured",
         "source_bytes": sum((sources / path).stat().st_size for path in heads), "probes": probes,
         "reviewed_notes": len(notes), "seeded_observer_notes": len(observer_notes),
         "observer_fixture_sha256": sha(root / "observer-fixture.json"),
         "samples": args.samples, "rows": rows, "summary": summary,
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Fresh CLI processes measured with fresh workers; warm OS cache; one warmup per workload/probe; alternating workload order; exact repeated packets, citation bytes, labelled source, reviewed notes and budgets checked; caller/callee citation spans independently checked against Python AST declarations",
         "limitations": "One synthetic Python code corpus, three reviewed notes, and three directly seeded observer records with exact source quotes. Observer record authoring and invalidation are not exercised. Measures feature costs, not an A/B optimization. Does not qualify a large memory catalogue, note-only query recall, or independent user-question recall."})


if __name__ == "__main__":
    main()
