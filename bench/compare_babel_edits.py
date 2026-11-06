#!/usr/bin/env python3
"""Compare real Babel updates using independent caches at the same store path.

Use a clean, pinned checkout. Only disposable source copies are changed; no
repository code or package scripts are executed. Complete graphs, unresolved
calls and source heads are checked after every operation, including a final
fresh-cache rebuild. Cold setup and correctness verification are not timed as
repeated-edit performance. Use --smoke for a harness/correctness check first.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import re
import statistics
from pathlib import Path

from compare_graph_scaling import copy_store
from large_repositories import run, save, sha
from qualify_babel import CLONE, ES3, inspect, prepare, probe


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    if args.samples < 1 or (not args.smoke and args.samples < 5):
        parser.error("Use positive samples; full performance comparisons need at least five")
    binaries = {arm: path.resolve(strict=True) for arm, path in
                (("baseline", args.baseline), ("candidate", args.candidate))}
    identities = {arm: sha(path) for arm, path in binaries.items()}
    helpers = ("compare_babel_edits.py", "qualify_babel.py", "compare_graph_scaling.py",
               "graph_scaling.py", "large_repositories.py")
    harness = {name: sha(Path(__file__).with_name(name)) for name in helpers}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    folder, corpus, manifest = prepare(args.checkout.resolve(strict=True), root, args.smoke)
    original = (folder / CLONE).read_text()
    assert original.count("cloneNodeInternal") == 3
    store, seed = root / "store", root / "seed"
    rows, expected = [], {}
    helper, deleted = "cloneNodeInternal", False
    admitted_heads = None

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    row, admitted = run(binaries["baseline"], root, store, "admit",
                         ["sync", str(folder), "--no-graph"])
    repository = admitted["repository_id"]
    record({**row, "phase": "admit", "warmup": True, "admission": admitted})
    store.rename(seed)

    def sync(arm, phase, sample):
        nonlocal admitted_heads
        label = f"{phase}-{sample}-{arm}"
        row, packet = run(binaries[arm], root, store, label, ["sync", str(folder)])
        assert packet["repository_id"] == repository
        assert packet["graph"]["status"] == ("current" if phase == "unchanged" else "built")
        checked, heads, nodes, calls = inspect(store, repository, packet["graph"]["episode"],
                                               folder, manifest)
        if admitted_heads is None:
            admitted_heads = set(heads)
        assert set(heads) == admitted_heads - ({ES3} if deleted else set())
        checked["reviewed_calls"] = probe(nodes, calls, folder, helper, deleted)
        actual = {name: checked[name] for name in
                  ("graph_sha256", "source_heads_sha256", "unresolved_calls_sha256")}
        key = "delete" if phase == "fresh_rebuild" else f"{phase}-{sample}"
        if phase == "delete":
            key = "delete"
        if key not in expected:
            expected[key] = actual
        assert expected[key] == actual, (label, expected[key], actual)
        phases = {}
        for name, ms in re.findall(r"^phase=(\S+) ms=(\d+)",
                                   (root / f"{label}.log").read_text(), re.M):
            phases.setdefault(name, []).append(int(ms))
        record({**row, "arm": arm, "phase": phase, "sample": sample,
                "warmup": phase != "edit" or sample == 0, "graph": packet["graph"],
                "checks": checked, "phase_milliseconds": phases})

    for arm in binaries:
        copy_store(seed, store)
        sync(arm, "cold_graph", -1)
        store.rename(root / f"parked-{arm}")
    for sample in range(args.samples + 1):
        helper = f"cloneNodeInternalGraphicleCompare{sample}"
        (folder / CLONE).write_text(original.replace("cloneNodeInternal", helper))
        order = list(binaries)
        if sample % 2:
            order.reverse()
        for arm in order:
            (root / f"parked-{arm}").rename(store)
            sync(arm, "edit", sample)
            store.rename(root / f"parked-{arm}")
    for arm in binaries:
        (root / f"parked-{arm}").rename(store)
        sync(arm, "unchanged", -1)
        store.rename(root / f"parked-{arm}")
    (folder / ES3).unlink()
    deleted = True
    for arm in reversed(list(binaries)):
        (root / f"parked-{arm}").rename(store)
        sync(arm, "delete", -1)
        store.rename(root / f"parked-{arm}")
    copy_store(seed, store)
    sync("candidate", "fresh_rebuild", -1)
    store.rename(root / "fresh-store")

    assert identities == {arm: sha(path) for arm, path in binaries.items()}
    assert harness == {name: sha(Path(__file__).with_name(name)) for name in helpers}
    for path, metadata in manifest.items():
        if path not in (CLONE, ES3):
            assert sha(folder / path) == metadata["sha256"]
    assert (folder / CLONE).read_text() == original.replace("cloneNodeInternal", helper)
    summary = {}
    for arm in binaries:
        measured = [r for r in rows if r.get("arm") == arm and not r["warmup"]]
        assert len(measured) == args.samples
        summary[arm] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                        "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured),
                        "phase_median_seconds": {
                            phase: statistics.median(r["phase_milliseconds"][phase][0] / 1000
                                                     for r in measured)
                            for phase in ("sync_read", "graph_materialize", "graph_extract",
                                          "graph_identity", "graph_persist", "graph_save")}}
    save(root / "results.json", {"binary_sha256": identities, "harness_sha256": harness,
         "corpus": corpus, "samples": args.samples, "rows": rows, "summary": summary,
         "canonical_graph_checks": len(rows) - 1, "fresh_cache_equal": True,
         "platform": platform.platform(), "cpu_count": os.cpu_count(),
         "engine_max_workers_env": 4,
         "method": "Fresh CLI/measurement processes, same admitted seed and absolute cache path; independent caches parked by rename. Five or more paired measured edits for the full corpus, one excluded warmup per arm, alternating arm order, warm OS cache. Setup and verification excluded from timings.",
         "limitations": "One real Babel corpus, dominated by parser/transformation fixtures. Does not establish general language completeness, large multi-repository throughput, warm-server latency or production SLOs. Cold builds, unchanged checks, deletion and fresh rebuild are correctness probes, not repeated performance estimates."})


if __name__ == "__main__":
    main()
