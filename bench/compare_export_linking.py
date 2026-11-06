#!/usr/bin/env python3
"""Compare export-index rebuilding on a completed workspace qualification.

Clone only a synthetic fixture, keep its source objects and query caches, and
reset the same disposable SQLite store before each arm. Delete only derived
cross-link state to force graph.link to rebuild indexes and relink imports.
All setup, database snapshots and correctness checks are outside timed calls.
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
from contextlib import closing
from pathlib import Path

from large_repositories import run, save, sha


IMMUTABLE = ("repo_projects", "repo_heads", "repo_snapshots", "repo_objects",
             "gf_episodes", "gf_nodes", "gf_edges", "gf_repo_access")
CROSS = ("gf_cross_link_state", "gf_cross_edges", "gf_cross_unresolved", "gf_cross_packages")
JSON_COLUMNS = {"export_index", "candidates", "exporter_episodes"}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"),
                      default=lambda value: {"bytes_hex": value.hex()}).encode()


def connect(path):
    return closing(sqlite3.connect(f"file:{path}?mode=ro", uri=True))


def immutable_snapshot(path):
    """Stream complete rows, including all graph attributes and source bytes."""
    result = {}
    with connect(path) as db:
        for table in IMMUTABLE:
            digest, count = hashlib.sha256(), 0
            for row in db.execute(f'SELECT * FROM "{table}" ORDER BY rowid'):
                digest.update(canonical(row) + b"\n")
                count += 1
            result[table] = {"rows": count, "sha256": digest.hexdigest()}
    return result


def cross_snapshot(path):
    result = {}
    with connect(path) as db:
        for table in CROSS:
            cursor = db.execute(f'SELECT * FROM "{table}"')
            columns = [entry[0] for entry in cursor.description]
            rows = []
            for values in cursor:
                row = dict(zip(columns, values))
                # Rebuilding sets the wall-clock creation time of link-state
                # rows. Compare all other fields, including history and IDs.
                row.pop("created_at")
                for name in JSON_COLUMNS & row.keys():
                    if row[name] is not None:
                        row[name] = json.loads(row[name])
                rows.append(row)
            result[table] = sorted(rows, key=canonical)
    return result


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
                  ("compare_export_linking.py", "large_repositories.py")}
    fixture = args.fixture.resolve(strict=True)
    prior = json.loads((fixture / "results.json").read_text())
    fixture_hash = sha(fixture / "results.json")
    projects = prior["projects"]
    assert len(projects) == 10 and len(prior["rows"]) == 73, "Use a completed workspace qualification"
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    save(root / "link.request.json", {})
    original = fixture / "store"
    store = root / "store"

    def skip_caches(directory, names):
        directory = Path(directory)
        if directory == original:
            # Use SQLite backup below so any committed WAL pages are included.
            return set(names) & {"reviews.sqlite3", "reviews.sqlite3-wal", "reviews.sqlite3-shm"}
        if directory.parent == original / "gf-cache":
            return set(names) & {"resolution", "sources", "sources.lock"}
        return set()

    shutil.copytree(original, store, ignore=skip_caches)
    seed = root / "seed.sqlite3"
    with connect(original / "reviews.sqlite3") as source, closing(sqlite3.connect(seed)) as target:
        source.backup(target)
    expected_immutable = immutable_snapshot(seed)
    expected_cross = cross_snapshot(seed)
    save(root / "expected-immutable.json", expected_immutable)
    save(root / "expected-cross.json", expected_cross)
    assert len(expected_cross["gf_cross_link_state"]) == len(projects)
    with connect(seed) as db:
        for project in projects:
            identifier, episode = project["id"], project["episode"]
            heads = dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (identifier,)))
            assert heads == project["heads"], identifier
            attributes = json.loads(db.execute(
                "SELECT attributes FROM gf_episodes WHERE repository_id=? AND episode=?",
                (identifier, episode)).fetchone()[0])
            assert attributes["source_heads"] == heads, identifier
    indexes = {row["repository_id"]: row["export_index"] for row in expected_cross["gf_cross_link_state"]}
    assert all(indexes[p["id"]]["repo"] == p["id"] and
               indexes[p["id"]]["episode"] == p["episode"] for p in projects)
    rows, packets = [], {}
    database = store / "reviews.sqlite3"
    for sample in range(args.samples + 1):
        order = list(binaries)
        if sample % 2:
            order.reverse()
        for arm in order:
            for suffix in ("-wal", "-shm"):
                Path(str(database) + suffix).unlink(missing_ok=True)
            shutil.copyfile(seed, database)
            with closing(sqlite3.connect(database)) as db:
                db.execute("DELETE FROM gf_cross_link_state")
                db.commit()
                assert db.execute("SELECT count(*) FROM gf_cross_link_state").fetchone()[0] == 0
            for workload in ("rebuild", "cached"):
                label = f"{sample}-{arm}-{workload}"
                row, packet = run(binaries[arm], root, store, label,
                                  ["call", "graph.link", "--arguments", str(root / "link.request.json")])
                assert packet["persisted"] and packet["hidden_repositories"] == 0
                assert not packet["unprojected_repositories"]
                assert {(p["id"], p["episode"]) for p in packet["repositories"]} == {
                    (p["id"], p["episode"]) for p in projects}
                assert all(p["current"] for p in packet["repositories"])
                assert set(packet["relinked"] if workload == "rebuild" else packet["reused"]) == set(indexes)
                assert not packet["reused" if workload == "rebuild" else "relinked"]
                if workload not in packets:
                    packets[workload] = packet
                    save(root / f"expected-{workload}.json", packet)
                assert packet == packets[workload], f"Complete packet differs: {label}"
                actual = cross_snapshot(database)
                if actual != expected_cross:
                    save(root / f"{label}-mismatch.json", actual)
                    raise AssertionError(f"Export indexes, cross edges or unresolved imports differ: {label}")
                assert immutable_snapshot(database) == expected_immutable, f"Sources, episodes, graph or access changed: {label}"
                row.update(arm=arm, sample=sample, warmup=sample == 0, workload=workload,
                           complete_packet_equal=True, complete_cross_state_equal=True,
                           immutable_source_graph_access_equal=True)
                rows.append(row)
                save(root / "observations.json", rows)
                print(json.dumps(row), flush=True)
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    assert fixture_hash == sha(fixture / "results.json")
    assert immutable_snapshot(original / "reviews.sqlite3") == expected_immutable
    assert cross_snapshot(original / "reviews.sqlite3") == expected_cross
    summary = {}
    for workload in ("rebuild", "cached"):
        summary[workload] = {}
        for arm in binaries:
            measured = [row for row in rows if row["arm"] == arm and row["workload"] == workload and not row["warmup"]]
            summary[workload][arm] = {"median_seconds": statistics.median(row["seconds"] for row in measured),
                                     "max_peak_rss_bytes": max(row["peak_rss_bytes"] for row in measured)}
    save(root / "results.json", {
        "binary_sha256": identities, "harness_sha256": provenance,
        "fixture": str(fixture), "fixture_result_sha256": fixture_hash,
        "generated_files": prior["generated_files"],
        "source_files": expected_immutable["repo_heads"]["rows"],
        "project_count": len(projects), "samples": args.samples,
        "expected_cross_sha256": sha(root / "expected-cross.json"),
        "immutable": expected_immutable, "rows": rows, "summary": summary,
        "platform": platform.platform(), "cpu_count": os.cpu_count(),
        "method": "Same disposable store path restored from SQLite backup per arm; derived cross-link state deleted during untimed setup. Sequential fresh CLI calls with warm OS cache, alternating arm order, one excluded warmup per arm. Compare complete packets and export indexes, all cross rows except creation timestamps, and all source/graph/access rows. Cached link immediately follows each rebuild.",
        "limitations": "Synthetic Python/TypeScript/Go workspace with the Python library already deleted. Qualifies export-index rebuilding and cached relinking, not cold parsing, admission, arbitrary import forms, concurrent writers, warm-server requests or real repository SLOs."})


if __name__ == "__main__":
    main()
