#!/usr/bin/env python3
"""Qualify a large reviewed-memory catalogue through public local APIs.

Creates a disposable synthetic source project and authors every note through
the normal member-approved note operation. Stops the setup server before any
timed retrieval. This isolates catalogue size; it does not qualify combined
large-source/large-memory workloads or independent real-user recall.
"""
from __future__ import annotations

import argparse
import http.client
import json
import os
import platform
import signal
import socket
import sqlite3
import statistics
import subprocess
import time
from contextlib import closing
from pathlib import Path

from large_repositories import environment, run, save, sha


def author(binary, root, store, repository, heads, count):
    session = json.loads((store / "local-session.json").read_text())
    private = root / "http"
    private.mkdir(mode=0o700)
    address = private / "api.sock"

    class Connection(http.client.HTTPConnection):
        def connect(self):
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.settimeout(self.timeout)
            self.sock.connect(str(address))

    paths, notes = sorted(heads), []
    with (root / "authoring.log").open("wb") as log, (root / "notes.jsonl").open("w") as journal:
        child = subprocess.Popen([str(binary), "--data-dir", str(store), "--config", str(root / "models.json"),
                                  "serve", "--socket", str(address), "--no-worker"],
                                 cwd=root, env=environment(root), stdin=subprocess.DEVNULL,
                                 stdout=log, stderr=log, start_new_session=True)
        connection = Connection("localhost", timeout=60)
        try:
            deadline = time.monotonic() + 30
            while not address.exists():
                assert child.poll() is None, "Authoring server failed"
                assert time.monotonic() < deadline, "Authoring server startup timed out"
                time.sleep(0.05)
            for index in range(count):
                path = paths[index % len(paths)]
                key = f"cataloguekey{index:06}"
                arguments = {"kind": "decision",
                             "title": f"Catalogue decision {key}",
                             "body": f"For {key}, check {path} before changing its behavior. Preserve its local caller-to-helper relationship.",
                             "dependencies": [path]}
                connection.request("POST", f"/v1/workspaces/{session['workspace']}/repos/{repository}/commands",
                                   json.dumps({"operation": "note", "arguments": arguments}),
                                   {"Authorization": f"Bearer {session['capability']}", "Content-Type": "application/json"})
                response = connection.getresponse()
                raw = response.read()
                assert response.status == 200, (index, response.status, raw[:1000])
                note = json.loads(raw)
                assert note["state"] == "reviewed" and note["revision"] == 1
                assert note["title"] == arguments["title"] and note["body"] == arguments["body"]
                assert note["data"]["dependencies"] == {path: heads[path]}
                notes.append(note)
                journal.write(json.dumps(note) + "\n")
                if (index + 1) % 250 == 0 or index + 1 == count:
                    journal.flush()
                    print(json.dumps({"setup_notes_authored": index + 1, "target": count}), flush=True)
        finally:
            connection.close()
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
    assert len({note["id"] for note in notes}) == count
    return notes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--notes", type=int, default=25000)
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if not 40 <= args.notes <= 100000 or args.samples < 3:
        parser.error("Use 40–100000 notes and at least three samples")
    binary, root = args.binary.resolve(strict=True), args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    identity = sha(binary)
    provenance = {name: sha(Path(__file__).with_name(name)) for name in
                  ("qualify_memory_catalogue.py", "large_repositories.py")}
    folder, store = root / "sources", root / "store"
    folder.mkdir()
    save(root / "models.json", {})
    for i in range(40):
        (folder / f"module_{i:03}.py").write_text(
            f"def process_{i}(value):\n    return value + {i}\n\ndef run_{i}(value):\n    return process_{i}(value)\n")
    heads = {p.name: sha(p) for p in sorted(folder.iterdir())}
    row, synced = run(binary, root, store, "sync", ["sync", str(folder)])
    assert synced["files"] == 40 and synced["graph"]["status"] == "built"
    repository = synced["repository_id"]
    setup = [row]
    notes = author(binary, root, store, repository, heads, args.notes)
    by_id = {note["id"]: note for note in notes}
    with closing(sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True)) as db:
        assert db.execute("SELECT count(*) FROM repo_memories WHERE repository_id=? AND state='reviewed'", (repository,)).fetchone()[0] == args.notes
        assert db.execute("SELECT count(*) FROM repo_memory_revisions").fetchone()[0] == args.notes
        ordered = [row[0] for row in db.execute("SELECT id FROM repo_memories WHERE repository_id=? ORDER BY updated_at DESC,id", (repository,))]
    recalled = set(ordered[:20000])
    probes = [0, len(notes) // 2, len(notes) - 1]
    rows, expected = [], {}

    def call(label, operation, request):
        path = root / f"{label}.request.json"
        save(path, request)
        return run(binary, root, store, label, ["call", operation, "--project", repository, "--arguments", str(path)])

    def verify(packet, request, index, workload, stale=False):
        assert packet["repository_id"] == repository
        assert packet["tokens"] <= request["max_tokens"] and packet["model_calls"] == 0
        for evidence in packet["evidence"]:
            path = evidence["id"].removeprefix("file:")
            raw = (folder / path).read_bytes()
            assert sha(folder / path) == evidence["sha256"]
            assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
            if "include" in request:
                assert evidence["id"] in request["include"]
        found = notes[index]["id"] in {m["id"] for m in packet["memories"]}
        if stale or workload == "disabled":
            assert not found, (index, workload, "withheld note returned")
        elif workload == "filtered" or notes[index]["id"] in recalled:
            assert found, (index, workload, "reviewed note missing")
        for memory in packet["memories"]:
            note = by_id[memory["id"]]
            assert memory["revision"] == note["revision"] and memory["data"] == note["data"]
            assert memory["title"] == note["title"] and note["body"] in packet["markdown"]
            assert memory.get("stale") is not True
            if "include" in request:
                assert all("file:" + p in request["include"] for p in note["data"]["dependencies"])
        if workload == "disabled":
            assert not packet["memories"]
        if not stale and workload != "disabled":
            omitted = max(args.notes - 20000, 0) if workload != "filtered" else 0
            assert packet["coverage"]["omitted_reviewed_memories"] == omitted, packet["coverage"]
        return found

    def question(index, workload):
        request = {"question": f"What decision was recorded for cataloguekey{index:06}?", "max_tokens": 3000,
                   "graph_retrieval": workload == "graph", "memory_retrieval": workload != "disabled"}
        if workload == "filtered":
            request["include"] = ["file:" + next(iter(notes[index]["data"]["dependencies"]))]
        return request

    def record(row, packet, index, workload, found, sample=None):
        row.update(index=index, workload=workload, sample=sample, warmup=sample == 0,
                   labelled_memory_found=found, tokens=packet["tokens"], memories=len(packet["memories"]),
                   coverage=packet.get("coverage"), mechanisms=packet.get("mechanisms"),
                   passage_index=packet.get("passage_index"))
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps({k: row[k] for k in ("label", "seconds", "peak_rss_bytes", "labelled_memory_found", "coverage")}), flush=True)

    for sample in range(args.samples + 1):
        workloads = ["memory", "filtered", "graph", "disabled"]
        if sample % 2:
            workloads.reverse()
        for index in probes:
            for workload in workloads:
                request = question(index, workload)
                row, packet = call(f"{workload}-{index}-{sample}", "context", request)
                found = verify(packet, request, index, workload)
                key = (index, workload)
                if key in expected:
                    assert packet == expected[key], (key, "repeated packet changed")
                else:
                    expected[key] = packet
                record(row, packet, index, workload, found, sample)
    # The full-file dependency changes through the normal sync path. Restoring
    # the original bytes makes the same authored revision current again.
    index = probes[-1]
    path = next(iter(notes[index]["data"]["dependencies"]))
    original = (folder / path).read_bytes()
    dependents = sum(path in note["data"]["dependencies"] for note in notes)
    (folder / path).write_bytes(original + b"\n# Changed source invalidates its reviewed decisions.\n")
    row, _ = run(binary, root, store, "source_changed", ["sync", str(folder), "--no-graph"])
    setup.append(row)
    request = question(index, "memory")
    row, packet = call("stale-memory", "context", request)
    found = verify(packet, request, index, "memory", stale=True)
    assert packet["coverage"]["stale_memories"] == dependents, packet["coverage"]
    record(row, packet, index, "stale", found)
    (folder / path).write_bytes(original)
    row, _ = run(binary, root, store, "source_restored", ["sync", str(folder), "--no-graph"])
    setup.append(row)
    row, packet = call("restored-memory", "context", request)
    found = verify(packet, request, index, "memory")
    # The event-driven freshness ledger may retain invalidations after source
    # bytes revert even though current dependency hashes permit the note. Keep
    # this visible as a finding; all other fields must match the original read.
    before = expected[(index, "memory")]
    restoration = {"before": before.get("freshness"), "after": packet.get("freshness"),
                   "freshness_ledger_differs": before.get("freshness") != packet.get("freshness")}
    assert {k: v for k, v in packet.items() if k != "freshness"} == {
        k: v for k, v in before.items() if k != "freshness"
    }, "Restoration changed fields other than the recorded freshness ledger"
    record(row, packet, index, "restored", found)
    with closing(sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True)) as db:
        assert list(db.execute("pragma quick_check")) == [("ok",)]
        assert dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (repository,))) == heads
        assert db.execute("SELECT count(*) FROM repo_memories WHERE repository_id=? AND state='reviewed' AND revision=1", (repository,)).fetchone()[0] == args.notes
    assert sha(binary) == identity
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in provenance}
    summary = {}
    for index, workload in expected:
        selected = [r for r in rows if r["index"] == index and r["workload"] == workload and r["sample"] not in (None, 0)]
        summary[f"{index}/{workload}"] = {"median_seconds": statistics.median(r["seconds"] for r in selected),
                                         "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in selected),
                                         "labelled_memory_found": all(r["labelled_memory_found"] for r in selected)}
    save(root / "results.json", {"binary_sha256": identity, "harness_sha256": provenance,
         "reviewed_notes": args.notes, "source_files": len(heads), "source_heads": heads,
         "notes_journal_sha256": sha(root / "notes.jsonl"), "notes_bytes": (root / "notes.jsonl").stat().st_size,
         "setup": setup, "probes": probes, "samples": args.samples, "rows": rows, "summary": summary,
         "restoration_freshness": restoration,
         "source_change_dependents": dependents, "platform": platform.platform(), "cpu_count": os.cpu_count(),
         "method": "All notes authored with the public note API and original file dependencies. Setup HTTP server stopped before fresh CLI/measurement-worker retrievals. One excluded warmup per workload/probe, alternating workload order, complete repeated packet equality, exact source and note contents, token budgets and source-change/restoration checks.",
         "limitations": "Synthetic authored decisions over 40 tiny Python files. Isolates memory catalogue size, not simultaneous large source/graph size or independent real-user relevance. Ordinary recall is limited to the newest 20000 eligible memories; older-note retrieval is recorded rather than assumed. No observer/model authoring, cross-repository permissions or concurrent writers are measured."})


if __name__ == "__main__":
    main()
