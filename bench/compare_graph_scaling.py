#!/usr/bin/env python3
"""Paired graph build/update comparisons with identical IDs and cache roots.

Each arm starts from the same admitted, unprojected synthetic store. Restoring
it at the same path preserves repository identity and materialization paths;
each binary builds its own derived caches. No model or user checkout is used.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sqlite3
import statistics
import subprocess
from pathlib import Path

from graph_scaling import inspect, source
from large_repositories import run, save, sha


def copy_store(source, destination):
    """Isolate a disposable store, using native copy-on-write where available."""
    assert not destination.exists(), destination
    if platform.system() == "Darwin":
        result = subprocess.run(["/bin/cp", "-cRp", str(source), str(destination)],
                                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        if result.returncode == 0:
            return
        # A filesystem without cloning may have created part of the tree.
        if destination.exists():
            shutil.rmtree(destination)
    shutil.copytree(source, destination)


def snapshot(store, repository, episode):
    with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
        nodes = [[identifier, json.loads(attributes)] for identifier, attributes in db.execute(
            "SELECT node_id,attributes FROM gf_nodes WHERE repository_id=? AND valid_to_episode IS NULL ORDER BY node_id",
            (repository,))]
        edges = [[a, b, relation, key, json.loads(attributes)] for a, b, relation, key, attributes in db.execute(
            "SELECT source,target,relation,key,attributes FROM gf_edges WHERE repository_id=? AND valid_to_episode IS NULL ORDER BY source,target,relation,key",
            (repository,))]
        directed, multigraph, attributes = db.execute(
            "SELECT directed,multigraph,attributes FROM gf_episodes WHERE repository_id=? AND episode=?",
            (repository, episode)).fetchone()
        attributes = json.loads(attributes)
        # Work counters describe the run, not graph content. Preserve every
        # node/edge attribute, source-head hash and remaining graph attribute.
        attributes.pop("extraction_run", None)
    return json.dumps({"nodes": nodes, "edges": edges, "directed": directed,
                       "multigraph": multigraph, "attributes": attributes},
                      sort_keys=True, separators=(",", ":")).encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--files", type=int, default=5000)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.files < 3 or args.samples < 3:
        parser.error("Use at least three files and three samples")
    binaries = {"baseline": args.baseline.resolve(strict=True),
                "candidate": args.candidate.resolve(strict=True)}
    identities = {arm: sha(binary) for arm, binary in binaries.items()}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    rows, expected, migrations = [], {}, []
    for kind in ("sparse", "common"):
        directory = root / kind
        directory.mkdir()
        folder, store, seed = directory / "sources", directory / "working-store", directory / "seed-store"

        def reset_sources():
            if folder.exists():
                shutil.rmtree(folder)
            folder.mkdir()
            sources = {f"unit_{i:05}.py": i for i in range(args.files)}
            for path, i in sources.items():
                (folder / path).write_text(source(kind, i))
            return sources

        reset_sources()
        save(directory / "models.json", {})
        _, admitted = run(binaries["baseline"], directory, store, "admit",
                          ["sync", str(folder), "--no-graph"])
        repository = admitted["repository_id"]
        copy_store(store, seed)
        for sample in range(args.samples):
            arms = list(binaries)
            if sample % 2:
                arms.reverse()
            for arm in arms:
                # Only disposable paths inside the newly created output tree.
                shutil.rmtree(store)
                copy_store(seed, store)
                sources = reset_sources()
                logs = directory / f"{sample}-{arm}"
                logs.mkdir()
                save(logs / "models.json", {})
                for phase in ("cold_graph", "unchanged", "edit", "rename", "delete"):
                    if phase == "edit":
                        (folder / "unit_00000.py").write_text(source(kind, 0, revision=1))
                    elif phase == "rename":
                        (folder / "unit_00001.py").rename(folder / "renamed_00001.py")
                        sources["renamed_00001.py"] = sources.pop("unit_00001.py")
                    elif phase == "delete":
                        (folder / "unit_00002.py").unlink()
                        sources.pop("unit_00002.py")
                    row, packet = run(binaries[arm], logs, store, phase, ["sync", str(folder)])
                    graph = packet["graph"]
                    assert packet["repository_id"] == repository
                    assert graph["status"] == ("current" if phase == "unchanged" else "built"), graph
                    heads = {path: sha(folder / path) for path in sorted(sources)}
                    extraction = inspect(store, repository, graph["episode"], sources, kind, heads)
                    data = snapshot(store, repository, graph["episode"])
                    digest = hashlib.sha256(data).hexdigest()
                    key = (kind, phase)
                    if key not in expected:
                        expected[key] = digest
                        (directory / f"expected-{phase}.json").write_bytes(data)
                    if expected[key] != digest:
                        (logs / f"mismatch-{phase}.json").write_bytes(data)
                        raise AssertionError(f"{kind}/{phase}: graph differs; see expected and mismatch snapshots")
                    del data
                    row.update(kind=kind, phase=phase, arm=arm, sample=sample, graph=graph,
                               graph_sha256=digest,
                               source_manifest_sha256=hashlib.sha256(json.dumps(heads, sort_keys=True).encode()).hexdigest(),
                               extraction=extraction if phase != "unchanged" else None)
                    rows.append(row)
                    save(root / "observations.json", rows)
                    print(json.dumps(row), flush=True)
                    if sample == 0 and phase == "cold_graph":
                        copy_store(store, directory / f"{arm}-cold-cache")
        # Exercise real cache upgrades and rollbacks separately from timings.
        # The edited graph must match the fresh-build arm's complete snapshot.
        for previous, current in (("baseline", "candidate"), ("candidate", "baseline")):
            shutil.rmtree(store)
            copy_store(directory / f"{previous}-cold-cache", store)
            sources = reset_sources()
            (folder / "unit_00000.py").write_text(source(kind, 0, revision=1))
            logs = directory / f"{previous}-to-{current}"
            logs.mkdir()
            save(logs / "models.json", {})
            _, packet = run(binaries[current], logs, store, "edit", ["sync", str(folder)])
            assert packet["repository_id"] == repository
            assert packet["graph"]["status"] == "built", packet["graph"]
            heads = {path: sha(folder / path) for path in sorted(sources)}
            extraction = inspect(store, repository, packet["graph"]["episode"], sources, kind, heads)
            data = snapshot(store, repository, packet["graph"]["episode"])
            digest = hashlib.sha256(data).hexdigest()
            if digest != expected[(kind, "edit")]:
                (logs / "mismatch.json").write_bytes(data)
                raise AssertionError(f"{kind}: {previous}-to-{current} graph differs")
            del data
            migrations.append({"kind": kind, "from": previous, "to": current,
                               "graph_sha256": digest, "extraction": extraction})
            save(root / "migrations.json", migrations)
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    summary = {}
    for kind, phase in expected:
        summary[f"{kind}/{phase}"] = {}
        for arm in binaries:
            selected = [r for r in rows if (r["kind"], r["phase"], r["arm"]) == (kind, phase, arm)]
            summary[f"{kind}/{phase}"][arm] = {
                "median_seconds": statistics.median(r["seconds"] for r in selected),
                "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in selected)}
    save(root / "results.json", {"binary_sha256": identities, "files": args.files,
         "samples": args.samples, "rows": rows, "summary": summary, "migrations": migrations,
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "harness_sha256": {name: sha(Path(__file__).with_name(name)) for name in
                            ("compare_graph_scaling.py", "graph_scaling.py", "large_repositories.py")},
         "method": "Alternating arm order; identical admitted seed restored at the same path per arm/sample; fresh CLI processes and warm OS cache; cold graph then unchanged/edit/rename/delete; setup excluded; process peak RSS",
         "correctness": "Exact generated topology and source hashes, plus identical canonical graph data across every arm/sample; only extraction_run work counters and persistence timestamps are excluded",
         "limitations": "Synthetic Python local calls; no mixed-language or cross-repository completeness claim. Cold graph excludes initial source admission and passage indexing."})


if __name__ == "__main__":
    main()
