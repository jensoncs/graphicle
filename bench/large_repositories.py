#!/usr/bin/env python3
"""Local, model-free scale probes through the actual Graphicle CLI.

Use a fresh output directory. `prepare` generates synthetic sources and syncs
them; `measure` makes read-only requests against that same retained store.
Separate stores made from the same seed preserve repository IDs for comparisons.
Results contain public generated source only. No user checkout is indexed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import signal
import statistics
import subprocess
import sys
import time
from pathlib import Path


def save(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def environment(root):
    # Explicit allowlist: never forward model credentials or runtime switches.
    env = {k: os.environ[k] for k in ("PATH", "TMPDIR", "SystemRoot") if k in os.environ}
    env.update(GRAPHICLE_NO_UPDATE_NOTIFIER="1", GRAPHICLE_PHASE_TIMINGS="1",
               XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "local"),
               GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GRAPHICLE_MAX_WORKERS="4")
    return env


def run(binary, root, store, label, args, timeout=1800):
    # A fresh worker keeps the measured child's pre-exec RSS independent of
    # graph snapshots retained by the parent harness. On macOS wait4 includes
    # that inherited RSS in the child's high-water mark.
    request = root / (label + ".measurement.json")
    measured = root / (label + ".resources.json")
    save(request, {"binary": str(binary), "root": str(root), "store": str(store),
                   "label": label, "args": args, "timeout": timeout})
    with (root / (label + ".measurement.log")).open("wb") as stderr:
        worker = subprocess.run([sys.executable, str(Path(__file__).resolve()),
                                 "_measure", str(request), str(measured)],
                                cwd=root, env=environment(root), stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL, stderr=stderr)
    if worker.returncode:
        raise RuntimeError(f"{label}: measurement worker exited {worker.returncode}; see {stderr.name}")
    return json.loads(measured.read_text()), json.loads((root / (label + ".json")).read_text())


def _measure(binary, root, store, label, args, timeout=1800):
    out, err = root / (label + ".json"), root / (label + ".log")
    command = [str(binary), "--data-dir", str(store), "--config", str(root / "models.json"), *args]
    started = time.monotonic()
    with out.open("wb") as stdout, err.open("wb") as stderr:
        child = subprocess.Popen(command, cwd=root, env=environment(root),
                                 stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                 start_new_session=True)
        while True:
            pid, status, usage = os.wait4(child.pid, os.WNOHANG)
            if pid:
                child.returncode = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic() - started > timeout:
                os.killpg(child.pid, signal.SIGKILL)
                _, status, _ = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
                raise TimeoutError(f"{label}: exceeded {timeout}s; see {err}")
            time.sleep(0.02)
    row = {"label": label, "seconds": time.monotonic() - started,
           "peak_rss_bytes": usage.ru_maxrss * (1 if platform.system() == "Darwin" else 1024),
           "exit_code": child.returncode, "stdout_sha256": sha(out),
           "resource_accounting": "wait4 of the CLI process"}
    if child.returncode:
        save(root / (label + ".failure.json"), row)
        raise RuntimeError(f"{label}: exit {child.returncode}; see {err}")
    return row, json.loads(out.read_text())


def source(project, index, paragraphs):
    # Distinct symbols, shared identifiers, different package ownership, and
    # multiple passages per file. The needle is inside the selected subtree.
    text = f"# Service {project} component {index}\n\n"
    if index == 7:
        text += "The aquamarine circuit breaker retries exactly seven times.\n\n"
    for part in range(paragraphs):
        text += (f"Component {index} section {part} records the request queue and its shard. "
                 "Workers track leases, retry deadlines, capacity and settlement. "
                 "The replica observes each batch and records its sequence before acknowledging it.\n\n")
    return text


def prepare(args):
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    rows, projects, manifests = [], [], []
    for project in range(args.projects):
        folder = root / "sources" / f"repo-{project:02}"
        manifest = {}
        for component in range(20):
            (folder / f"component-{component:02}").mkdir(parents=True, exist_ok=True)
        for index in range(args.files):
            name = f"component-{index % 20:02}/unit-{index:06}.md"
            path = folder / name
            content = source(project, index, args.paragraphs).encode()
            path.write_bytes(content)
            manifest[name] = hashlib.sha256(content).hexdigest()
        manifests.append(manifest)
        row, reply = run(args.binary, root, root / "store", f"sync-{project:02}",
                         ["sync", str(folder), *([] if args.graph else ["--no-graph"])])
        rows.append(row)
        projects.append(reply["repository_id"])
        print(json.dumps(row), flush=True)
    save(root / "fixture.json", {"synthetic": True, "projects": projects,
         "files_per_project": args.files, "paragraphs": args.paragraphs, "graph": args.graph,
         "manifest_sha256": hashlib.sha256(json.dumps(manifests, sort_keys=True).encode()).hexdigest(),
         "binary_sha256": sha(args.binary), "platform": platform.platform(), "setup": rows})


def requests(projects):
    base = {"repository_id": projects[0],
            "question": "How many times does the aquamarine circuit breaker retry?",
            "max_tokens": 3000, "memory_retrieval": False, "graph_retrieval": False}
    yield "ordinary", base
    yield "filtered", {**base, "include": ["file:component-07/"]}
    yield "filtered_empty", {**base, "include": ["file:missing/"]}
    yield "workspace", {**base, "scope": "workspace", "max_projects": len(projects)}
    yield "workspace_filtered", {**base, "scope": "workspace", "max_projects": len(projects),
                                  "include": ["file:component-07/"]}


def comparable(packet):
    # Only the implementation-route diagnostics differ. All source content,
    # identities, ranks, omissions, budgets and snapshot bindings remain exact.
    packet = dict(packet)
    packet.pop("passage_index", None)
    return packet


def verify(packet, request, fixture, root):
    assert packet["tokens"] <= request["max_tokens"], "Token budget exceeded"
    assert packet["model_calls"] == 0, "Unexpected model invocation"
    workspace = request.get("scope") == "workspace"
    if not workspace:
        assert packet["repository_id"] == request["repository_id"], "Wrong project queried"
    else:
        assert packet["workspace"]["projects_searched"] == len(fixture["projects"])
    for evidence in packet["evidence"]:
        repository = evidence.get("repository_id", packet["repository_id"])
        number = fixture["projects"].index(repository)
        relative = evidence["id"].removeprefix("file:")
        path = root / "sources" / f"repo-{number:02}" / relative
        raw = path.read_bytes()
        assert hashlib.sha256(raw).hexdigest() == evidence["sha256"], "Wrong source hash"
        span = raw[evidence["byte_start"]:evidence["byte_end"]].decode()
        assert span == evidence["text"], "Citation bytes differ"
        if "include" in request:
            assert any(evidence["id"].startswith(p) for p in request["include"]), "Filter escaped"
    if request.get("include") == ["file:missing/"]:
        assert not packet["evidence"], "Empty filter returned source"
    else:
        assert any("retries exactly seven times" in e["text"] for e in packet["evidence"]), "Labelled evidence missing"


def measure(args):
    root = args.output.resolve()
    fixture = json.loads((root / "fixture.json").read_text())
    directory = root / args.label
    directory.mkdir(exist_ok=False)
    save(directory / "models.json", {})
    binaries = {"baseline": args.binary}
    if args.command == "compare":
        binaries["candidate"] = args.candidate.resolve(strict=True)
    identities = {name: sha(binary) for name, binary in binaries.items()}
    rows = []
    for name, request in requests(fixture["projects"]):
        request_path = directory / (name + ".request.json")
        save(request_path, request)
        expected = None
        for sample in range(args.samples + 1):
            order = list(binaries)
            if sample % 2:
                order.reverse()
            for arm in order:
                row, packet = run(binaries[arm], directory, root / "store", f"{name}-{arm}-{sample}",
                                  ["call", "context", "--project", request["repository_id"],
                                   "--arguments", str(request_path)])
                verify(packet, request, fixture, root)
                normalized = comparable(packet)
                if expected is not None and normalized != expected:
                    save(directory / "mismatch.json", {"workload":name, "arm":arm,
                         "expected":expected, "actual":normalized})
                    raise AssertionError(f"{name}: packets differ; see mismatch.json")
                expected = normalized
                row.update(workload=name, arm=arm, warmup=sample == 0)
                rows.append(row)
                save(directory / "observations.json", rows)
                print(json.dumps(row), flush=True)
    summary = {}
    for name in sorted({r["workload"] for r in rows}):
        summary[name] = {}
        for arm in binaries:
            measured = [r for r in rows if r["workload"] == name and r["arm"] == arm and not r["warmup"]]
            summary[name][arm] = {"median_seconds": statistics.median(r["seconds"] for r in measured),
                                 "max_peak_rss_bytes": max(r["peak_rss_bytes"] for r in measured)}
    assert identities == {name: sha(binary) for name, binary in binaries.items()}, "Binary changed during run"
    save(directory / "results.json", {"binary_sha256": identities, "rows": rows,
         "summary": summary, "platform": platform.platform(), "samples": args.samples,
         "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "memory_max_threads": 16,
         "fixture": fixture, "harness_sha256": sha(Path(__file__)),
         "method": "Alternating arm order; fresh CLI processes; one excluded warmup per arm; warm OS cache; read-only context; peak RSS per process"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("prepare")
    build.add_argument("--files", type=int, default=2000)
    build.add_argument("--projects", type=int, default=1)
    build.add_argument("--paragraphs", type=int, default=20)
    build.add_argument("--graph", action="store_true")
    for mode in ("measure", "compare"):
        query = commands.add_parser(mode)
        query.add_argument("--label", required=True)
        query.add_argument("--samples", type=int, default=5)
        if mode == "compare":
            query.add_argument("--candidate", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    if args.command == "prepare":
        if args.files < 8 or not 1 <= args.projects <= 32 or args.paragraphs < 1:
            parser.error("Use at least eight files, 1–32 projects and positive paragraphs")
        prepare(args)
    else:
        if args.samples < 3 or Path(args.label).name != args.label or args.label in (".", ".."):
            parser.error("Use at least three samples and a single directory name")
        measure(args)


if __name__ == "__main__":
    if sys.argv[1:2] == ["_measure"]:
        request = json.loads(Path(sys.argv[2]).read_text())
        for key in ("binary", "root", "store"):
            request[key] = Path(request[key])
        row, _ = _measure(**request)
        row["measurement_worker"] = "fresh Python process"
        save(Path(sys.argv[3]), row)
    else:
        main()
