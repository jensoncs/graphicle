#!/usr/bin/env python3
"""Store under load: index a large repository into a store that is serving reads.

Stdlib only. README.md beside this file says what it measures and how to run
it; docs/configuration.md lists the store settings it exercises.

One run:

  1. A fresh data folder. The seed repository is synced into it (`graphicle
     sync SEED --json`), so readers have a large code graph to query.
  2. `graphicle serve` on loopback, optionally under `strace -f -c` for fsyncs.
  3. N clients (default 33) call the server's HTTP MCP endpoint for the seed
     project in a loop, with a fixed mix of the graph tools (`MIX`), each
     client with its own MCP session, a new one every `--calls-per-pass`
     calls (a "pass").
  4. After `--warmup` seconds, the gate's indexing flow: `register` an empty
     project over the API, then sync the indexed repository into it with
     `graphicle sync PATH --project ID --json` as a separate process, under
     optional `strace -f -c`. Readers keep reading the seed project throughout.
  5. Readers go on for `--after` seconds once indexing ends, then stop.

Every `--sample` seconds (default 10) it records the store and WAL size, the
RSS of the server and of the indexer, and the reads completed. At the end it
writes `<out>/run.json` (the summary), `<out>/samples.csv`, `<out>/calls.csv`
and the processes' logs.
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import random
import re
import math
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

# The weighted mix of MCP tool calls, weighted to the graph queries. Names
# are the server's (`tools/list`); a tool the server does not list is
# dropped from the mix and reported. Each entry: (tool, weight, argument
# builder name).
MIX = [
    ("query_graph", 20, "question"),
    ("get_node", 14, "symbol"),
    ("get_neighbors", 14, "symbol"),
    ("shortest_path", 6, "pair"),
    ("graph_as_of", 6, "query"),
    ("god_nodes", 3, "none"),
    ("graph_stats", 3, "none"),
    ("recall_project", 10, "question"),
    ("read_project_source", 8, "file"),
    ("repository_files", 6, "prefix"),
    ("search_project_memory", 2, "question"),
    ("repository_status", 2, "none"),
]


def now() -> float:
    return time.monotonic()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def rss_kib(pid: int | None) -> tuple[int | None, int | None]:
    """RSS and high-water RSS in KiB; macOS supplies sampled RSS only."""
    if not pid:
        return None, None
    if sys.platform == "darwin":
        try:
            result = subprocess.run(["ps", "-p", str(pid), "-o", "rss="],
                                    capture_output=True, text=True, timeout=2, check=True)
            rss = int(result.stdout.strip())
            return rss, rss
        except (OSError, ValueError, subprocess.SubprocessError):
            return None, None
    try:
        text = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return None, None
    def field(name: str) -> int | None:
        match = re.search(rf"^{name}:\s+(\d+) kB", text, re.M)
        return int(match.group(1)) if match else None
    return field("VmRSS"), field("VmHWM")


def graphicle_pid(root: int | None, traced: bool) -> int | None:
    """Without strace the root is Graphicle; otherwise locate its child."""
    if not root or not traced:
        return root
    try:
        entries = list(Path("/proc").iterdir())
    except OSError:
        return None
    for entry in entries:
        if not entry.name.isdigit():
            continue
        try:
            parent = re.search(r"^PPid:\s+(\d+)", (entry / "status").read_text(), re.M)
            if parent and int(parent.group(1)) == root:
                return int(entry.name)
        except OSError:
            continue
    return None


def stop_process(process: subprocess.Popen | None) -> None:
    """Stop the process group we started, including an indexer's helpers."""
    if process is None:
        return
    for sig, wait in ((signal.SIGINT, 10), (signal.SIGKILL, 5)):
        try:
            os.killpg(process.pid, sig)
        except ProcessLookupError:
            break
        try:
            process.wait(timeout=wait)
            # The leader may exit before its helpers; terminate any left.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            break
        except subprocess.TimeoutExpired:
            continue
    process.wait(timeout=5)


def file_size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def pct(values: list[float], q: float) -> float | None:
    if not values:
        return None
    values = sorted(values)
    k = (len(values) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (k - lo)


class Server:
    def __init__(self, base: str, token: str, workspace: str, timeout: float = 30):
        self.base, self.token, self.workspace = base, token, workspace
        self.timeout = timeout

    def post(self, path: str, body: dict, session: str | None = None, timeout: float | None = None) -> tuple[int, dict, dict]:
        data = json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method="POST")
        request.add_header("Content-Type", "application/json")
        request.add_header("Accept", "application/json, text/event-stream")
        request.add_header("Authorization", f"Bearer {self.token}")
        if session:
            request.add_header("Mcp-Session-Id", session)
        try:
            with urllib.request.urlopen(request, timeout=timeout or self.timeout) as response:
                raw = response.read()
                headers = dict(response.headers)
                status = response.status
        except urllib.error.HTTPError as error:
            raw, headers, status = error.read(), dict(error.headers), error.code
        try:
            parsed = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            parsed = {"raw": raw[:500].decode(errors="replace")}
        return status, parsed, headers

    def mcp(self, repository: str, method: str, params: dict, session: str | None, ident: int):
        return self.post(f"/mcp/{self.workspace}/{repository}",
                         {"jsonrpc": "2.0", "id": ident, "method": method, "params": params}, session)

    def command(self, operation: str, arguments: dict) -> dict:
        status, body, _ = self.post(f"/api/workspaces/{self.workspace}/repositories/command",
                                    {"operation": operation, "arguments": arguments})
        if status >= 300:
            raise RuntimeError(f"{operation}: HTTP {status}: {body}")
        return body


class Corpus:
    """Arguments for the mix, drawn from the seed checkout."""

    WORDS = re.compile(r"^(?:func (?:\([^)]*\) )?|(?:pub )?(?:async )?(?:def|fn|function) )([A-Za-z_][A-Za-z0-9_]{1,80})\(", re.M)

    def __init__(self, root: Path, rng: random.Random, limit: int = 4000):
        files = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, text=True, check=True).stdout.split("\0")
        files = [f for f in files if f]
        code = [f for f in files if f.endswith((".go", ".rs", ".py", ".ts", ".c", ".h", ".java"))]
        rng.shuffle(code)
        self.files = code[:limit]
        if not self.files:
            raise ValueError('seed checkout has no tracked supported code files')
        symbols: set[str] = set()
        for path in self.files[:1500]:
            try:
                text = (root / path).read_text(errors="replace")
            except OSError:
                continue
            for match in self.WORDS.finditer(text):
                symbols.add(match.group(1))
        self.symbols = sorted(symbols) or ["main"]

    def args(self, kind: str, schema: dict, rng: random.Random) -> dict:
        r, props = rng, set((schema.get("properties") or {}).keys())
        def put(out: dict, names: tuple[str, ...], value):
            for name in names:
                if name in props:
                    out[name] = value
                    return
        out: dict = {}
        symbol = r.choice(self.symbols)
        if kind == "query":
            put(out, ("query", "q", "text", "pattern", "question"), symbol)
        elif kind == "symbol":
            put(out, ("symbol", "node", "name", "id", "query", "label"), symbol)
        elif kind == "pair":
            put(out, ("source", "from", "start"), symbol)
            put(out, ("target", "to", "end"), r.choice(self.symbols))
        elif kind == "file":
            put(out, ("path", "file", "file_path"), r.choice(self.files))
        elif kind == "prefix":
            path = r.choice(self.files)
            put(out, ("prefix",), path.rsplit("/", 1)[0] if "/" in path else "")
            put(out, ("limit",), 50)
        elif kind == "question":
            put(out, ("question", "query", "task", "text"), f"How does {symbol} work and what calls it?")
        for name in (schema.get("required") or []):
            if name not in out:
                out[name] = symbol
        return out


def build_mix(tools: list[dict]) -> tuple[list[tuple[str, int, str, dict]], list[str]]:
    listed = {t["name"]: t for t in tools}
    mix, dropped = [], []
    for name, weight, kind in MIX:
        if name in listed:
            mix.append((name, weight, kind, listed[name].get("inputSchema") or {}))
        else:
            dropped.append(name)
    return mix, dropped


class Readers:
    def __init__(self, server: Server, repository: str, corpus: Corpus, mix, clients: int,
                 calls_per_pass: int, seed: int):
        self.server, self.repository, self.corpus, self.mix = server, repository, corpus, mix
        self.clients, self.calls_per_pass, self.seed = clients, calls_per_pass, seed
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.calls: list[tuple[float, float, str, int, bool, str]] = []  # t, seconds, tool, status, ok, phase
        self.phase = "warmup"
        self.passes = 0
        self.errors: dict[str, str] = {}  # the first error text of each tool and status
        self.threads: list[threading.Thread] = []

    def start(self):
        for n in range(self.clients):
            t = threading.Thread(target=self.client, args=(n,), daemon=True)
            t.start()
            self.threads.append(t)

    def open_session(self, ident: int) -> str | None:
        status, body, headers = self.server.mcp(self.repository, "initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "store-load", "version": "1"}}, None, ident)
        if status != 200 or "error" in body:
            raise RuntimeError(f"initialize: HTTP {status}: {body}")
        session = headers.get("Mcp-Session-Id") or headers.get("mcp-session-id")
        if session:
            self.server.mcp(self.repository, "notifications/initialized", {}, session, ident + 1)
        with self.lock:
            self.passes += 1
        return session

    def client(self, n: int):
        rng = random.Random(self.seed * 1000 + n)
        weights = [w for _, w, _, _ in self.mix]
        ident, made, session = 0, 0, None
        while not self.stop.is_set():
            if made % self.calls_per_pass == 0:
                ident += 2
                try:
                    session = self.open_session(ident)
                except Exception:  # a refused or dropped connection is a failed call
                    session = None
            if self.stop.is_set():
                break
            name, _, kind, schema = rng.choices(self.mix, weights=weights)[0]
            arguments = self.corpus.args(kind, schema, rng)
            with self.lock:
                phase = self.phase
            ident += 1
            started = now()
            try:
                status, body, _ = self.server.mcp(self.repository, "tools/call",
                                                  {"name": name, "arguments": arguments}, session, ident)
                ok = status == 200 and "error" not in body and not (body.get("result") or {}).get("isError")
                if not ok:
                    key = f"{name}:{status}"
                    text = json.dumps(body.get("error") or body.get("result") or body)[:400]
                    with self.lock:
                        self.errors.setdefault(key, text)
            except Exception as error:
                status, ok = 0, False
                with self.lock:
                    self.errors.setdefault(f"{name}:0", str(error)[:400])
            elapsed = now() - started
            made += 1
            with self.lock:
                self.calls.append((started, elapsed, name, status, ok, phase))

    def finish(self):
        self.stop.set()
        deadline = now() + 2 * self.server.timeout + 3
        for t in self.threads:
            t.join(timeout=max(0, deadline - now()))
        if any(t.is_alive() for t in self.threads):
            raise RuntimeError('a reader did not stop within its request timeout')


def strace_wrap(cmd: list[str], out: Path, enabled: bool) -> list[str]:
    if not enabled:
        return cmd
    return ["strace", "-f", "--seccomp-bpf", "-qq", "-c", "-e",
            "trace=fsync,fdatasync,sync_file_range", "-o", str(out)] + cmd


def fsync_count(path: Path) -> dict:
    """strace -c's table: {syscall: calls}."""
    counts: dict[str, int] = {}
    try:
        text = path.read_text()
    except OSError:
        return counts
    for line in text.splitlines():
        parts = line.split()
        if len(parts) >= 5 and parts[-1] in ("fsync", "fdatasync", "sync_file_range"):
            # % time, seconds, usecs/call, calls, [errors], syscall
            counts[parts[-1]] = int(parts[3])
    return counts


def latency_ms(values: list[float], quantile: float) -> float | None:
    value = pct(values, quantile)
    return round(value * 1000, 1) if value is not None else None


def summarize_calls(calls: list[tuple]) -> tuple[dict, dict, dict]:
    phases, tools, failures = {}, {}, {}
    for phase in ("warmup", "indexing", "after"):
        rows = [c for c in calls if c[5] == phase]
        latency = [c[1] for c in rows if c[4]]
        span = max((c[0] + c[1] for c in rows), default=0) - min((c[0] for c in rows), default=0)
        phases[phase] = {
            "calls": len(rows), "failed": sum(not c[4] for c in rows),
            "p50_ms": latency_ms(latency, .5), "p95_ms": latency_ms(latency, .95),
            "p99_ms": latency_ms(latency, .99),
            "reads_per_s": round(len(rows) / span, 1) if span else 0,
        }
    for name in sorted({c[2] for c in calls}):
        rows = [c for c in calls if c[2] == name]
        latency = [c[1] for c in rows if c[4]]
        tools[name] = {"calls": len(rows), "failed": sum(not c[4] for c in rows),
                       "p50_ms": latency_ms(latency, .5), "p99_ms": latency_ms(latency, .99)}
    for call in calls:
        if not call[4]:
            key = f"{call[2]}:{call[3]}"
            failures[key] = failures.get(key, 0) + 1
    return phases, tools, failures


def fresh_paths(data: Path, output: Path, seed: Path, index: Path) -> tuple[Path, Path]:
    """Validate all paths before creating anything; never delete old runs."""
    for path in (data, output):
        if path.exists() or path.is_symlink():
            raise ValueError(f"benchmark folder must be new: {path}")
    data, output, seed, index = (p.resolve() for p in (data, output, seed, index))
    for left, right in ((data, output), (data, seed), (data, index), (output, seed), (output, index)):
        if left == right or left in right.parents or right in left.parents:
            raise ValueError(f"benchmark folders and checkouts must not overlap: {left}, {right}")
    for path in (seed, index):
        if not path.is_dir():
            raise ValueError(f"checkout does not exist: {path}")
    return data, output


def positive(value: str) -> float:
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be a finite positive number")
    return number


def positive_int(value: str) -> int:
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin", required=True, type=Path, help="Graphicle binary to measure")
    ap.add_argument("--data-dir", required=True, type=Path, help="new disposable data folder")
    ap.add_argument("--seed", required=True, type=Path, help="tracked checkout queried by readers")
    ap.add_argument("--index", required=True, type=Path, help="checkout indexed under load")
    ap.add_argument("--out", required=True, type=Path, help="new results folder")
    ap.add_argument("--label", default="")
    ap.add_argument("--clients", type=positive_int, default=33)
    ap.add_argument("--calls-per-pass", type=positive_int, default=20)
    ap.add_argument("--warmup", type=positive, default=60)
    ap.add_argument("--after", type=positive, default=120)
    ap.add_argument("--sample", type=positive, default=10)
    ap.add_argument("--index-limit", type=positive, default=8 * 3600, help="timeout for each sync, in seconds")
    ap.add_argument("--request-timeout", type=positive, default=30)
    ap.add_argument("--no-strace", action="store_true", help="disable optional Linux fsync counters")
    ap.add_argument("--rng", type=int, default=1)
    a = ap.parse_args(argv)
    if sys.platform not in ("linux", "darwin"):
        ap.error("the load runner supports Linux and macOS")
    try:
        data, a.out = fresh_paths(a.data_dir, a.out, a.seed, a.index)
        a.bin, a.seed, a.index = a.bin.resolve(), a.seed.resolve(), a.index.resolve()
        if not a.bin.is_file() or not os.access(a.bin, os.X_OK):
            raise ValueError(f"binary is not executable: {a.bin}")
        corpus = Corpus(a.seed, random.Random(a.rng))
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        ap.error(str(error))

    env = dict(os.environ)
    env["GRAPHICLE_NO_UPDATE_NOTIFIER"] = "1"
    env["GRAPHICLE_NO_BROWSER"] = "1"
    # This backport measures SQLite only, even with a future binary.
    env.pop("GRAPHICLE_DATABASE_URL", None)
    env.pop("PGPASSFILE", None)
    traced = sys.platform == "linux" and not a.no_strace and shutil.which("strace") is not None
    data.mkdir(parents=True)
    a.out.mkdir(parents=True)
    database, wal = data / "reviews.sqlite3", data / "reviews.sqlite3-wal"
    shm = data / "reviews.sqlite3-shm"
    record = {
        "label": a.label, "backend": "sqlite", "clients": a.clients, "rng": a.rng,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "binary": str(a.bin), "seed_path": str(a.seed), "index_path": str(a.index),
        "settings": {key: getattr(a, key) for key in
                     ("calls_per_pass", "warmup", "after", "sample", "index_limit", "request_timeout")},
        "strace": traced,
        "memory_measurement": "sampled RSS" if sys.platform == "darwin" else "sampled VmRSS/VmHWM",
    }
    server = sync = readers = sampler_thread = None
    sampling = threading.Event()
    samples, sampling_errors = [], []
    indexer = {"pid": None, "peak": None}
    server_peak = None
    t0 = now()
    failed = False

    def sample_once():
        nonlocal server_peak
        server_rss, server_hwm = rss_kib(graphicle_pid(server.pid, traced))
        index_rss, index_hwm = rss_kib(graphicle_pid(indexer["pid"], traced))
        if server_hwm is not None:
            server_peak = max(server_peak or 0, server_hwm)
        if index_hwm is not None:
            indexer["peak"] = max(indexer["peak"] or 0, index_hwm)
        with readers.lock:
            done, errors = len(readers.calls), sum(not c[4] for c in readers.calls)
            phase = readers.phase
        samples.append({
            "t": round(now() - t0, 3), "phase": phase,
            "store_bytes": file_size(database), "wal_bytes": file_size(wal), "shm_bytes": file_size(shm),
            "server_rss_kib": server_rss, "indexer_rss_kib": index_rss,
            "reads_done": done, "reads_failed": errors,
        })

    def sampler():
        try:
            while not sampling.wait(a.sample):
                sample_once()
        except Exception as error:
            sampling_errors.append(str(error))

    try:
        version = subprocess.run([str(a.bin), "--version"], capture_output=True, text=True,
                                 env=env, timeout=10, check=True)
        record["version"] = version.stdout.strip()
        started = now()
        with (a.out / "seed-sync.log").open("wb") as log, (a.out / "seed.json").open("wb") as out:
            sync = subprocess.Popen([str(a.bin), "--data-dir", str(data), "sync", str(a.seed), "--json"],
                                    stdout=out, stderr=log, env=env, start_new_session=True)
            code = sync.wait(timeout=a.index_limit)
        if code:
            raise RuntimeError(f"seed sync exited {code}; see seed-sync.log")
        seed = json.loads((a.out / "seed.json").read_text())
        record["seed"] = {"repository_id": seed["repository_id"], "seconds": round(now() - started, 3),
                          "graph": seed.get("graph")}
        session = json.loads((data / "local-session.json").read_text())
        workspace, token = session["workspace"], session["capability"]
        port = free_port()
        command = strace_wrap([str(a.bin), "--data-dir", str(data), "serve", "--port", str(port)],
                              a.out / "server.strace", traced)
        with (a.out / "server.log").open("wb") as log:
            server = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT,
                                      env=env, start_new_session=True)
        client = Server(f"http://127.0.0.1:{port}", token, workspace, a.request_timeout)
        deadline = now() + 120
        while now() < deadline:
            if server.poll() is not None:
                raise RuntimeError(f"server exited {server.returncode}; see server.log")
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as response:
                    if response.status == 200:
                        break
            except (OSError, urllib.error.URLError):
                time.sleep(.1)
        else:
            raise RuntimeError("server did not become healthy within 120 seconds")

        repository = seed["repository_id"]
        status, response, headers = client.mcp(repository, "initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "store-load", "version": "1"}}, None, 1)
        if status != 200 or "error" in response:
            raise RuntimeError(f"initialize failed: HTTP {status}")
        sid = headers.get("Mcp-Session-Id") or headers.get("mcp-session-id")
        status, response, _ = client.mcp(repository, "tools/list", {}, sid, 2)
        if status != 200 or "error" in response:
            raise RuntimeError(f"tools/list failed: HTTP {status}")
        tools = (response.get("result") or {}).get("tools") or []
        (a.out / "tools.json").write_text(json.dumps(tools, indent=2))
        mix, dropped = build_mix(tools)
        record["mix"], record["mix_dropped"] = [[name, weight] for name, weight, _, _ in mix], dropped
        if not mix:
            raise RuntimeError("server lists none of the workload's tools")
        record["server_write_lock"] = {"before": client.command(
            "runtime.status", {"repository_id": repository}).get("write_lock")}
        readers = Readers(client, repository, corpus, mix, a.clients, a.calls_per_pass, a.rng)
        t0 = now()
        sample_once()
        sampler_thread = threading.Thread(target=sampler, daemon=True)
        sampler_thread.start()
        readers.start()
        time.sleep(a.warmup)

        name = a.index.name
        registered = client.command("register", {"project_key": f"bench-{name}-{time.time_ns()}", "name": name})
        project = registered["id"]
        with readers.lock:
            readers.phase = "indexing"
        started = now()
        command = strace_wrap([str(a.bin), "--data-dir", str(data), "sync", str(a.index),
                              "--project", project, "--json"], a.out / "indexer.strace", traced)
        killed = False
        with (a.out / "indexer.log").open("wb") as log, (a.out / "indexer.json").open("wb") as out:
            sync = subprocess.Popen(command, stdout=out, stderr=log, env=env, start_new_session=True)
            indexer["pid"] = sync.pid
            try:
                code = sync.wait(timeout=a.index_limit)
            except subprocess.TimeoutExpired:
                stop_process(sync)
                code, killed = sync.returncode, True
        record["index"] = {"project": project, "exit": code, "killed_at_limit": killed,
                           "seconds": round(now() - started, 3),
                           "result": _tail_json(a.out / "indexer.json")}
        failed = code != 0
        indexer["pid"] = None
        with readers.lock:
            readers.phase = "after"
        time.sleep(a.after)
    except (Exception, KeyboardInterrupt) as error:
        failed = True
        record["error"] = f"{type(error).__name__}: {error}"
        print(record["error"], file=sys.stderr)
    finally:
        if readers:
            try:
                readers.finish()
                record["server_write_lock"]["after"] = readers.server.command(
                    "runtime.status", {"repository_id": readers.repository}).get("write_lock")
            except Exception as error:
                failed = True
                record["reader_shutdown_error"] = str(error)
        sampling.set()
        if sampler_thread:
            sampler_thread.join()
        if readers and server:
            try:
                sample_once()
            except Exception as error:
                sampling_errors.append(str(error))
        for process in (sync, server):
            try:
                stop_process(process)
            except Exception as error:
                failed = True
                record.setdefault("cleanup_errors", []).append(str(error))
        record["after_shutdown"] = {"store_bytes": file_size(database), "wal_bytes": file_size(wal)}

    calls = readers.calls if readers else []
    phases, tool_summary, failures = summarize_calls(calls)
    memory_mib = lambda kib: round(kib / 1024, 1) if kib is not None else None
    record.update({
        "completed": "index" in record and not failed and not sampling_errors,
        "reads": phases, "tools": tool_summary, "failures": failures,
        "errors": readers.errors if readers else {}, "passes": readers.passes if readers else 0,
        "sampling_errors": sampling_errors,
        "peak_rss_mib": {"server": memory_mib(server_peak), "indexer": memory_mib(indexer["peak"])},
        "wal_peak_mib": round(max((s["wal_bytes"] for s in samples), default=0) / 2**20, 1),
        "wal_end_mib": round(samples[-1]["wal_bytes"] / 2**20, 1) if samples else None,
        "store_end_mib": round(samples[-1]["store_bytes"] / 2**20, 1) if samples else None,
        "fsyncs": {name: fsync_count(a.out / f"{name}.strace") if traced else None
                   for name in ("server", "indexer")},
    })
    if samples:
        with (a.out / "samples.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, fieldnames=list(samples[0]))
            writer.writeheader()
            writer.writerows(samples)
    with (a.out / "calls.csv").open("w", newline="") as out:
        writer = csv.writer(out)
        writer.writerow(["t", "seconds", "tool", "status", "ok", "phase"])
        writer.writerows([round(c[0] - t0, 3), round(c[1], 4), c[2], c[3], int(c[4]), c[5]] for c in calls)
    (a.out / "run.json").write_text(json.dumps(record, indent=2, allow_nan=False))
    print(json.dumps({key: record[key] for key in
                      ("label", "completed", "reads", "peak_rss_mib", "wal_peak_mib", "failures")}, indent=2))
    return 1 if failed or sampling_errors else (2 if failures else 0)


def _tail_json(path: Path):
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    if isinstance(value, dict):
        graph = value.get("graph") or {}
        return {"status": graph.get("status") or value.get("state"), "nodes": graph.get("nodes"),
                "edges": graph.get("edges"), "files": value.get("files")}
    return None


if __name__ == "__main__":
    sys.exit(main())
