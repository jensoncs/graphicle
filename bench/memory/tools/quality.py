#!/usr/bin/env python3
"""Run the local quality suite, report measurements, and gate paired regressions.

Uses the existing memory and exact-source evaluators. No model, network service,
or telemetry is required. Latency is descriptive, never a shared-runner CI gate.
"""
import argparse
import copy
import hashlib
import json
import math
import platform
import re
import statistics
import tempfile
from pathlib import Path

from real_query_eval import load_sources, run_project, safe_path, validate_questions
from retrieval_recall import digest, save
from symbol_link_precision import REQUEST, VARIANTS, evaluate_project, validate
from quality_diagnostics import summarize_memory_diagnostics

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SUITE = ROOT / "fixtures/quality_suite.json"
FORMAT = "graphicle.quality.v1"
UNMEASURED = ["independent_real_query_quality", "generated_answer_correctness",
              "agent_task_success", "total_agent_tokens", "production_latency"]


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def load_suite(path, source_roots=None):
    suite = read(path)
    if suite.get("format") != "graphicle.quality-suite.v1" or not suite.get("datasets"):
        raise ValueError("Expected a nonempty graphicle.quality-suite.v1 manifest")
    entries, seen, relocated = [], set(), set()
    source_roots = source_roots or {}
    for entry in suite["datasets"]:
        name = entry["id"]
        if len(safe_path(name).parts) != 1 or name in seen:
            raise ValueError("Suite IDs must be unique safe names")
        seen.add(name)
        source = path.parent / safe_path(entry["file"])
        if not source.resolve().is_relative_to(path.parent.resolve()):
            raise ValueError("Dataset escaped the suite directory")
        dataset = read(source)
        for project in dataset.get("projects", []):
            project_coverage(project)
            key = name + "/" + project["id"]
            if key in source_roots:
                if dataset.get("format") != "graphicle.real-query-seed.v1":
                    raise ValueError("Source overrides require a pinned real-source dataset")
                project["source_root"] = source_roots[key]
                relocated.add(key)
        if entry["kind"] == "memory":
            validate(dataset)
        elif entry["kind"] == "source":
            validate_source(dataset)
        else:
            raise ValueError("Unknown evaluator kind")
        entries.append({**entry, "data": dataset, "sha256": digest(source)})
    if relocated != set(source_roots):
        raise ValueError("Unknown source-root dataset/project")
    return entries


def source_project(project, folder):
    """Materialize inline fixtures; gold quotes become exact evaluator-only spans."""
    files = {}
    for name, body in project["files"].items():
        path = folder / safe_path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        raw = body.encode("utf-8")
        path.write_bytes(raw)
        files[name] = raw
    queries = copy.deepcopy(project["queries"])
    for q in queries:
        q["origin"] = {"kind": "authored_fixture"}
        for g in q["required"]:
            raw, quote = files[g["file"]], g["quote"].encode("utf-8")
            if not quote or raw.count(quote) != 1:
                raise ValueError("Source labels need unique nonempty exact quotes")
            a = raw.index(quote)
            g.update(byte_start=a, byte_end=a + len(quote), line=raw[:a].count(b"\n") + 1)
    result = {**project, "source_root": str(folder), "queries": queries,
              "files": {name: hashlib.sha256(raw).hexdigest() for name, raw in files.items()}}
    validate_questions(result, files)
    return result


def validate_source(dataset):
    pinned = dataset.get("format") == "graphicle.real-query-seed.v1"
    if (dataset.get("format") not in ("graphicle.quality-sources.v1", "graphicle.real-query-seed.v1")
            or not dataset.get("projects") or not dataset.get("provenance")):
        raise ValueError("Expected nonempty source fixtures with provenance")
    seen = set()
    for p in dataset["projects"]:
        if len(safe_path(p["id"]).parts) != 1 or p["id"] in seen:
            raise ValueError("Source project IDs must be unique safe names")
        seen.add(p["id"])
        if (p.get("split") not in (("regression", "development", "held_out") if pinned else ("regression",))
                or not p.get("files") or not p.get("queries")):
            raise ValueError("Source fixtures must declare their split, files and queries")
        if pinned:
            validate_questions(p, load_sources(p))
        else:
            if any(q.get("origin", {}).get("kind", "authored_fixture") != "authored_fixture"
                   for q in p["queries"]):
                raise ValueError("Inline synthetic sources cannot claim captured-query provenance")
            with tempfile.TemporaryDirectory(prefix="graphicle-quality-labels-") as tmp:
                source_project(p, Path(tmp))


def project_language(project):
    """Count a mixed-language project once; unknown extensions stay explicit."""
    names = {".py": "python", ".js": "javascript", ".ts": "typescript", ".tsx": "typescript",
             ".go": "go", ".java": "java", ".kt": "kotlin", ".rs": "rust",
             ".tf": "hcl", ".tfvars": "hcl", ".hcl": "hcl", ".sh": "bash", ".bash": "bash",
             ".yaml": "yaml", ".yml": "yaml", ".tpl": "gotemplate"}
    languages = sorted({names.get(Path(name).suffix.lower(), "unknown") for name in project["files"]})
    return languages[0] if len(languages) == 1 else "mixed:" + "+".join(languages)


def project_coverage(project):
    """Explicit fixture domain, separate from inferred syntax (Helm vs YAML)."""
    group = project.get("coverage_group")
    if group is None:
        return project_language(project)
    if not isinstance(group, str) or not re.fullmatch(r"[a-z][a-z0-9_-]{0,39}", group):
        raise ValueError("Coverage groups must be short lowercase slugs")
    return group


def normalize(entry, project, rows):
    cases = []
    questions = {q["id"]: q for q in project["queries"]}
    for row in rows:
        qid = row.get("id", row.get("query"))
        q = questions[qid]
        for variant, value in row["variants"].items():
            case = {"id": "/".join((entry["id"], project["id"], qid, variant)),
                    "kind": entry["kind"], "variant": variant,
                    "category": q.get("category", q.get("intent")), "split": project["split"],
                    "language": project_language(project),
                    "coverage": project_coverage(project),
                    "origin": q.get("origin", {}).get("kind", "authored_fixture"),
                    "seconds": value.get("seconds"), "tokens": value.get("tokens"),
                    "error": value.get("error"), "content_preserved": row.get("counted_preserves_content")}
            if entry["kind"] == "memory":
                gold, found = set(q["relevant_memories"]), set(value.get("returned", []))
                case.update(relevant=sorted(gold), retrieved_relevant=sorted(gold & found),
                            irrelevant=sorted(found - gold))
                if "memory_diagnostics" in value:
                    case["memory_diagnostics"] = value["memory_diagnostics"]
                if q["category"] == "stale" and found:
                    case["error"] = "Stale memory was returned"
            else:
                case["delivered"] = value.get("required_delivered", [False] * len(q["required"]))
            cases.append(case)
    return cases


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(len(values) * p) - 1)] if values else None


def summary(cases):
    result = {}
    for kind, variant in sorted({(c["kind"], c["variant"]) for c in cases}):
        rows = [c for c in cases if (c["kind"], c["variant"]) == (kind, variant)]
        ok = [c for c in rows if not c["error"]]
        times = [c["seconds"] * 1000 for c in ok if c["seconds"] is not None]
        tokens = [c["tokens"] for c in ok]
        metrics = {"planned": len(rows), "completed": len(ok), "errors": len(rows) - len(ok),
                   "error_rate": (len(rows) - len(ok)) / len(rows),
                   "total_tokens_completed": sum(tokens),
                   "mean_tokens_completed": statistics.mean(tokens) if tokens else None,
                   "latency_ms_completed": {"n": len(times), "p50": percentile(times, .5),
                       "p95": percentile(times, .95), "p99": percentile(times, .99)}}
        if kind == "memory":
            metrics["diagnostics"] = summarize_memory_diagnostics(rows)
            tp = sum(len(c["retrieved_relevant"]) for c in ok)
            fp = sum(len(c["irrelevant"]) for c in ok)
            gold = sum(len(c["relevant"]) for c in rows)
            positives = [c for c in rows if c["relevant"]]
            negatives = [c for c in rows if not c["relevant"]]
            metrics.update(tp=tp, fp=fp, fn=gold - tp,
                precision_completed=tp / (tp + fp) if tp + fp else None,
                recall=tp / gold if gold else None,
                macro_recall=statistics.mean(0 if c["error"] else len(c["retrieved_relevant"]) / len(c["relevant"])
                                             for c in positives) if positives else None,
                negative_queries=len(negatives),
                empty_memory_accuracy=sum(not c["error"] and not c["irrelevant"] for c in negatives) / len(negatives)
                                      if negatives else None)
        else:
            required = sum(len(c["delivered"]) for c in rows)
            delivered = sum(sum(c["delivered"]) for c in ok)
            metrics.update(required=required, delivered=delivered,
                           evidence_recall=delivered / required if required else None,
                           complete=sum(all(c["delivered"]) for c in ok),
                           completeness=sum(all(c["delivered"]) for c in ok) / len(rows))
        result[kind + "/" + variant] = metrics
    return result


def validate_report(report):
    if report.get("format") != FORMAT or not report.get("cases") or not report.get("contract"):
        raise ValueError("Expected a nonempty quality report")
    seen = set()
    for c in report["cases"]:
        if c["id"] in seen:
            raise ValueError("Duplicate case ID")
        seen.add(c["id"])
        if c["kind"] == "memory":
            gold, tp, fp = (c[k] for k in ("relevant", "retrieved_relevant", "irrelevant"))
            if (any(not isinstance(v, list) or len(v) != len(set(v)) for v in (gold, tp, fp))
                    or not set(tp) <= set(gold) or set(fp) & set(gold)):
                raise ValueError("Invalid memory outcomes")
        elif c["kind"] == "source":
            if not c["delivered"] or any(type(v) is not bool for v in c["delivered"]):
                raise ValueError("Invalid evidence outcomes")
        else:
            raise ValueError("Unknown case kind")
    if seen != set(report["contract"]["case_ids"]):
        raise ValueError("Report does not contain every planned case and variant")
    return report


def gate(report, baseline=None):
    validate_report(report)
    findings = [{"case": c["id"], "reason": c["error"] or "Counted content parity failed or was not measured"}
                for c in report["cases"] if c["error"] or
                (c["kind"] == "memory" and c["content_preserved"] is not True)]
    findings += [{"case": "suite", "reason": error} for error in report.get("integrity_errors", [])]
    if baseline is not None:
        validate_report(baseline)
        findings += [{"case": "suite", "reason": "Invalid baseline: " + error}
                     for error in baseline.get("integrity_errors", [])]
        if report["contract"] != baseline["contract"]:
            findings.append({"case": "suite", "reason": "Incompatible dataset, request, or scoring contract"})
        current, before = ({c["id"]: c for c in r["cases"]} for r in (report, baseline))
        if current.keys() != before.keys():
            findings.append({"case": "suite", "reason": "Case inventory changed (including removed variants)"})
        for key in current.keys() & before.keys():
            c, b = current[key], before[key]
            if b["error"] or (b["kind"] == "memory" and b["content_preserved"] is not True):
                findings.append({"case": key, "reason": "Baseline contains an invalid observation"})
                continue
            if c["kind"] != b["kind"]:
                findings.append({"case": key, "reason": "Case kind changed"})
            elif c["kind"] == "memory":
                if c["relevant"] != b["relevant"]:
                    findings.append({"case": key, "reason": "Relevance labels changed"})
                lost = sorted(set(b["retrieved_relevant"]) - set(c["retrieved_relevant"]))
                added = sorted(set(c["irrelevant"]) - set(b["irrelevant"]))
                if lost:
                    findings.append({"case": key, "reason": "Lost relevant memories", "ids": lost})
                if added:
                    findings.append({"case": key, "reason": "Added irrelevant memories", "ids": added})
            elif len(c["delivered"]) != len(b["delivered"]):
                findings.append({"case": key, "reason": "Evidence labels changed"})
            elif any(was and not now for was, now in zip(b["delivered"], c["delivered"])):
                findings.append({"case": key, "reason": "Lost required source evidence"})
    return {"status": "failed" if findings else "passed" if baseline else "unbaselined",
            "findings": sorted(findings, key=lambda f: (f["case"], f["reason"]))}


def markdown(report):
    lines = ["# Graphicle quality scorecard", "", "Gate: **" + report["gate"]["status"] + "**", "",
             "Local regression measurements; these do not estimate real-user or generated-answer quality.", "",
             "| Variant | Completed / planned | Precision | Recall / evidence recall | Empty memory / complete | Mean tokens | p95 ms (n) |",
             "|---|---:|---:|---:|---:|---:|---:|"]
    def percent(value):
        return "unmeasured" if value is None else f"{value:.1%}"
    for name, m in report["summary"].items():
        latency = m["latency_ms_completed"]
        p95 = "unmeasured" if latency["p95"] is None else f"{latency['p95']:.0f} ({latency['n']})"
        tokens = "unmeasured" if m["mean_tokens_completed"] is None else f"{m['mean_tokens_completed']:.0f}"
        lines.append(f"| {name} | {m['completed']} / {m['planned']} | {percent(m.get('precision_completed'))} | "
                     f"{percent(m.get('recall', m.get('evidence_recall')))} | "
                     f"{percent(m.get('empty_memory_accuracy', m.get('completeness')))} | {tokens} | {p95} |")
    for field, title in (("language", "Code language"), ("coverage", "Coverage group"),
                         ("origin", "Query origin")):
        lines += ["", "## " + title, "",
                  "| Group | Variant | Completed / planned | Precision | Recall / evidence recall |",
                  "|---|---|---:|---:|---:|"]
        for group, variants in report.get("by_" + field, {}).items():
            for variant, metrics in variants.items():
                lines.append(f"| {group} | {variant} | {metrics['completed']} / {metrics['planned']} | "
                             f"{percent(metrics.get('precision_completed'))} | "
                             f"{percent(metrics.get('recall', metrics.get('evidence_recall')))} |")
    lines += ["", "Authored fixtures, authored real-source questions and captured diagnostic tool queries are distinct populations.",
              "A captured tool query is not evidence of independent human-query quality. Mixed-language projects count once."]
    lines += ["", "Failed calls remain in recall/completeness denominators; precision, tokens and latency use completed calls only.",
              "Latency includes a new CLI process per query and profiling overhead. Small-sample percentiles are descriptive; no latency gate.",
              "Empty memory means no reviewed notes returned, not answer abstention. Phase durations overlap and must not be summed.", "",
              "## Regressions", ""]
    lines += [f"- `{f['case']}`: {f['reason']}" + (" — " + ", ".join(f["ids"]) if "ids" in f else "")
              for f in report["gate"]["findings"]] or ["No observed regressions." if report["gate"]["status"] == "passed" else "No baseline comparison."]
    lines += ["", "## Not measured", ""] + ["- " + name for name in UNMEASURED]
    lines += ["", "## Memory failure diagnostics", "",
              "Observed stages only: an absent candidate is not proof of an indexing or authorization failure.",
              "Trace coverage and truncation are explicit; diagnostic metrics do not change the regression gate.", ""]
    for name, m in report["summary"].items():
        d = m.get("diagnostics")
        if d is None:
            continue
        lines.append(f"- **{name}**: {d['traced']}/{d['planned']} traced; "
                     f"{d['truncated_traces']} truncated traces; missing stages "
                     f"`{json.dumps(d['missing_by_observed_stage'], sort_keys=True)}`; "
                     f"irrelevant-note routes `{json.dumps(d['false_positives_by_route'], sort_keys=True)}`.")
    lines += ["", "Inspect `cases/<dataset>/<project>/<query>/<variant>.diagnostics.json` and saved packets for failures.",
              "JSON also contains category breakdowns, individual outcomes, corpus hashes and the evaluated binary hash.", ""]
    return "\n".join(lines)


def run(binary, suite, output, baseline=None, source_roots=None):
    entries = load_suite(suite, source_roots)
    # Reserve the run directory atomically; concurrent runs may not overwrite
    # one another's frozen protocol, packets or partial results.
    output.mkdir(parents=True, exist_ok=False)
    contract = {"scoring_version": 1, "request": REQUEST, "memory_variants": VARIANTS,
                "source_variants": {"baseline": False, "routed": True},
                "source_debug_candidates": 400, "memory_debug_candidates": 400,
                "datasets": [{k: e[k] for k in ("id", "kind", "sha256")} for e in entries],
                "case_ids": sorted("/".join((e["id"], p["id"], q["id"], v))
                                   for e in entries for p in e["data"]["projects"] for q in p["queries"]
                                   for v in (VARIANTS if e["kind"] == "memory" else ("baseline", "routed")))}
    protocol = {"contract": contract, "binary_sha256": digest(binary),
                "python": platform.python_version(), "platform": platform.platform(),
                "runner_sha256": {name: digest(Path(__file__).with_name(name)) for name in
                    ("quality.py", "quality_diagnostics.py", "symbol_link_precision.py", "real_query_eval.py", "retrieval_recall.py")},
                "models": "none", "phase_timings": True,
                "source_roots": {e["id"] + "/" + p["id"]: p["source_root"]
                                 for e in entries for p in e["data"]["projects"] if "source_root" in p},
                "memory_debug_candidates": 400, "memory_diagnostics_version": 1,
                "latency_scope": "One fresh CLI process per query; warm store, uncontrolled filesystem cache"}
    save(output / "PROTOCOL.json", protocol)
    cases, grounding, file_grounding = [], {}, {}
    for entry in entries:
        for project in entry["data"]["projects"]:
            destination = output / "cases" / entry["id"]
            if entry["kind"] == "memory":
                rows = evaluate_project(project, binary, destination, diagnose=True)
                if rows and "memory_grounding" in rows[0]:
                    grounding[entry["id"] + "/" + project["id"]] = rows[0]["memory_grounding"]
                    file_grounding[entry["id"] + "/" + project["id"]] = rows[0]["memory_file_grounding"]
            else:
                try:
                    with tempfile.TemporaryDirectory(prefix="graphicle-quality-source-") as tmp:
                        prepared = (project if entry["data"]["format"] == "graphicle.real-query-seed.v1"
                                    else source_project(project, Path(tmp)))
                        rows = run_project(prepared, binary, destination, None, diagnose=True)
                except Exception as error:
                    rows = [{"query": q["id"], "variants": {v: {"error": f"setup: {error}"}
                             for v in ("baseline", "routed")}} for q in project["queries"]]
            cases.extend(normalize(entry, project, rows))
            save(output / "CASES.json", cases)
            print(f"{entry['id']}/{project['id']}: {len(rows)} queries", flush=True)
    report = {"format": FORMAT, "contract": contract, "protocol": protocol, "cases": cases,
              "summary": summary(cases), "unmeasured": UNMEASURED, "memory_grounding": grounding,
              "memory_file_grounding": file_grounding,
              "by_category": {cat: summary([c for c in cases if c["category"] == cat])
                              for cat in sorted({c["category"] for c in cases})}}
    for field in ("language", "coverage", "origin"):
        report["by_" + field] = {key: summary([c for c in cases if c[field] == key])
                                for key in sorted({c[field] for c in cases})}
    inputs = [(binary, protocol["binary_sha256"])]
    inputs += [(suite.parent / e["file"], e["sha256"]) for e in entries]
    inputs += [(Path(__file__).with_name(name), sha) for name, sha in protocol["runner_sha256"].items()]
    report["integrity_errors"] = []
    for entry in entries:
        if entry["data"]["format"] == "graphicle.real-query-seed.v1":
            try:
                validate_source(entry["data"])
            except (ValueError, KeyError, TypeError, OSError) as error:
                report["integrity_errors"].append(f"Pinned source or capture changed: {error}")
    for path, sha in inputs:
        try:
            unchanged = digest(path) == sha
        except OSError:
            unchanged = False
        if not unchanged:
            report["integrity_errors"].append(f"Run input changed during measurement: {path.name}")
    report["gate"] = gate(report, baseline)
    save(output / "SCORECARD.json", report)
    (output / "SCORECARD.md").write_text(markdown(report), encoding="utf-8")
    print(json.dumps(report["gate"], indent=2))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    execute = commands.add_parser("run", help="Run real CLI evaluations in disposable stores")
    execute.add_argument("--binary", type=Path, required=True)
    execute.add_argument("--suite", type=Path, default=DEFAULT_SUITE)
    execute.add_argument("--out", type=Path, required=True)
    execute.add_argument("--baseline", type=Path)
    execute.add_argument("--source-root", action="append", default=[], metavar="DATASET/PROJECT=DIR",
                         help="Relocate pinned source; file and capture hashes must still match")
    compare = commands.add_parser("compare", help="Recheck a saved scorecard against a frozen baseline")
    compare.add_argument("--report", type=Path, required=True)
    compare.add_argument("--baseline", type=Path, required=True)
    freeze = commands.add_parser("freeze", help="Explicitly establish a reviewed regression baseline")
    freeze.add_argument("--report", type=Path, required=True)
    freeze.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "run":
            roots = {}
            for override in args.source_root:
                key, separator, value = override.partition("=")
                if not separator or not key or not value or key in roots:
                    raise ValueError("Source override must uniquely name DATASET/PROJECT=DIR")
                roots[key] = value
            result = run(args.binary.resolve(strict=True), args.suite.resolve(strict=True), args.out.resolve(),
                         read(args.baseline) if args.baseline else None, roots)["gate"]
        elif args.command == "compare":
            result = gate(read(args.report), read(args.baseline))
            print(json.dumps(result, indent=2))
        else:
            report = read(args.report)
            result = gate(report)
            if result["findings"]:
                raise ValueError("Cannot freeze a run containing invalid observations")
            baseline = {"format": FORMAT, "contract": report["contract"],
                        "provenance": {"report_sha256": digest(args.report), "protocol": report["protocol"]},
                        "cases": [{k: v for k, v in c.items() if k not in ("seconds", "tokens", "memory_diagnostics")}
                                  for c in report["cases"]]}
            # Never overwrite an accepted baseline implicitly.
            args.out.parent.mkdir(parents=True, exist_ok=True)
            with args.out.open("x", encoding="utf-8") as stream:
                json.dump(baseline, stream, indent=2)
                stream.write("\n")
        return int(result["status"] == "failed")
    except (ValueError, KeyError, TypeError, OSError) as error:
        parser.exit(2, f"quality: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
