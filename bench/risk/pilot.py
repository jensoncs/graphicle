#!/usr/bin/env python3
"""Local, model-free risk-evidence pilot for Sentinel. No risk verdict is generated."""
import argparse
from collections import Counter
import copy
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tempfile
import time

from fixtures import suite

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "sdk/python"))
from graphicle_sdk import LocalServer

PROFILE = {"goos": "linux", "goarch": "amd64", "tests": False, "tags": [], "workspace": ""}
FORMAT = "graphicle.risk-evidence-pilot.v1"
PHASE = re.compile(r"^phase=(\w+) ms=(\d+)(?: rss_mb=([\d.]+) peak_rss_mb=([\d.]+))?$")


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(value):
    return hashlib.sha256(value).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")


def phases(stderr):
    rows = []
    for line in stderr.splitlines():
        match = PHASE.fullmatch(line)
        if match:
            name, ms, rss, peak = match.groups()
            rows.append({"name": name, "ms": int(ms),
                         "rss_mb": float(rss) if rss is not None else None,
                         "process_peak_rss_mb": float(peak) if peak is not None else None})
    return rows


def verify_packet(packet, request, sources):
    """Fail on wrong pins, unresolved delta bodies, corrupt or unbound excerpts."""
    for key in ("base", "head", "profile"):
        if packet[key] != request[key]:
            raise ValueError(f"Packet {key} differs from request")
    if packet.get("previous_packet_id") or packet.get("unchanged"):
        raise ValueError("Pilot scores full packets only")
    if type(packet.get("tokens")) is not int or not 0 <= packet["tokens"] <= request["max_tokens"]:
        raise ValueError("Invalid packet token count")
    for entry in packet["entries"]:
        if entry.get("service_boundary"):
            continue  # Boundary assertions are scored separately, not as source excerpts.
        raw = sources[(entry["side"], entry["repository_id"], entry["file"])].encode()
        start, end = entry["byte_start"], entry["byte_end"]
        if (not 0 <= start < end <= len(raw) or digest(raw) != entry["file_sha256"]
                or raw[start:end].decode() != entry.get("text")
                or digest(raw[start:end]) != entry["text_sha256"]):
            raise ValueError("Source excerpt failed pinned byte verification")


def delivered(label, packet, repositories):
    if label["kind"] == "relationship":
        return any(step.get("relation") == label["relation"]
                   for e in packet["entries"] if e["side"] == "head"
                   for reason in e.get("reasons", []) for step in reason.get("steps", []))
    entries = [e for e in packet["entries"] if e["repository_id"] == repositories[label["repository"]]
               and e["side"] == label["side"] and e["file"] == label["file"]]
    if label["kind"] == "source":
        return any(label["quote"] in e.get("text", "") for e in entries)
    if label["kind"] == "history":
        return any(a.get("subject") == label["subject"] for e in entries
                   for a in e.get("business_context", {}).get("historical_assertions", []))
    if label["kind"] == "observation":
        return any(e.get("execution_observations") for e in entries)
    raise ValueError("Unknown evidence label kind")


def diagnose(packet, labels, repositories, history_requested):
    coverage = packet["coverage"]
    results = []
    for i, label in enumerate(labels):
        present = delivered(label, packet, repositories)
        # Aggregate omissions cannot locate one missing item at a precise stage.
        status = "delivered" if present else ("not_requested" if label["kind"] == "history"
                  and not history_requested else "not_delivered_stage_unknown")
        results.append({"label": i, "kind": label["kind"], "status": status})
    return {"labels": results, "required": len(results),
            "delivered": sum(r["status"] == "delivered" for r in results),
            "recall": sum(r["status"] == "delivered" for r in results) / len(results) if results else None,
            "coverage": coverage, "analysis_cache": packet.get("analysis_cache"),
            "business_context": packet.get("business_context"),
            "execution_coverage": packet.get("execution_coverage"),
            "execution_flows": packet.get("execution_flows", {}).get("coverage"),
            "service_graph": packet.get("service_graph"),
            "selected_entries": len(packet["entries"]),
            "selected_by_reason": dict(Counter(r.get("reason", "unknown")
                for e in packet["entries"] for r in e.get("reasons", []))),
            "business_criticality": "unassessed", "production_hotness": "unassessed",
            "risk_verdict": "unmeasured", "may_lower_sentinel_risk": False}


def compare(current, previous):
    if current["contract"] != previous["contract"]:
        raise ValueError("Cannot compare different fixtures, budgets, profile or pilot protocol")
    now = {r["id"]: r for r in current["runs"]}
    findings = []
    for old in previous["runs"]:
        new = now.get(old["id"])
        if new is None or (old["status"] == "ok" and new["status"] != "ok"):
            findings.append({"id": old["id"], "reason": "missing_or_failed_run"})
        elif old["status"] == "ok" and new["status"] == "ok":
            lost = sorted(set(old["delivered_labels"]) - set(new["delivered_labels"]))
            if lost:
                findings.append({"id": old["id"], "reason": "evidence_lost", "labels": lost})
            if new["forbidden_hits"] > old["forbidden_hits"]:
                findings.append({"id": old["id"], "reason": "negative_control_regressed"})
    return findings


def rollup(runs):
    groups = {}
    for r in runs:
        key = f'{r["variant"]}/{r["budget"]}/{r["sample"]}'
        groups.setdefault(key, []).append(r)
    result = {}
    for key, rows in groups.items():
        ok = [r for r in rows if r["status"] == "ok"]
        required = sum(r["required"] for r in rows)  # Failed calls remain misses.
        times = sorted(r["elapsed_ms"] for r in ok)
        result[key] = {"planned": len(rows), "completed": len(ok), "errors": len(rows) - len(ok),
            "required": required, "delivered": sum(len(r["delivered_labels"]) for r in ok),
            "recall": sum(len(r["delivered_labels"]) for r in ok) / required if required else None,
            "forbidden_hits": sum(r["forbidden_hits"] for r in ok),
            "tokens_completed": sum(r["tokens"] for r in ok),
            "latency_ms_completed": {"samples": len(times), "median":
                (times[(len(times) - 1) // 2] + times[len(times) // 2]) / 2 if times else None},
            "phase_ms_sum": dict(sum((Counter(r["phase_ms"]) for r in ok), Counter()))}
    return result


class Pilot:
    def __init__(self, binary, output, budgets, timeout):
        self.binary, self.output, self.budgets, self.timeout = binary, output, budgets, timeout
        self.env = {k: v for k, v in os.environ.items() if k in
                    ("PATH", "TMPDIR", "TEMP", "TMP", "SystemRoot", "ComSpec", "PATHEXT", "SDKROOT")}
        self.env.update(GRAPHICLE_NO_UPDATE_NOTIFIER="1", GOPROXY="off", GOSUMDB="off",
                        GOTOOLCHAIN="local", CGO_ENABLED="0")
        data = suite()
        save(output / "fixtures.json", data)
        self.cases = data["cases"]
        self.summary = {"format": FORMAT, "scope": "authored synthetic local evidence regression; not Sentinel verdict accuracy",
            "contract": {"format": FORMAT, "fixtures_sha256": digest(encoded(data)), "budgets": budgets,
                         "profile": PROFILE, "max_depth": 16},
            "binary_sha256": digest(binary.read_bytes()), "runner_sha256": digest(Path(__file__).read_bytes()),
            "platform": platform.platform(), "python": platform.python_version(),
            "runs": [], "checks": [], "unmeasured": ["sentinel_risk_accuracy", "production_hotness",
                "business_criticality", "real_query_quality", "model_tokens", "production_latency"]}

    def command(self, args, timing=False):
        env = {**self.env, "GRAPHICLE_PHASE_TIMINGS": "1" if timing else "0"}
        started = time.monotonic()
        p = subprocess.run([str(self.binary), *args], env=env, capture_output=True,
                           text=True, timeout=self.timeout)
        return p, (time.monotonic() - started) * 1000

    def case(self, case):
        out = self.output / case["id"]
        out.mkdir()
        with tempfile.TemporaryDirectory(prefix="graphicle-risk-pilot-") as tmp:
            root = Path(tmp)
            config, store = root / "models.json", root / "store"
            save(config, {"models": {}})
            base, head, repositories, sources, histories = [], [], {}, {}, []
            setup_started = time.monotonic()
            with LocalServer(self.binary, store, config, env=self.env, timeout=self.timeout) as client:
                for spec in case["repositories"]:
                    project = client.project(spec["name"])
                    repositories[spec["name"]] = project.id
                    checkout = root / spec["name"]
                    checkout.mkdir()

                    def git(*args):
                        return subprocess.check_output(["git", "-C", str(checkout), "-c", "user.name=Pilot",
                            "-c", "user.email=pilot@example.invalid", "-c", "core.hooksPath=/dev/null",
                            "-c", "commit.gpgsign=false", *args], env={**self.env,
                            "GIT_AUTHOR_DATE": "2001-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2001-01-01T00:00:00Z"},
                            stderr=subprocess.PIPE, timeout=self.timeout).decode().strip()

                    git("init", "-q")
                    for side, pins in (("base", base), ("head", head)):
                        files = spec[side]
                        for old in set(spec["base"]) - set(files):
                            (checkout / old).unlink(missing_ok=True)
                        for name, body in files.items():
                            path = checkout / name
                            path.parent.mkdir(parents=True, exist_ok=True)
                            path.write_text(body, encoding="utf-8")
                            sources[(side, project.id, name)] = body
                        git("add", "--all")
                        subject = spec.get("base_subject", "Initial pilot behavior") if side == "base" else "Proposed pilot change"
                        git("commit", "--allow-empty", "-qm", subject + "\n\nCo-authored-by: Codex <noreply@openai.com>")
                        policy = digest(b"complete authored synthetic fixture inventory v1")
                        commit = git("rev-parse", "HEAD")
                        begin = project.command("sync.prepare", expected_snapshot=project.status()["repository"]["snapshot"],
                            metadata={"commit": commit, "policy_sha256": policy, "scope_complete": True},
                            manifest=[{"path": p, "sha256": digest(c.encode()), "bytes": len(c.encode())} for p, c in files.items()])
                        if begin.get("sync_id"):
                            project.command("sync.upload", sync_id=begin["sync_id"],
                                            files=[{"path": p, "content": c} for p, c in files.items()])
                            project.command("sync.commit", sync_id=begin["sync_id"])
                        pin = {"repository_id": project.id, "directory": spec["name"], "commit": commit,
                               "snapshot": begin["snapshot"], "policy_sha256": policy, "modules": ["."]}
                        pins.append(pin)
                        pin_path = root / f'{spec["name"]}-{side}.json'
                        save(pin_path, pin)
                        history_args = ["review-history", "--path", str(checkout), "--pin", str(pin_path), "--analyze"]
                        for name in files:
                            if name.endswith(".go"):
                                history_args += ["--file", name]
                        p, ms = self.command(history_args, timing=True)
                        if p.returncode:
                            raise RuntimeError(f"History collection exited {p.returncode}")
                        history = json.loads(p.stdout)
                        histories.append(history)
                        save(out / f'{spec["name"]}-{side}-history.json', history)
                        save(out / f'{spec["name"]}-{side}-history-timing.json', {"elapsed_ms": ms, "phases": phases(p.stderr)})
                observation = case.get("observation")
                if observation:
                    target = next(p for p in head if p["directory"] == observation["repository"])
                    receipt = client.command("coverage.upload", **{"repository_id": target["repository_id"],
                        "format": "graphicle.coverage.v1", **{k: target[k] for k in ("commit", "snapshot", "policy_sha256")},
                        "profile": PROFILE, "environment": "synthetic-ci-not-production",
                        "ranges": [{k: observation[k] for k in ("path", "line_start", "line_end")}]})
                    save(out / "coverage-upload.json", receipt)
                self.summary["checks"].append({"case": case["id"], "check": "no_model_calls",
                    "passed": client.usage()["model_usage"]["calls"] == 0})
            save(out / "setup.json", {"elapsed_ms": (time.monotonic() - setup_started) * 1000,
                "repositories": repositories, "base": base, "head": head})
            original = {"repository_id": repositories[case.get("anchor", case["repositories"][0]["name"])],
                        "base": base, "head": head, "profile": PROFILE, "max_depth": 16}
            last = None
            for budget in self.budgets:
                for variant in ("static", "history"):
                    request = {**original, "max_tokens": budget}
                    if variant == "history":
                        request["business_context"] = {"history": histories}
                    last = self.review(case, out, store, config, request, sources, repositories, variant, "first")
            # Explicit same-request warm replay and instrumentation-off control.
            warm = self.review(case, out, store, config, request, sources, repositories, "history", "warm")
            off = self.review(case, out, store, config, request, sources, repositories, "history", "timing-off", False)
            for name, other in (("warm_replay_equivalent", warm), ("instrumentation_preserves_packet", off)):
                self.summary["checks"].append({"case": case["id"], "check": name,
                    "passed": last is not None and other is not None and last == other})
            if case["id"] == "service-chain":
                self.review(case, out, store, config, {**request, "max_depth": 0}, sources, repositories,
                            "history", "depth-zero")

    def review(self, case, out, store, config, request, sources, repositories, variant, sample, timing=True):
        name = f'{variant}-{request["max_tokens"]}-{sample}'
        folder = out / name
        folder.mkdir()
        save(folder / "request.json", request)  # Evaluator labels are never sent.
        row = {"id": case["id"] + "/" + name, "case": case["id"], "variant": variant,
               "budget": request["max_tokens"], "sample": sample, "required": len(case["labels"]),
               "status": "error", "delivered_labels": [], "forbidden_hits": 0}
        self.summary["runs"].append(row)
        started = time.monotonic()
        try:
            p, ms = self.command(["--data-dir", str(store), "--config", str(config),
                                  "review", "--request", str(folder / "request.json")], timing=timing)
            row["elapsed_ms"] = ms
            (folder / "stderr.log").write_text(p.stderr, encoding="utf-8")
            if p.returncode:
                raise RuntimeError(f"Review exited {p.returncode}")
            packet = json.loads(p.stdout)
            save(folder / "packet.json", packet)
            verify_packet(packet, request, sources)
            diagnostics = diagnose(packet, case["labels"], repositories, variant == "history")
            phase_rows = phases(p.stderr)
            diagnostics["timing"] = {"elapsed_ms": ms, "phases": phase_rows,
                "attribution": "one CLI process per request", "includes_cli_startup": True,
                "note": "Nested phases overlap; RSS is process memory, not phase allocation."}
            save(folder / "diagnostics.json", diagnostics)
            row.update(status="ok", tokens=packet["tokens"],
                delivered_labels=[r["label"] for r in diagnostics["labels"] if r["status"] == "delivered"],
                forbidden_hits=sum(delivered(l, packet, repositories) for l in case["forbidden"]),
                phase_ms=dict(sum((Counter({r["name"]: r["ms"]}) for r in phase_rows), Counter())),
                coverage_complete=packet["coverage"]["complete"], analysis_cache=packet.get("analysis_cache"))
            checks = {"negative_controls": row["forbidden_hits"] == 0,
                      "phase_trace_available": not timing or any(r["name"] == "review_pack" for r in phase_rows),
                      "unknown_is_preserved": not case.get("must_be_incomplete") or not packet["coverage"]["complete"]}
            if sample == "depth-zero":
                checks["depth_limit_visible"] = not packet["coverage"]["complete"] and any(
                    i["reason"] == "depth_limit" for i in packet["coverage"]["issues"])
            for check, passed in checks.items():
                self.summary["checks"].append({"run": row["id"], "check": check, "passed": passed})
            normalized = copy.deepcopy(packet)
            normalized.pop("analysis_cache", None)
            normalized.pop("tokens", None)
            return normalized
        except (ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as e:
            row.update(status="error", error=type(e).__name__, elapsed_ms=(time.monotonic() - started) * 1000)
            save(folder / "error.json", row)
            return None
        finally:
            save(folder / "measurement.json", row)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--budgets", nargs="+", type=int, default=[4000, 32000])
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--baseline", type=Path)
    args = parser.parse_args()
    if any(not 512 <= b <= 64000 for b in args.budgets) or args.timeout <= 0:
        parser.error("Budgets must be 512–64000 tokens; timeout must be positive")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    pilot = Pilot(args.binary.resolve(strict=True), args.output, sorted(set(args.budgets)), args.timeout)
    try:
        for case in pilot.cases:
            print(f'Running {case["id"]}', flush=True)
            pilot.case(case)
            save(args.output / "summary.json", pilot.summary)
    except Exception as e:
        pilot.summary["setup_error"] = {"type": type(e).__name__, "message": str(e)[:300]}
    pilot.summary["metrics"] = rollup(pilot.summary["runs"])
    pilot.summary["regressions"] = []
    if args.baseline:
        try:
            pilot.summary["regressions"] = compare(pilot.summary, json.loads(args.baseline.read_text()))
        except ValueError as e:
            pilot.summary["comparison_error"] = str(e)
    passed = (not pilot.summary.get("setup_error") and not pilot.summary.get("comparison_error")
              and len(pilot.summary["runs"]) == len(pilot.cases) * (len(pilot.budgets) * 2 + 2) + 1
              and all(r["status"] == "ok" for r in pilot.summary["runs"])
              and all(c["passed"] for c in pilot.summary["checks"]) and not pilot.summary["regressions"])
    pilot.summary["status"] = "passed" if passed else "failed"
    pilot.summary["gaps"] = [{"run": r["id"], "missing_labels": sorted(set(range(r["required"])) - set(r["delivered_labels"]))}
                             for r in pilot.summary["runs"] if len(r["delivered_labels"]) < r["required"]]
    save(args.output / "summary.json", pilot.summary)
    print(json.dumps({"status": pilot.summary["status"], "runs": len(pilot.summary["runs"]),
                      "runs_with_evidence_gaps": len(pilot.summary["gaps"]), "output": str(args.output)}, indent=2))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
