#!/usr/bin/env python3
"""Qualify a fixed public Babel checkout using a disposable copy and real CLI.

No package scripts or downloaded source are executed. Source-reviewed probes
supplement complete source-head checks and incremental/fresh-cache equality.
This is an observational qualification, not a speedup benchmark or a complete
JavaScript/TypeScript semantic oracle. Use --smoke before the full checkout.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
import os
import platform
import re
import shutil
import sqlite3
import subprocess
from pathlib import Path

from compare_graph_scaling import copy_store, snapshot
from large_repositories import environment, run, save, sha


CLONE = "packages/babel-types/src/clone/cloneNode.ts"
ES3 = "packages/babel-types/src/validators/isValidES3Identifier.ts"
VALID = "packages/babel-types/src/validators/isValidIdentifier.ts"
DEEP = "packages/babel-types/src/modifications/removePropertiesDeep.ts"
FAST = "packages/babel-types/src/traverse/traverseFast.ts"


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def prepare(checkout, root, smoke):
    def git(*args):
        return subprocess.check_output(["git", *args], cwd=checkout)

    assert not git("status", "--porcelain"), "Checkout must be clean"
    revision = git("rev-parse", "HEAD").decode().strip()
    tracked = git("ls-files", "-z").decode().split("\0")[:-1]
    folder = root / "sources"
    folder.mkdir()
    manifest, excluded = {}, []
    for name in tracked:
        path = Path(name)
        if any(part.startswith(".env") or part == "secrets" for part in path.parts):
            excluded.append({"path": name, "reason": "harness_sensitive_path"})
            continue
        if smoke and not name.startswith("packages/babel-types/"):
            continue
        original = checkout / name
        if original.is_symlink() or not original.is_file():
            excluded.append({"path": name, "reason": "harness_nonregular"})
            continue
        target = folder / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(original, target)
        manifest[name] = {"sha256": sha(target), "bytes": target.stat().st_size}
    assert revision == git("rev-parse", "HEAD").decode().strip()
    assert not git("status", "--porcelain")
    # Preserve tracked-file admission semantics without shared git metadata,
    # hooks, commits, remote URLs, credentials or executing repository code.
    env = environment(root)
    subprocess.run(["git", "init", "-q", str(folder)], env=env, check=True)
    subprocess.run(["git", "-C", str(folder), "add", "-f", "--all"], env=env, check=True)
    save(root / "original-manifest.json", manifest)
    corpus = {"repository": "https://github.com/babel/babel", "commit": revision,
              "tracked_files": len(tracked), "copied_files": len(manifest),
              "bytes": sum(row["bytes"] for row in manifest.values()),
              "extensions": dict(collections.Counter(Path(p).suffix for p in manifest)),
              "manifest_sha256": digest(manifest), "harness_excluded": excluded,
              "scope": "packages/babel-types only" if smoke else "all tracked regular files"}
    save(root / "corpus.json", corpus)
    return folder, corpus, manifest


def inspect(store, repository, episode, folder, source_manifest):
    with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
        heads = dict(db.execute("SELECT path,sha256 FROM repo_heads WHERE repository_id=?", (repository,)))
        for path, value in heads.items():
            assert path in source_manifest, ("unexpected source", path)
            assert sha(folder / path) == value, ("source head mismatch", path)
        attributes = json.loads(db.execute(
            "SELECT attributes FROM gf_episodes WHERE repository_id=? AND episode=?",
            (repository, episode)).fetchone()[0])
        assert attributes["source_heads"] == heads, "Graph source coverage mismatch"
        nodes = {identifier: json.loads(raw) for identifier, raw in db.execute(
            "SELECT node_id,attributes FROM gf_nodes WHERE repository_id=? AND valid_to_episode IS NULL",
            (repository,))}
        calls = set(db.execute(
            "SELECT source,target FROM gf_edges WHERE repository_id=? AND relation='calls' AND valid_to_episode IS NULL",
            (repository,)))
        unresolved = sorted(raw for raw, in db.execute(
            "SELECT call FROM gf_unresolved_calls WHERE repository_id=? AND episode=?", (repository, episode)))
    canonical = snapshot(store, repository, episode)
    return {"files": len(heads), "source_heads_sha256": digest(heads),
            "nodes": len(nodes), "call_edges": len(calls), "unresolved_calls": len(unresolved),
            "unresolved_calls_sha256": digest([json.loads(raw) for raw in unresolved]),
            "graph_sha256": hashlib.sha256(canonical).hexdigest(),
            "extraction": attributes["extraction_run"]}, heads, nodes, calls


def probe(nodes, calls, folder, helper, deleted):
    """Expected call sites were reviewed in the pinned upstream source."""
    pairs = [(CLONE, "cloneNode", CLONE, helper),
             (CLONE, "cloneIfNode", CLONE, helper),
             (DEEP, "removePropertiesDeep", FAST, "traverseFast")]
    if not deleted:
        pairs.append((ES3, "isValidES3Identifier", VALID, "isValidIdentifier"))
    matches = []
    for source_path, source_name, target_path, target_name in pairs:
        def find(path, name):
            result = [identifier for identifier, node in nodes.items()
                      if node.get("source_file") == path and node.get("label") == name + "()"]
            assert len(result) == 1, (path, name, result)
            return result[0]

        source, target = find(source_path, source_name), find(target_path, target_name)
        # Bind each independently reviewed expectation to the actual source.
        text = (folder / source_path).read_text()
        assert re.search(r"function\s+" + source_name + r"\b", text)
        assert re.search(r"\b" + target_name + r"\(", text)
        assert (source, target) in calls, ("reviewed call missing", source, target)
        matches.append({"source": source, "target": target, "source_path": source_path,
                        "target_path": target_path})
    if deleted:
        assert not any(n.get("source_file") == ES3 for n in nodes.values()), "Deleted source retained"
    if helper != "cloneNodeInternal":
        assert not any(n.get("source_file") == CLONE and n.get("label") == "cloneNodeInternal()"
                       for n in nodes.values()), "Renamed declaration retained"
    return matches


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    binary, root = args.binary.resolve(strict=True), args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    identity = sha(binary)
    save(root / "models.json", {})
    folder, corpus, manifest = prepare(args.checkout.resolve(strict=True), root, args.smoke)
    store, seed = root / "store", root / "admitted-seed"
    rows = []

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    row, admitted = run(binary, root, store, "admit", ["sync", str(folder), "--no-graph"])
    repository = admitted["repository_id"]
    row["admission"] = admitted
    record(row)
    copy_store(store, seed)
    original = (folder / CLONE).read_text()
    helper, deleted = "cloneNodeInternal", False
    admission_heads = None

    def sync(label, unchanged=False):
        nonlocal admission_heads
        row, packet = run(binary, root, store, label, ["sync", str(folder)])
        record({**row, "graph": packet["graph"], "verification": "pending"})
        assert packet["repository_id"] == repository
        assert packet["graph"]["status"] == ("current" if unchanged else "built"), packet
        checked, heads, nodes, calls = inspect(store, repository, packet["graph"]["episode"], folder, manifest)
        if admission_heads is None:
            admission_heads = heads
            missing = sorted(set(manifest) - heads.keys())
            with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
                metadata = json.loads(db.execute(
                    "SELECT metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?",
                    (repository, packet["snapshot"])).fetchone()[0])
            save(root / "admission-coverage.json", {"missing_paths": missing, "metadata": metadata,
                 "copied_files": len(manifest), "admitted_files": len(heads),
                 "admitted_bytes": sum((folder / p).stat().st_size for p in heads)})
        else:
            assert set(heads) == set(admission_heads) - ({ES3} if deleted else set())
        checked["reviewed_calls"] = probe(nodes, calls, folder, helper, deleted)
        if unchanged:
            checked["extraction"] = None  # Persisted counters describe the previous build.
        rows[-1].update(verification="passed", checks=checked)
        save(root / "observations.json", rows)
        return packet, checked

    cold, cold_check = sync("cold_graph")
    for i in range(3):
        _, check = sync(f"unchanged-{i}", unchanged=True)
        assert check["graph_sha256"] == cold_check["graph_sha256"]
    for i in range(3):
        helper = f"cloneNodeInternalGraphicleProbe{i}"
        assert original.count("cloneNodeInternal") == 3
        (folder / CLONE).write_text(original.replace("cloneNodeInternal", helper))
        sync(f"edit-{i}")
    (folder / ES3).unlink()
    deleted = True
    _, final = sync("delete")
    store.rename(root / "incremental-store")
    copy_store(seed, store)
    fresh, rebuilt = sync("fresh_rebuild")
    assert rebuilt["graph_sha256"] == final["graph_sha256"], "Fresh graph differs"
    assert rebuilt["unresolved_calls_sha256"] == final["unresolved_calls_sha256"], "Unresolved calls differ"
    for label, question, include in [
        ("clone", "How does cloneNode control deep cloning and location properties?", None),
        ("filtered", "How does cloneNode control deep cloning and location properties?", ["file:" + CLONE]),
        ("deep", "How does removePropertiesDeep traverse the tree?", None),
    ]:
        request = {"repository_id": repository, "question": question, "max_tokens": 4000,
                   "graph_retrieval": True, "memory_retrieval": False}
        if include:
            request["include"] = include
        request_path = root / f"context-{label}.request.json"
        save(request_path, request)
        row, packet = run(binary, root, store, f"context-{label}",
                          ["call", "context", "--project", repository, "--arguments", str(request_path)])
        assert packet["tokens"] <= request["max_tokens"] and packet["model_calls"] == 0
        expected = DEEP if label == "deep" else CLONE
        assert any(e["id"] == "file:" + expected for e in packet["evidence"]), (label, "source missing")
        for evidence in packet["evidence"]:
            path = evidence["id"].removeprefix("file:")
            raw = (folder / path).read_bytes()
            assert sha(folder / path) == evidence["sha256"]
            assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
            if include:
                assert evidence["id"] in include
        record({**row, "tokens": packet["tokens"], "evidence": len(packet["evidence"]),
                "mechanisms": packet.get("mechanisms"), "limitations": packet.get("limitations")})
    for i, pair in enumerate(rebuilt["reviewed_calls"]):
        request = {"repository_id": repository, "tool": "get_neighbors", "node_id": pair["source"],
                   "relation_filter": "calls"}
        request_path = root / f"neighbors-{i}.request.json"
        save(request_path, request)
        row, packet = run(binary, root, store, f"neighbors-{i}",
                          ["call", "graph.navigate", "--project", repository, "--arguments", str(request_path)])
        assert packet["episode"] == fresh["graph"]["episode"] == packet["current_episode"]
        assert {pair["source"], pair["target"]} <= {n["node_id"] for n in packet["nodes"]}
        for node in packet["nodes"]:
            assert node["checked"], node
            raw = (folder / node["relpath"]).read_bytes()
            assert sha(folder / node["relpath"]) == node["sha256"]
            assert 0 <= node["byte_start"] < node["byte_end"] <= len(raw)
        record({**row, "nodes": len(packet["nodes"]), "reviewed_pair": pair})
    assert sha(binary) == identity
    save(root / "results.json", {"binary_sha256": identity, "corpus": corpus, "rows": rows,
         "fresh_cache_equal": True, "unresolved_calls_equal": True, "platform": platform.platform(),
         "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "harness_sha256": {name: sha(Path(__file__).with_name(name)) for name in
                            ("qualify_babel.py", "compare_graph_scaling.py", "large_repositories.py")},
         "method": "Sequential fresh CLI processes; warm OS cache; preparation and verification excluded; source admission separated from graph build; same-path fresh rebuild",
         "limitations": "Real repository dominated by parser/test fixtures, including intentionally invalid syntax. Reviewed call probes are not a complete language oracle. No model, large memory catalogue, cross-repository or warm-server qualification. Fresh-cache equality uses the same extractor."})


if __name__ == "__main__":
    main()
