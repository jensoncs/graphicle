#!/usr/bin/env python3
"""Compare read-only context on a completed reviewed-memory qualification.

Each binary receives an isolated query store and prepares its graph cache
through pack. Stores are parked by rename so both arms use the same path.
Check complete packets and all source/graph/memory/dependency rows; setup and
verification are outside timing. No source or original fixture is mutated.
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

from compare_export_linking import canonical, connect, immutable_snapshot
from large_repositories import run, save, sha


def memory_snapshot(path):
    result = {}
    with connect(path) as db:
        for table in ("repo_memories", "repo_memory_revisions", "gf_dependencies", "gf_stale"):
            digest, count = hashlib.sha256(), 0
            for row in db.execute(f'SELECT * FROM "{table}" ORDER BY rowid'):
                digest.update(canonical(row) + b"\n")
                count += 1
            result[table] = {"rows": count, "sha256": digest.hexdigest()}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("Use positive samples (five or more for a performance claim)")
    binaries = {"baseline": args.baseline.resolve(strict=True), "candidate": args.candidate.resolve(strict=True)}
    identities = {arm: sha(binary) for arm, binary in binaries.items()}
    provenance = {name: sha(Path(__file__).with_name(name)) for name in
                  ("compare_memory_catalogue.py", "compare_export_linking.py", "large_repositories.py")}
    fixture = args.fixture.resolve(strict=True)
    fixture_hashes = {name: sha(fixture / name) for name in ("results.json", "notes.jsonl", "sync.json")}
    prior = json.loads((fixture / "results.json").read_text())
    assert len(prior["rows"]) == 50 and prior["reviewed_notes"] >= 40
    notes = [json.loads(line) for line in (fixture / "notes.jsonl").read_text().splitlines()]
    by_id = {note["id"]: note for note in notes}
    assert len(notes) == len(by_id) == prior["reviewed_notes"]
    repository = notes[0]["repository_id"]
    episode = json.loads((fixture / "sync.json").read_text())["graph"]["episode"]
    heads = prior["source_heads"]
    for path, expected in heads.items():
        assert sha(fixture / "sources" / path) == expected
    original = fixture / "store"
    original_database = original / "reviews.sqlite3"
    immutable, memories = immutable_snapshot(original_database), memory_snapshot(original_database)
    with connect(original_database) as db:
        assert dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (repository,))) == heads
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    store = root / "store"
    save(root / "models.json", {})

    def skip_build_caches(directory, names):
        directory = Path(directory)
        if directory == original:
            return set(names) & {"reviews.sqlite3", "reviews.sqlite3-wal", "reviews.sqlite3-shm"}
        if directory.parent == original / "gf-cache":
            return set(names) & {"resolution", "sources", "sources.lock"}
        return set()

    setup, caches = [], {}
    request_path = root / "prepare.request.json"
    save(request_path, {"question": "run_0", "budget": 1000})
    for arm, binary in binaries.items():
        shutil.copytree(original, store, ignore=skip_build_caches)
        with connect(original_database) as source, closing(sqlite3.connect(store / "reviews.sqlite3")) as destination:
            source.backup(destination)
        row, _ = run(binary, root, store, f"prepare-{arm}",
                     ["call", "pack", "--project", repository, "--arguments", str(request_path)])
        setup.append(row)
        graph = store / "gf-cache" / repository / "query" / f"{episode}.graph"
        assert graph.is_file() and graph.stat().st_size > 0
        caches[arm] = {"graph_path": str(graph.relative_to(store)), "graph_sha256": sha(graph),
                       "graph_bytes": graph.stat().st_size}
        assert immutable_snapshot(store / "reviews.sqlite3") == immutable
        assert memory_snapshot(store / "reviews.sqlite3") == memories
        store.rename(root / f"parked-{arm}")
    if identities["baseline"] != identities["candidate"]:
        assert caches["baseline"]["graph_sha256"] != caches["candidate"]["graph_sha256"], "Per-build caches were not prepared"
    save(root / "setup.json", {"calls": setup, "caches": caches})
    workloads = ["memory", "filtered", "graph", "disabled"]
    requests = {}
    for index in prior["probes"]:
        for workload in workloads:
            request = json.loads((fixture / f"{workload}-{index}-0.request.json").read_text())
            name = f"{workload}-{index}"
            save(root / f"{name}.request.json", request)
            requests[name] = (index, workload, request)
    rows, expected = [], {}

    def verify(packet, index, workload, request):
        assert packet["repository_id"] == repository
        assert packet["model_calls"] == 0 and packet["tokens"] <= request["max_tokens"]
        assert packet["freshness"] == prior["restoration_freshness"]["after"]
        found = notes[index]["id"] in {m["id"] for m in packet["memories"]}
        assert found == prior["summary"][f"{index}/{workload}"]["labelled_memory_found"]
        assert packet["coverage"]["stale_memories"] == 0
        omitted = max(len(notes) - 20000, 0) if workload in ("memory", "graph") else 0
        assert packet["coverage"]["omitted_reviewed_memories"] == omitted
        for evidence in packet["evidence"]:
            path = evidence["id"].removeprefix("file:")
            raw = (fixture / "sources" / path).read_bytes()
            assert hashlib.sha256(raw).hexdigest() == heads[path] == evidence["sha256"]
            assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
            if "include" in request:
                assert evidence["id"] in request["include"]
        for memory in packet["memories"]:
            note = by_id[memory["id"]]
            assert memory["revision"] == note["revision"] and memory["data"] == note["data"]
            assert memory["title"] == note["title"] and note["body"] in packet["markdown"]
            assert memory.get("stale") is not True
            if "include" in request:
                assert all("file:" + p in request["include"] for p in note["data"]["dependencies"])
        if workload == "disabled":
            assert not packet["memories"]

    for sample in range(args.samples + 1):
        arms = list(binaries)
        names = list(requests)
        if sample % 2:
            arms.reverse()
            names.reverse()
        for arm in arms:
            (root / f"parked-{arm}").rename(store)
            for name in names:
                index, workload, request = requests[name]
                row, packet = run(binaries[arm], root, store, f"{sample}-{arm}-{name}",
                                 ["call", "context", "--project", repository,
                                  "--arguments", str(root / f"{name}.request.json")])
                verify(packet, index, workload, request)
                if name not in expected:
                    expected[name] = packet
                    save(root / f"expected-{name}.json", packet)
                if packet != expected[name]:
                    save(root / "mismatch.json", {"name": name, "arm": arm, "sample": sample,
                                                  "expected": expected[name], "actual": packet})
                    raise AssertionError("Complete context packets differ; see mismatch.json")
                row.update(arm=arm, workload=workload, probe=index, sample=sample, warmup=sample == 0,
                           complete_packet_equal=True)
                rows.append(row)
                save(root / "observations.json", rows)
                print(json.dumps(row), flush=True)
            store.rename(root / f"parked-{arm}")
    for arm in binaries:
        parked = root / f"parked-{arm}"
        assert immutable_snapshot(parked / "reviews.sqlite3") == immutable
        assert memory_snapshot(parked / "reviews.sqlite3") == memories
        assert sha(parked / caches[arm]["graph_path"]) == caches[arm]["graph_sha256"]
    assert immutable_snapshot(original_database) == immutable
    assert memory_snapshot(original_database) == memories
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    assert fixture_hashes == {name: sha(fixture / name) for name in fixture_hashes}
    summary = {}
    for name, (index, workload, _) in requests.items():
        summary[name] = {}
        for arm in binaries:
            measured = [r for r in rows if r["probe"] == index and r["workload"] == workload
                        and r["arm"] == arm and not r["warmup"]]
            summary[name][arm] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                                  "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured)}
    save(root / "results.json", {"binary_sha256": identities, "harness_sha256": provenance,
         "fixture": str(fixture), "fixture_sha256": fixture_hashes, "reviewed_notes": len(notes),
         "source_files": len(heads), "probes": prior["probes"], "samples": args.samples,
         "setup": setup, "caches": caches, "rows": rows, "summary": summary,
         "immutable": immutable, "memory_integrity": memories, "platform": platform.platform(),
         "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Per-binary caches prepared through pack in disposable stores, then parked by rename for same-path requests. Fresh CLI/measurement processes, warm OS cache, alternating arm/workload order and one excluded warmup. Complete context packets, citations, notes, omissions and all source/graph/memory/dependency/stale rows checked.",
         "limitations": "Synthetic decisions over 40 tiny sources. Does not qualify combined large source/catalogue scale, independent real-user quality or warm-server latency. Existing oldest-memory omissions and the source-reversion freshness finding must remain identical."})


if __name__ == "__main__":
    main()
