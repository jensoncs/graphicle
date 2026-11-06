#!/usr/bin/env python3
"""Compare navigation with per-build verified caches on a qualified fixture.

Each binary prepares its own cache through the existing pack operation in a
disposable store. Park those stores by rename so timed calls share one path.
Setup, source/graph integrity checks and packet verification are untimed.
"""
from __future__ import annotations

import argparse
import ast
import json
import os
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
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("Use at least three measured samples")
    binaries = {"baseline": args.baseline.resolve(strict=True),
                "candidate": args.candidate.resolve(strict=True)}
    identities = {arm: sha(binary) for arm, binary in binaries.items()}
    provenance = {name: sha(Path(__file__).with_name(name)) for name in
                  ("compare_navigation_cache.py", "compare_export_linking.py", "large_repositories.py")}
    fixture = args.fixture.resolve(strict=True)
    fixture_hashes = {name: sha(fixture / name) for name in ("results.json", "admit.json", "final-heads.json")}
    prior = json.loads((fixture / "results.json").read_text())
    assert prior["fresh_cache_equal"] is True
    heads = json.loads((fixture / "final-heads.json").read_text())
    repository = json.loads((fixture / "admit.json").read_text())["repository_id"]
    episode = next(row["graph"]["episode"] for row in reversed(prior["rows"]) if "graph" in row)
    path = next(p for p in sorted(heads) if p.startswith("unit_"))
    index = int(Path(path).stem.removeprefix("unit_"))
    target = Path(path).stem + f"_run_{index}"
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    store = root / "store"
    original = fixture / "store"
    original_database = original / "reviews.sqlite3"
    immutable = immutable_snapshot(original_database)
    with connect(original_database) as db:
        assert dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (repository,))) == heads
        attributes = json.loads(db.execute(
            "SELECT attributes FROM gf_episodes WHERE repository_id=? AND episode=?", (repository, episode)).fetchone()[0])
        assert attributes["source_heads"] == heads

    def skip_build_caches(directory, names):
        directory = Path(directory)
        if directory == original:
            return set(names) & {"reviews.sqlite3", "reviews.sqlite3-wal", "reviews.sqlite3-shm"}
        if directory.parent == original / "gf-cache":
            return set(names) & {"resolution", "sources", "sources.lock"}
        return set()

    prepared = {}
    setup = []
    for arm, binary in binaries.items():
        shutil.copytree(original, store, ignore=skip_build_caches)
        with connect(original_database) as source, closing(sqlite3.connect(store / "reviews.sqlite3")) as destination:
            source.backup(destination)
        request = root / f"{arm}-pack.request.json"
        save(request, {"question": f"run_{index}", "budget": 1000})
        row, _ = run(binary, root, store, f"{arm}-prepare",
                     ["call", "pack", "--project", repository, "--arguments", str(request)])
        # Pack refreshes both the saved graph and query file for this binary's
        # build key. Old fixture caches cannot qualify a new binary's reuse.
        assert "wrote the project's query file for this build" in (root / f"{arm}-prepare.log").read_text()
        graph = store / "gf-cache" / repository / "query" / f"{episode}.graph"
        assert graph.is_file() and graph.stat().st_size > 0
        assert immutable_snapshot(store / "reviews.sqlite3") == immutable
        prepared[arm] = {"graph_path": str(graph.relative_to(store)),
                         "graph_sha256": sha(graph), "graph_bytes": graph.stat().st_size}
        setup.append(row)
        store.rename(root / f"parked-{arm}")
    save(root / "setup.json", {"calls": setup, "caches": prepared})

    requests = {
        "neighbors": {"tool": "get_neighbors", "node_id": target, "relation_filter": "calls"},
        "search": {"tool": "query_graph", "question": f"run_{index}"},
    }
    for name, request in requests.items():
        save(root / f"{name}.request.json", request)
    sources = {}

    def verify(packet, workload):
        assert packet["episode"] == packet["current_episode"] == episode
        assert not packet.get("nodes_omitted") and not packet.get("nodes_unmatched")
        identifiers = {node["node_id"] for node in packet["nodes"]}
        assert target in identifiers
        if workload == "neighbors":
            assert identifiers == {target, Path(path).stem + "_process"}
        for node in packet["nodes"]:
            relative = node["relpath"]
            assert node["checked"] and relative in heads
            if relative not in sources:
                raw = (fixture / "sources" / relative).read_bytes()
                assert sha(fixture / "sources" / relative) == heads[relative]
                source = raw.decode()
                declarations = {n.name: ast.get_source_segment(source, n) for n in ast.parse(source).body
                                if isinstance(n, ast.FunctionDef)}
                sources[relative] = (raw, declarations)
            raw, declarations = sources[relative]
            assert node["sha256"] == heads[relative]
            start, end = node["byte_start"], node["byte_end"]
            assert 0 <= start < end <= len(raw)
            if node["kind"] == "file":
                assert start == 0 and end == len(raw)
            else:
                name = node["label"].removesuffix("()")
                assert raw[start:end].decode().strip() == declarations[name].strip()

    rows, expected = [], {}
    for sample in range(args.samples + 1):
        order = list(binaries)
        if sample % 2:
            order.reverse()
        for arm in order:
            parked = root / f"parked-{arm}"
            parked.rename(store)
            workloads = list(requests)
            if sample % 2:
                workloads.reverse()
            for workload in workloads:
                label = f"{sample}-{arm}-{workload}"
                row, packet = run(binaries[arm], root, store, label,
                                  ["call", "graph.navigate", "--project", repository,
                                   "--arguments", str(root / f"{workload}.request.json")])
                verify(packet, workload)
                if workload not in expected:
                    expected[workload] = packet
                    save(root / f"expected-{workload}.json", packet)
                assert packet == expected[workload], f"Complete packet differs: {label}"
                row.update(arm=arm, workload=workload, sample=sample, warmup=sample == 0,
                           complete_packet_equal=True, source_spans_checked_against_ast=True)
                rows.append(row)
                save(root / "observations.json", rows)
                print(json.dumps(row), flush=True)
            store.rename(parked)
    for arm in binaries:
        parked = root / f"parked-{arm}"
        assert immutable_snapshot(parked / "reviews.sqlite3") == immutable
        assert sha(parked / prepared[arm]["graph_path"]) == prepared[arm]["graph_sha256"]
    assert immutable_snapshot(original_database) == immutable
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    assert fixture_hashes == {name: sha(fixture / name) for name in fixture_hashes}
    summary = {}
    for workload in requests:
        summary[workload] = {}
        for arm in binaries:
            measured = [r for r in rows if r["workload"] == workload and r["arm"] == arm and not r["warmup"]]
            summary[workload][arm] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                                     "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured)}
    save(root / "results.json", {
        "binary_sha256": identities, "harness_sha256": provenance,
        "fixture": str(fixture), "fixture_sha256": fixture_hashes,
        "source_files": len(heads), "probe": path, "samples": args.samples,
        "setup": setup, "prepared_caches": prepared, "immutable": immutable,
        "rows": rows, "summary": summary, "platform": platform.platform(), "cpu_count": os.cpu_count(),
        "method": "Each binary's cache is prepared through pack in a disposable clone. Stores are parked by rename to use the same path. Fresh CLI and measurement processes, warm OS cache, alternating arm and workload order, one excluded warmup per arm/workload. Complete packets and independent AST citation spans match; all source/graph/access rows and saved graphs remain unchanged.",
        "limitations": "Synthetic Python search and caller/callee navigation, with compatible saved graphs. Measures first-request resident setup, not warm-server latency, parsing or source updates. Missing/damaged/disabled/wrong-build caches are covered by focused integration tests, not these timings."})


if __name__ == "__main__":
    main()
