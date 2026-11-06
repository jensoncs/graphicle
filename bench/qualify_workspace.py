#!/usr/bin/env python3
"""Qualify ten Python/TypeScript/Go repositories through the local CLI.

Creates only synthetic public source in a new disposable directory. Reuses
workspace context and import linkage; checks citations, source coverage,
freshness and revoked access. This is feasibility evidence, not A/B timing
or a proof of language completeness. Use --units for uneven project sizes or
--files-per-project for exact equal sizes, including manifests and probes.
Run without competing benchmarks.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import http.client
import json
import os
import platform
import signal
import socket
import sqlite3
import subprocess
import time
from pathlib import Path

from compare_graph_scaling import snapshot
from large_repositories import environment, run, save, sha


WEIGHTS = (1, 1, 1, 2, 3, 4, 5, 6, 8, 9)
LANGUAGES = ("py", "ts", "go", "py", "ts", "go", "py", "ts", "go", "py")
LIBRARY_PATHS = ("moneylib/rounding.py", "src/core.ts", "core/core.go")
LIBRARY_SOURCES = (
    "def quantize_cents(value):\n    scaled = int(value * 100 + 0.5)\n    return scaled / 100\n",
    "export function quantize_cents(value: number): number {\n  return Math.round(value * 100) / 100;\n}\n",
    "package core\n\nfunc QuantizeCents(value float64) float64 {\n return float64(int(value * 100 + 0.5)) / 100\n}\n",
)


def probe_files(index, language):
    library = index < 3
    if language == "py":
        manifest = '[project]\nname = "' + ("moneylib" if library else f"shop{index}") + '"\nversion = "1.0.0"\n'
        if not library:
            manifest += 'dependencies = ["moneylib>=1.0"]\n'
        files = {"pyproject.toml": manifest}
        if library:
            files.update({"moneylib/__init__.py": "", LIBRARY_PATHS[0]: LIBRARY_SOURCES[0]})
        else:
            files["invoice.py"] = (
                "from moneylib.rounding import quantize_cents\n\n"
                f"def settle_invoice_{index}(total, tax):\n"
                f'    """Settle the cobalt{index} invoice: the amount due with tax."""\n'
                "    return quantize_cents(total + tax)\n")
    elif language == "ts":
        manifest = {"name": "@scale/moneylib" if library else f"@scale/shop{index}", "version": "1.0.0"}
        manifest.update({"exports": {"./core": "./src/core.ts"}} if library else
                        {"dependencies": {"@scale/moneylib": "^1.0.0"}})
        files = {"package.json": json.dumps(manifest) + "\n"}
        if library:
            files[LIBRARY_PATHS[1]] = LIBRARY_SOURCES[1]
        else:
            files["src/invoice.ts"] = (
                'import { quantize_cents } from "@scale/moneylib/core";\n\n'
                f"/** Settle the cobalt{index} invoice: the amount due with tax. */\n"
                f"export function settle_invoice_{index}(total: number, tax: number): number {{\n"
                "  return quantize_cents(total + tax);\n}\n")
    else:
        manifest = f"module example.com/{'moneylib' if library else f'shop{index}'}\n\ngo 1.22\n"
        if not library:
            manifest += "\nrequire example.com/moneylib v1.0.0\n"
        files = {"go.mod": manifest}
        if library:
            files[LIBRARY_PATHS[2]] = LIBRARY_SOURCES[2]
        else:
            files["invoice/invoice.go"] = (
                'package invoice\n\nimport "example.com/moneylib/core"\n\n'
                f"// SettleInvoice{index} settles the cobalt{index} invoice: the amount due with tax.\n"
                f"func SettleInvoice{index}(total, tax float64) float64 {{\n"
                " return core.QuantizeCents(total + tax)\n}\n")
    return files


def unit(language, index):
    directory = "generated" if index % 10 == 0 else "units"
    name, caller = f"work_{index:05}", f"run_{index:05}"
    if language == "py":
        path = f"{directory}/unit_{index:05}.py"
        text = f"def {name}(value):\n    return value + {index}\n\ndef {caller}(value):\n    return {name}(value)\n"
    elif language == "ts":
        path = f"{directory}/unit_{index:05}.ts"
        text = f"function {name}(value: number): number {{ return value + {index}; }}\nexport function {caller}(value: number): number {{ return {name}(value); }}\n"
    else:
        # Unique declarations share multi-file packages. Each twenty-file
        # group has eighteen ordinary and two generated sources in separate
        # package directories; the generated sources remain admitted.
        group = index // 20
        path = f"{directory}/package_{group:05}/unit_{index:05}.go"
        text = f"package unit{group:05}\n\nfunc {name}(value int) int {{ return value + {index} }}\nfunc {caller}(value int) int {{ return {name}(value) }}\n"
    return path, text, name, caller


def inspect(store, project):
    """Check every admitted source and every generated local call independently."""
    with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
        attributes = json.loads(db.execute(
            "SELECT attributes FROM gf_episodes WHERE repository_id=? AND episode=?",
            (project["id"], project["episode"])).fetchone()[0])
        assert attributes["source_heads"] == project["heads"], "Source coverage differs"
        by_path = collections.defaultdict(list)
        nodes = list(db.execute("SELECT node_id,attributes FROM gf_nodes WHERE repository_id=? AND valid_to_episode IS NULL", (project["id"],)))
        for identifier, value in nodes:
            node = json.loads(value)
            by_path[node.get("source_file")].append((identifier, node))
        edges = list(db.execute("SELECT source,target,relation FROM gf_edges WHERE repository_id=? AND valid_to_episode IS NULL", (project["id"],)))
        calls = collections.defaultdict(list)
        for source, target, relation in edges:
            if relation == "calls":
                calls[source].append(target)
        generated_identifiers = set()
        for path, name, caller in project["units"]:
            owned = by_path[path]
            targets = [identifier for identifier, node in owned if node.get("label") == name + "()"]
            callers = [identifier for identifier, node in owned if node.get("label") == caller + "()"]
            assert len(targets) == len(callers) == 1, (path, owned)
            assert calls[callers[0]] == targets, (path, "missing, duplicate or incorrect local call")
            assert not calls[targets[0]], (path, "unexpected helper call")
            generated_identifiers.update(targets + callers)
        unresolved = [json.loads(row[0]) for row in db.execute(
            "SELECT call FROM gf_unresolved_calls WHERE repository_id=? AND episode=?",
            (project["id"], project["episode"]))]
        assert attributes.get("unresolved_calls_count", 0) == len(unresolved)
        generated_paths = {path for path, _, _ in project["units"]}
        # Go's compiler diagnostics can name only a source location, without
        # a caller ID. Source ownership still catches unresolved filler calls.
        assert not any(call.get("caller") in generated_identifiers or call.get("source_file") in generated_paths
                       for call in unresolved), "Generated local call also reported unresolved"
        canonical_unresolved = sorted(json.dumps(call, sort_keys=True, separators=(",", ":")) for call in unresolved)
        assert attributes["extraction_run"]["resolution"]["fallback"] is None
    return {"nodes": len(nodes), "edges": len(edges), "generated_calls_checked": len(project["units"]),
            "unresolved_calls": len(unresolved),
            "unresolved_calls_sha256": hashlib.sha256(json.dumps(canonical_unresolved).encode()).hexdigest(),
            "extraction": attributes["extraction_run"]}


def grant_reader(binary, root, store):
    """Use the existing HTTP grant API, with credentials confined to the fixture."""
    session = json.loads((store / "local-session.json").read_text())
    private = root / "http"
    private.mkdir(mode=0o700)
    address = private / "api.sock"

    class Connection(http.client.HTTPConnection):
        def connect(self):
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.settimeout(self.timeout)
            self.sock.connect(str(address))

    with (root / "grant-server.log").open("wb") as log:
        child = subprocess.Popen([str(binary), "--data-dir", str(store), "--config", str(root / "models.json"),
                                  "serve", "--socket", str(address), "--no-worker"],
                                 cwd=root, env=environment(root), stdin=subprocess.DEVNULL,
                                 stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic() + 30
            while not address.exists():
                assert child.poll() is None, "Grant server failed; see grant-server.log"
                assert time.monotonic() < deadline, "Grant server timed out"
                time.sleep(0.05)
            connection = Connection("localhost", timeout=30)
            connection.request("POST", f"/api/workspaces/{session['workspace']}/credentials",
                               json.dumps({"role": "reader", "label": "synthetic scale reader"}),
                               {"Authorization": f"Bearer {session['capability']}", "Content-Type": "application/json"})
            response = connection.getresponse()
            assert response.status == 200, f"Grant API status {response.status}"
            granted = json.loads(response.read())
            connection.close()
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
    return granted


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    size = parser.add_mutually_exclusive_group()
    size.add_argument("--units", type=int, default=2500, help="40 times this many generated code files, plus manifests/probes")
    size.add_argument("--files-per-project", type=int, help="Exact files per project, including manifests/probes; 100000 gives one million in total")
    parser.add_argument("--samples", type=int, default=3)
    args = parser.parse_args()
    if args.units < 1 or args.samples < 3:
        parser.error("Use positive units and at least three samples")
    if args.files_per_project is not None and args.files_per_project < 4:
        parser.error("Use at least four files per project")
    binary, root = args.binary.resolve(strict=True), args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    store = root / "store"
    identity = sha(binary)
    harness = {name: sha(Path(__file__).with_name(name)) for name in
               ("qualify_workspace.py", "large_repositories.py", "compare_graph_scaling.py")}
    projects, rows = [], []

    def record(row):
        rows.append(row)
        save(root / "observations.json", rows)
        print(json.dumps(row), flush=True)

    def call(label, operation, project, arguments):
        path = root / f"{label}.request.json"
        save(path, arguments)
        return run(binary, root, store, label, ["call", operation, "--project", project["id"], "--arguments", str(path)])

    def sync(project, label, no_graph=False):
        row, packet = run(binary, root, store, label, ["sync", str(project["folder"]), *(["--no-graph"] if no_graph else [])])
        assert project.get("id", packet["repository_id"]) == packet["repository_id"]
        project["id"] = packet["repository_id"]
        project["heads"] = {p: sha(project["folder"] / p) for p in sorted(project["paths"])}
        if not no_graph:
            project["episode"] = packet["graph"]["episode"]
            row.update(graph=packet["graph"], **inspect(store, project))
            if packet["graph"]["status"] == "current":
                row["extraction"] = None  # Recorded work belongs to the older build.
            row["graph_sha256"] = hashlib.sha256(snapshot(store, project["id"], project["episode"])).hexdigest()
        record(row)
        return row

    for index, (language, weight) in enumerate(zip(LANGUAGES, WEIGHTS)):
        folder = root / "sources" / f"repo-{index:02}"
        files = probe_files(index, language)
        if language == "go" and index >= 3:
            # A real, immutable local dependency keeps Go compiler analysis
            # independent of a remote module cache. Its bytes are also admitted
            # and checked as the exporting repository's source heads.
            files["go.mod"] += "\nreplace example.com/moneylib => " + json.dumps(str(projects[2]["folder"])) + "\n"
        units = []
        generated = (args.files_per_project - len(files) if args.files_per_project is not None
                     else args.units * weight)
        for i in range(generated):
            path, content, name, caller = unit(language, i)
            files[path] = content
            units.append((path, name, caller))
        for path, content in files.items():
            target = folder / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
        project = {"index": index, "language": language, "folder": folder, "paths": list(files),
                   "units": units, "initial_files": len(files),
                   "initial_source_bytes": sum(len(s.encode()) for s in files.values())}
        projects.append(project)
        sync(project, f"admit-{index:02}", no_graph=True)
        cold = sync(project, f"cold-{index:02}")
        current = sync(project, f"unchanged-{index:02}")
        assert current["graph"]["status"] == "current"
        assert cold["graph_sha256"] == current["graph_sha256"]
        assert cold["unresolved_calls_sha256"] == current["unresolved_calls_sha256"]

    def link(label):
        row, packet = call(label, "graph.link", projects[0], {})
        linked = {p["id"]: p for p in packet["repositories"]}
        assert set(linked) == {p["id"] for p in projects}, packet
        assert packet["persisted"] is True and not packet["hidden_repositories"] and not packet["unprojected_repositories"]
        for project in projects:
            assert linked[project["id"]]["current"] is True
            assert linked[project["id"]]["episode"] == project["episode"]
        row.update(edges=packet["edges"], unresolved=packet["unresolved"], repositories=len(linked))
        record(row)
        return packet

    assert link("link-initial")["unresolved"] == 0, "Initial workspace imports are unresolved"
    for project in projects[3:]:
        library = projects[LANGUAGES[:3].index(project["language"])]
        row, packet = call(f"edges-{project['index']:02}", "graph.cross_edges", project, {})
        assert not packet["unresolved"], packet
        targets = [edge for edge in packet["outgoing"] if edge["target_repository_id"] == library["id"]]
        symbol = "QuantizeCents" if project["language"] == "go" else "quantize_cents"
        assert any(edge["symbol"] == symbol for edge in targets), packet
        for edge in targets:
            assert edge["source_episode"] == project["episode"] and edge["target_episode"] == library["episode"]
        row.update(edges=len(packet["outgoing"]), expected_symbol=symbol)
        record(row)

    def context(label, anchor=3, cross_repo=True, max_projects=10, searched=10):
        request = {"scope": "workspace", "question": f"How is the cobalt{anchor} invoice settled?",
                   "max_tokens": 8000, "memory_retrieval": False, "graph_retrieval": True, "cross_repo": cross_repo}
        if max_projects is not None:
            request["max_projects"] = max_projects
        row, packet = call(label, "context", projects[anchor], request)
        assert packet["model_calls"] == 0 and packet["tokens"] <= request["max_tokens"]
        assert packet["workspace"]["projects_searched"] == searched, packet["workspace"]
        assert packet["workspace"]["projects_failed"] == 0, packet["workspace"]
        assert packet["workspace"]["cross_repo"].get("error") is None, packet["workspace"]
        lookup = {p["id"]: p for p in projects}
        for evidence in packet["evidence"]:
            project = lookup[evidence["repository_id"]]
            path = evidence["id"].removeprefix("file:")
            assert path in project["paths"], (path, "deleted/unexpected citation")
            raw = (project["folder"] / path).read_bytes()
            assert hashlib.sha256(raw).hexdigest() == evidence["sha256"]
            assert raw[evidence["byte_start"]:evidence["byte_end"]].decode() == evidence["text"]
        assert any(e["repository_id"] == projects[anchor]["id"] and f"cobalt{anchor}" in e["text"] for e in packet["evidence"])
        row.update(tokens=packet["tokens"], evidence=len(packet["evidence"]), workspace=packet["workspace"])
        record(row)
        return packet

    def lifted(packet, library):
        return [e for e in packet["evidence"] if e["repository_id"] == projects[library]["id"] and "cross_repo" in e]

    repeated = {}
    for sample in range(args.samples):
        for anchor in (3, 4, 5):
            for cross in ([False, True] if sample % 2 == 0 else [True, False]):
                packet = context(f"context-{anchor}-{int(cross)}-{sample}", anchor, cross)
                assert bool(lifted(packet, anchor - 3)) == cross, (anchor, cross, packet["workspace"])
                key = (anchor, cross)
                if key in repeated:
                    assert packet == repeated[key], (key, "Repeated workspace read changed")
                else:
                    repeated[key] = packet
    default = context("default-fanout", max_projects=None, searched=8)
    assert default["workspace"]["projects_left_out"] == 2

    # The local capability belongs only to this disposable fixture. Never save
    # the newly granted capability in a result, request or command line.
    reader = grant_reader(binary, root, store)
    session_path = store / "local-session.json"
    owner_bytes = session_path.read_bytes()
    reader_session = json.loads(owner_bytes)
    reader_session["capability"] = reader["capability"]

    def as_reader(label, searched):
        try:
            session_path.write_text(json.dumps(reader_session))
            return context(label, anchor=4, searched=searched)
        finally:
            session_path.write_bytes(owner_bytes)

    before = as_reader("reader-before", 10)
    assert lifted(before, 1)
    row, _ = call("revoke-library", "graph.access", projects[1], {"credential_id": reader["id"], "state": "revoked"})
    record(row)
    denied = as_reader("reader-after", 9)
    assert projects[1]["id"] not in json.dumps(denied), "Revoked project leaked"
    assert lifted(context("owner-after-revoke", anchor=4), 1)

    library = projects[0]
    target = library["folder"] / LIBRARY_PATHS[0]
    target.write_text(LIBRARY_SOURCES[0] + "\n# Half up, to whole cents.\n")
    sync(library, "edit-without-graph", no_graph=True)
    stale = context("stale-library")
    assert stale["workspace"]["cross_repo"]["withheld_changed"] >= 1
    assert not lifted(stale, 0)
    sync(library, "edit-rebuild")
    link("link-edited")
    assert lifted(context("edited-library"), 0)
    target.unlink()
    library["paths"].remove(LIBRARY_PATHS[0])
    sync(library, "delete-without-graph", no_graph=True)
    deleted = context("deleted-library-stale-graph")
    assert not lifted(deleted, 0)
    sync(library, "delete-rebuild")
    deleted_links = link("link-deleted")
    python_consumers = [p for p in projects[3:] if p["language"] == "py"]
    assert deleted_links["unresolved"] == len(python_consumers)
    assert not lifted(context("deleted-library"), 0)
    row, edges = call("edges-deleted", "graph.cross_edges", projects[3], {})
    assert not [edge for edge in edges["outgoing"] if edge["target_repository_id"] == library["id"]]
    row.update(outgoing=len(edges["outgoing"]))
    record(row)
    # graph.cross_edges reports unresolved imports toward its project. Read
    # the exporter to inspect the consumers of its now-deleted module.
    row, edges = call("unresolved-deleted-exporter", "graph.cross_edges", library, {})
    unresolved = edges["unresolved"]
    assert {entry["source_repository_id"] for entry in unresolved} == {p["id"] for p in python_consumers}
    assert len(unresolved) == len(python_consumers)
    for entry in unresolved:
        assert entry["module_path"] == "moneylib.rounding" and entry["reason"] == "module_not_exported"
        assert json.loads(entry["candidates"]) == [library["id"]]
        assert entry["source_episode"] == next(p["episode"] for p in python_consumers if p["id"] == entry["source_repository_id"])
    row.update(unresolved=len(unresolved))
    record(row)

    assert sha(binary) == identity
    assert harness == {name: sha(Path(__file__).with_name(name)) for name in harness}, "Harness changed during run"
    save(root / "results.json", {"binary_sha256": identity, "harness_sha256": harness, "rows": rows,
         "samples": args.samples, "generated_files": sum(len(p["units"]) for p in projects),
         "initial_files": sum(p["initial_files"] for p in projects),
         "size_policy": {"files_per_project": args.files_per_project,
                         "weighted_units": args.units if args.files_per_project is None else None},
         "projects": [{k: p[k] for k in ("index", "language", "id", "episode", "initial_files", "initial_source_bytes", "heads")} for p in projects],
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Sequential fresh CLI measurements; warm OS cache; source hashes, generated local calls, cross-project episode bindings, identical repeated packets, exact citation bytes, budgets, stale edges, deletions and revoked access checked",
         "limitations": "Synthetic files and supported import forms only. Every generated local call is checked, but other graph content is not an independent full-extraction oracle. No A/B speedup or production SLO is established."})


if __name__ == "__main__":
    main()
