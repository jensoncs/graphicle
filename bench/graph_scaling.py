#!/usr/bin/env python3
"""Profile graph admission and updates on sparse/common-name Python corpora.

Uses a fresh synthetic folder and the real sync CLI. Every graph must contain
exactly the generated files, declarations, containment edges and local calls.
This is a scale probe, not a mixed-language completeness qualification.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import platform
import sqlite3
from pathlib import Path

from large_repositories import run, save, sha


def source(kind, index, revision=0):
    name = "process" if kind == "common" else f"process_{index}"
    return (f"def {name}(value):\n    return value + {index + revision}\n\n"
            f"def run_{index}(value):\n    return {name}(value)\n")


def inspect(store, repository, episode, sources, kind, heads):
    with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
        nodes = dict(db.execute(
            "SELECT node_id,attributes FROM gf_nodes WHERE repository_id=? AND valid_to_episode IS NULL",
            (repository,)))
        edges = list(db.execute(
            "SELECT source,target,relation FROM gf_edges WHERE repository_id=? AND valid_to_episode IS NULL",
            (repository,)))
        expected_nodes, expected_edges = set(), set()
        for path, index in sources.items():
            stem = Path(path).stem
            name = "process" if kind == "common" else f"process_{index}"
            file_id, function, caller = stem, f"{stem}_{name}", f"{stem}_run_{index}"
            expected_nodes.update((file_id, function, caller))
            expected_edges.update(((file_id, function, "contains"), (file_id, caller, "contains"),
                                   (caller, function, "calls")))
            for identifier in (file_id, function, caller):
                assert json.loads(nodes[identifier])["source_file"] == path
        assert set(nodes) == expected_nodes, "Missing or unexpected graph nodes"
        assert len(edges) == len(expected_edges) and set(edges) == expected_edges, "Graph edges differ"
        attributes = json.loads(db.execute(
            "SELECT attributes FROM gf_episodes WHERE repository_id=? AND episode=?",
            (repository, episode)).fetchone()[0])
        assert attributes["source_heads"] == heads, "Graph does not cover current source bytes"
        assert db.execute(
            "SELECT count(*) FROM gf_unresolved_calls WHERE repository_id=? AND episode=?",
            (repository, episode)).fetchone()[0] == 0, "Local calls were unresolved"
    return attributes["extraction_run"]


def dependencies(store, repository):
    # Measure the actual conservative dependency descriptions, separately from
    # timed CLI runs. Count unique file neighbors without retaining dense edges.
    index = store / "gf-cache" / repository / "resolution" / "index.json"
    files = json.loads(index.read_text())["output"]
    providers = collections.defaultdict(set)
    for i, file in enumerate(files):
        for name in file["provides"]:
            providers[name].add(i)
    edges = 0
    for i, file in enumerate(files):
        adjacent = set()
        for name in file["uses"]:
            adjacent.update(providers.get(name, ()))
        adjacent.discard(i)
        edges += len(adjacent)
    return {"files": len(files), "dependency_edges": edges,
            "index_sha256": sha(index)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--files", type=int, default=1000)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.files < 3 or args.samples < 3:
        parser.error("Use at least three files and three samples")
    binary, root = args.binary.resolve(strict=True), args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    identity = sha(binary)
    rows, topology = [], {}
    for kind in ("sparse", "common"):
        directory = root / kind
        folder = directory / "sources"
        folder.mkdir(parents=True)
        save(directory / "models.json", {})
        sources = {f"unit_{i:05}.py": i for i in range(args.files)}
        for path, i in sources.items():
            (folder / path).write_text(source(kind, i))
        store = directory / "store"

        def sync(label):
            row, packet = run(binary, directory, store, label, ["sync", str(folder)])
            graph = packet["graph"]
            assert graph["status"] in ("built", "current"), graph
            heads = {path: sha(folder / path) for path in sorted(sources)}
            extraction = inspect(store, packet["repository_id"], graph["episode"], sources, kind, heads)
            # A current episode reuses its graph, so its stored extraction
            # report describes an earlier build, not work performed this run.
            row.update(kind=kind, graph=graph,
                       extraction=extraction if graph["status"] == "built" else None,
                       source_manifest_sha256=hashlib.sha256(json.dumps(
                           heads, sort_keys=True).encode()).hexdigest())
            rows.append(row)
            save(root / "observations.json", rows)
            print(json.dumps(row), flush=True)
            return packet["repository_id"]

        repository = sync("cold")
        topology[kind] = dependencies(store, repository)
        for sample in range(args.samples):
            sync(f"unchanged-{sample}")
        for sample in range(args.samples):
            (folder / "unit_00000.py").write_text(source(kind, 0, revision=sample + 1))
            sync(f"edit-{sample}")
        (folder / "unit_00001.py").rename(folder / "renamed_00001.py")
        sources["renamed_00001.py"] = sources.pop("unit_00001.py")
        sync("rename")
        (folder / "unit_00002.py").unlink()
        sources.pop("unit_00002.py")
        sync("delete")
    assert sha(binary) == identity, "Binary changed during probe"
    save(root / "results.json", {"binary_sha256": identity, "harness_sha256": sha(Path(__file__)),
         "runner_sha256": sha(Path(__file__).with_name("large_repositories.py")),
         "files": args.files, "samples": args.samples, "rows": rows, "topology": topology,
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Sequential fresh CLI processes; warm OS cache; one cold admission per corpus; repeated unchanged and single-file edits; peak RSS per process",
         "limitations": "Synthetic Python only. Graph oracle checks exact nodes, source ownership and edge topology; it does not independently check every attribute or prove full-rebuild equivalence."})


if __name__ == "__main__":
    main()
