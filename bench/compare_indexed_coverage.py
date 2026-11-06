#!/usr/bin/env python3
"""Compare indexed workspace coverage on a completed synthetic qualification.

Each arm gets an isolated SQLite backup and query caches at the same parked
path. Fresh CLI processes, alternating arm order, one excluded warmup, and no
fixture mutation. Coverage may intentionally differ; independently verify
labelled sources, citation bytes, budgets and repeatability within each arm.
Use --require-identical for changes that must preserve complete packets.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
import sqlite3
import statistics
from contextlib import closing
from pathlib import Path

from compare_export_linking import connect, immutable_snapshot
from large_repositories import run, save, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("baseline", "candidate", "fixture", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--require-identical", action="store_true",
                        help="Require complete packets to match across binaries, not just within each arm")
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("Use at least three measured samples")
    binaries = {name: getattr(args, name).resolve(strict=True) for name in ("baseline", "candidate")}
    identities = {name: sha(binary) for name, binary in binaries.items()}
    fixture = args.fixture.resolve(strict=True)
    prior_hash = sha(fixture / "results.json")
    prior = json.loads((fixture / "results.json").read_text())
    projects = prior["projects"]
    assert len(projects) == 10 and len(prior["rows"]) == 73
    provenance = {name: sha(Path(__file__).with_name(name)) for name in
                  ("compare_indexed_coverage.py", "compare_export_linking.py", "large_repositories.py")}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    original, store = fixture / "store", root / "store"
    expected = immutable_snapshot(original / "reviews.sqlite3")
    save(root / "immutable.json", expected)
    with connect(original / "reviews.sqlite3") as db:
        counts = {}
        for project in projects:
            heads = dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (project["id"],)))
            assert heads == project["heads"]
            counts[project["id"]] = len(heads)
    total = sum(counts.values())
    print(json.dumps({"setup": "source checked", "files": total}), flush=True)

    def skip_caches(directory, names):
        directory = Path(directory)
        if directory == original:
            return set(names) & {"reviews.sqlite3", "reviews.sqlite3-wal", "reviews.sqlite3-shm"}
        if directory.parent == original / "gf-cache":
            return set(names) & {"resolution", "sources", "sources.lock"}
        return set()

    setup, caches = [], {}
    prepare_path = root / "prepare.request.json"
    save(prepare_path, {"question": "run_00001", "budget": 1000})
    for arm, binary in binaries.items():
        parked = root / f"parked-{arm}"
        shutil.copytree(original, parked, ignore=skip_caches)
        with connect(original / "reviews.sqlite3") as source, closing(sqlite3.connect(parked / "reviews.sqlite3")) as target:
            source.backup(target)
        print(json.dumps({"setup": "store copied", "arm": arm}), flush=True)
        # context reads existing caches but does not repair a different build's
        # archive. pack invokes the existing ensure path. Prepare every project
        # outside timing, as compare_memory_catalogue does for its one project.
        parked.rename(store)
        caches[arm] = {}
        for project in projects:
            row, _ = run(binary, root, store, f"prepare-{arm}-{project['index']}",
                         ["call", "pack", "--project", project["id"], "--arguments", str(prepare_path)])
            graph = store / "gf-cache" / project["id"] / "query" / f"{project['episode']}.graph"
            assert graph.is_file() and graph.stat().st_size > 0
            caches[arm][project["id"]] = {"sha256": sha(graph), "bytes": graph.stat().st_size}
            setup.append(row)
            save(root / "setup.json", {"calls": setup, "caches": caches})
            print(json.dumps({"setup": "cache prepared", "arm": arm, "project": project["index"]}), flush=True)
        store.rename(parked)
    if identities["baseline"] != identities["candidate"]:
        assert all(caches["baseline"][p["id"]]["sha256"] != caches["candidate"][p["id"]]["sha256"] for p in projects)

    requests = {}
    for cross in (False, True):
        name = "cross" if cross else "local"
        request = {"scope": "workspace", "question": "How is the cobalt4 invoice settled?",
                   "graph_retrieval": True, "memory_retrieval": False, "max_projects": 10,
                   "max_tokens": 8000, "cross_repo": cross}
        path = root / f"{name}.request.json"
        save(path, request)
        requests[name] = (request, path)
    rows, repeated = [], {}

    def verify(packet, request, arm):
        assert packet["model_calls"] == 0 and packet["tokens"] <= request["max_tokens"]
        assert packet["workspace"]["projects_searched"] == 10
        coverage = packet["coverage"]
        assert coverage["source_documents"] == total
        assert packet["mechanisms"]["graph_local_status"] == "degraded"
        assert packet["mechanisms"]["code_graph_searched"] is True
        assert packet["complete_evidence_guaranteed"] is False
        if arm == "candidate":
            # This fixture contains empty __init__.py files; they have no passage.
            assert coverage["source_passages"] == total - 1
            assert coverage["omitted_documents"] == 0
            assert coverage["passage_limit_reached"] is False
            assert not coverage.get("omitted_by_document_limit", 0)
        evidence = packet["evidence"]
        assert any(e["repository_id"] == projects[4]["id"] and e["id"] == "file:src/invoice.ts" for e in evidence)
        if request["cross_repo"]:
            assert any(e["repository_id"] == projects[1]["id"] and e["id"] == "file:src/core.ts" for e in evidence)
        with connect(store / "reviews.sqlite3") as db:
            for e in evidence:
                project, path = e["repository_id"], e["id"].removeprefix("file:")
                expected_sha = next(p["heads"][path] for p in projects if p["id"] == project)
                assert e["sha256"] == expected_sha
                content = db.execute("SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?",
                                     (project, expected_sha)).fetchone()[0].encode()
                assert hashlib.sha256(content).hexdigest() == expected_sha
                assert content[e["byte_start"]:e["byte_end"]].decode() == e["text"]
                assert not (project == projects[0]["id"] and path == "moneylib/rounding.py")
        assert not packet["memories"]

    for sample in range(args.samples + 1):
        arms = list(binaries)
        if sample % 2:
            arms.reverse()
        for arm in arms:
            parked = root / f"parked-{arm}"
            parked.rename(store)
            for name in list(requests) if sample % 2 == 0 else list(reversed(requests)):
                request, path = requests[name]
                label = f"{sample}-{arm}-{name}"
                row, packet = run(binaries[arm], root, store, label,
                                  ["call", "context", "--project", projects[4]["id"], "--arguments", str(path)])
                verify(packet, request, arm)
                if args.require_identical:
                    other = ("candidate" if arm == "baseline" else "baseline", name)
                    if other in repeated and packet != repeated[other]:
                        save(root / "mismatch.json", {"label": label, "expected": repeated[other], "actual": packet})
                        raise AssertionError(f"Packets differ across binaries: {label}")
                key = (arm, name)
                if key in repeated:
                    assert packet == repeated[key], f"Nonrepeatable packet: {label}"
                else:
                    repeated[key] = packet
                row.update(arm=arm, workload=name, sample=sample, warmup=sample == 0,
                           coverage=packet["coverage"], citations_checked=len(packet["evidence"]),
                           labelled_sources_found=True)
                rows.append(row)
                save(root / "observations.json", rows)
                print(json.dumps(row), flush=True)
            store.rename(parked)

    # Read-only queries may update derived caches, but never source or graph rows.
    for arm in binaries:
        assert immutable_snapshot(root / f"parked-{arm}" / "reviews.sqlite3") == expected
        for project in projects:
            graph = root / f"parked-{arm}" / "gf-cache" / project["id"] / "query" / f"{project['episode']}.graph"
            assert sha(graph) == caches[arm][project["id"]]["sha256"]
        print(json.dumps({"verified_immutable": arm}), flush=True)
    assert immutable_snapshot(original / "reviews.sqlite3") == expected
    assert sha(fixture / "results.json") == prior_hash
    assert identities == {name: sha(binary) for name, binary in binaries.items()}
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    summary = {}
    for name in requests:
        summary[name] = {}
        for arm in binaries:
            measured = [r for r in rows if r["workload"] == name and r["arm"] == arm and not r["warmup"]]
            summary[name][arm] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                                  "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured),
                                  "coverage": measured[0]["coverage"]}
        for field in ("median_seconds", "max_peak_rss_bytes"):
            summary[name][field + "_reduction_percent"] = 100 * (1 - summary[name]["candidate"][field] / summary[name]["baseline"][field])
    save(root / "results.json", {"files": total, "samples": args.samples, "binary_sha256": identities,
         "fixture_result_sha256": prior_hash, "harness_sha256": provenance, "platform": platform.platform(),
         "rows": rows, "summary": summary, "setup": {"calls": setup, "caches": caches},
         "complete_packets_equal_across_binaries": args.require_identical,
         "method": "Isolated stores at one path; build-matched caches prepared through pack; alternating arms/workload order; one excluded warmup; fresh CLI processes, warm OS caches; source/graph rows unchanged",
         "limitations": "Synthetic tiny files; no reviewed memories; one authored question with cross-repository retrieval off/on, no independent relevance evaluation or concurrency/SLO qualification"})


if __name__ == "__main__":
    main()
