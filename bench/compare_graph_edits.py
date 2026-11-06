#!/usr/bin/env python3
"""Compare repeated graph edits while retaining each arm's own warm cache.

Creates a synthetic corpus and an admitted seed in a new disposable directory.
Each binary builds once at the same working-store path. Between calls, stores
are parked by rename, so cache roots and repository IDs stay identical without
copying every cache file before each measurement. No stores are shared by the
two arms. Cold builds are setup, not a repeated cold-performance comparison.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import statistics
from pathlib import Path

from compare_graph_scaling import copy_store, snapshot
from graph_scaling import inspect, source
from large_repositories import run, save, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--kind", choices=("sparse", "common"), required=True)
    parser.add_argument("--files", type=int, default=25000)
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.files < 3 or args.samples < 3:
        parser.error("Use at least three files and three measured samples")
    binaries = {"baseline": args.baseline.resolve(strict=True),
                "candidate": args.candidate.resolve(strict=True)}
    identities = {arm: sha(binary) for arm, binary in binaries.items()}
    helpers = ("compare_graph_edits.py", "compare_graph_scaling.py",
               "graph_scaling.py", "large_repositories.py")
    harness = {name: sha(Path(__file__).with_name(name)) for name in helpers}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    folder, store, seed = root / "sources", root / "working-store", root / "seed-store"
    folder.mkdir()
    save(root / "models.json", {})
    sources = {f"unit_{i:05}.py": i for i in range(args.files)}
    for path, index in sources.items():
        (folder / path).write_text(source(args.kind, index))
    heads = {path: sha(folder / path) for path in sources}
    save(root / "initial-heads.json", heads)
    rows, expected = [], {}
    parked = {arm: root / f"{arm}-store" for arm in binaries}

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    row, admission = run(binaries["baseline"], root, store, "admit",
                         ["sync", str(folder), "--no-graph"])
    repository = admission["repository_id"]
    row.update(phase="admit", warmup=True)
    record(row)
    store.rename(seed)

    def sync(arm, phase, sample):
        label = f"{phase}-{sample}-{arm}"
        row, packet = run(binaries[arm], root, store, label, ["sync", str(folder)])
        assert packet["repository_id"] == repository
        graph = packet["graph"]
        assert graph["status"] == "built", graph
        extraction = inspect(store, repository, graph["episode"], sources, args.kind, heads)
        cold = phase == "cold_graph"
        assert extraction["cached"] is True
        assert extraction["changed"] == (args.files if cold else 1)
        assert extraction["unchanged"] == (0 if cold else args.files - 1)
        assert extraction["materialization"]["cached"] is True
        assert extraction["materialization"]["written"] == (args.files if cold else 1)
        assert extraction["materialization"]["reused"] == (0 if cold else args.files - 1)
        resolution = extraction["resolution"]
        dirty = args.files if cold or args.kind == "common" else 1
        assert resolution["fallback"] is None
        assert resolution["dirty_files"] == resolution["resolved_files"] == dirty
        data = snapshot(store, repository, graph["episode"])
        digest = hashlib.sha256(data).hexdigest()
        key = f"{phase}-{sample}"
        if key not in expected:
            expected[key] = digest
            (root / f"expected-{key}.json").write_bytes(data)
        if expected[key] != digest:
            (root / f"mismatch-{label}.json").write_bytes(data)
            raise AssertionError(f"Canonical graph differs: {label}")
        del data
        phases = {}
        for name, ms in re.findall(r"^phase=(\S+) ms=(\d+)",
                                   (root / f"{label}.log").read_text(), re.M):
            phases.setdefault(name, []).append(int(ms))
        assert len(phases.get("graph_materialize", [])) == 1
        row.update(arm=arm, phase=phase, sample=sample, warmup=cold or sample == 0,
                   kind=args.kind, graph=graph, graph_sha256=digest, extraction=extraction,
                   phase_milliseconds=phases,
                   source_manifest_sha256=hashlib.sha256(json.dumps(heads, sort_keys=True).encode()).hexdigest())
        record(row)

    for arm in binaries:
        copy_store(seed, store)
        sync(arm, "cold_graph", -1)
        # Park only after successful verification. On failure/interruption,
        # leave the working path intact for diagnosis and any child cleanup.
        store.rename(parked[arm])

    for sample in range(args.samples + 1):
        target = folder / "unit_00000.py"
        target.write_text(source(args.kind, 0, revision=sample + 1))
        heads[target.name] = sha(target)
        order = list(binaries)
        if sample % 2:
            order.reverse()
        for arm in order:
            assert not store.exists()
            parked[arm].rename(store)
            sync(arm, "edit", sample)
            store.rename(parked[arm])

    assert len(rows) == 2 * args.samples + 5
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    assert harness == {name: sha(Path(__file__).with_name(name)) for name in helpers}
    assert heads == {path: sha(folder / path) for path in sources}
    save(root / "final-heads.json", heads)
    summary = {}
    for arm in binaries:
        measured = [row for row in rows if row.get("arm") == arm and not row["warmup"]]
        assert len(measured) == args.samples
        summary[arm] = {
            "median_seconds": statistics.median(row["seconds"] for row in measured),
            "max_peak_rss_bytes": max(row["peak_rss_bytes"] for row in measured),
            "median_materialize_seconds": statistics.median(
                row["phase_milliseconds"]["graph_materialize"][0] / 1000 for row in measured),
        }
    save(root / "results.json", {"binary_sha256": identities, "harness_sha256": harness,
         "files": args.files, "kind": args.kind, "samples": args.samples, "rows": rows,
         "summary": summary, "canonical_graph_checks": len(rows) - 1,
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Fresh CLI/measurement processes; same admitted seed, repository IDs and absolute cache path; independent retained caches parked by rename; one excluded warmup edit per arm; alternating arm order; setup/verification excluded from timings; warm OS cache",
         "correctness": "Every call checks complete synthetic topology, current source heads, parse/materialization counts, conservative dirty sets and all canonical graph attributes against the other arm",
         "limitations": "Synthetic Python single-file edits only. Does not measure repeated cold builds, rename/delete, cross-repository or mixed-language completeness; those require the separate qualification harnesses."})


if __name__ == "__main__":
    main()
