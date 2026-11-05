"""A quality gate must reject regressions, incomplete runs and incomparable data."""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import quality
from quality_diagnostics import diagnostics, memory_diagnostics, summarize_memory_diagnostics


def test_memory_diagnostics_separate_caps_filters_packing_and_unknowns():
    candidates = [
        {"id": "packed", "pack_rank": 1},
        {"id": "rank-cap", "link_final_rank": 5},
        {"id": "prefix-cap", "link_outcomes": {"candidate_limit": 1}},
        {"id": "filtered", "link_outcomes": {"outside_symbol_scope": 1}},
        {"id": "unselected", "fusion": {"rank": 3, "score": .1}},
    ]
    gold = [c["id"] for c in candidates] + ["unobserved"]
    packet = {"memories": [{"id": "noise", "via": "link", "link": {"qualified_name": "alpha"}}],
              "memory_candidates_debug": {"version": 1, "truncated": True,
                                          "linked_memory_limit": 4, "candidates": candidates}}
    detail = memory_diagnostics(packet, {k: k for k in gold + ["noise"]}, gold)
    assert {m["id"]: m["stage"] for m in detail["missing"]} == {
        "packed": "packing", "rank-cap": "linked_final_cap", "prefix-cap": "linked_candidate_cap",
        "filtered": "link_withheld", "unselected": "eligible_not_selected",
        "unobserved": "not_observed_in_candidates"}
    assert detail["false_positives"][0]["route"] == "symbol_link"
    assert detail["false_positives"][0]["linked_symbol"] == "alpha"
    assert detail["observed_candidate_relevant"] == 5
    assert detail["observed_packing_relevant"] == 1
    summary = summarize_memory_diagnostics([{"memory_diagnostics": detail}, {"error": "timeout"}])
    assert summary["planned"] == 2 and summary["traced"] == 1 and summary["unmeasured"] == 1
    assert summary["truncated_traces"] == 1
    assert summary["observed_candidate_recall_lower_bound"] == 5 / 6


def test_memory_rank_metrics_and_unavailable_diagnostics_are_honest():
    packet = {"memories": [{"id": "noise"}, {"id": "useful"}]}
    detail = memory_diagnostics(packet, {k: k for k in ("noise", "useful")}, ["useful", "missing"])
    assert detail["reciprocal_rank"] == .5
    assert detail["at_k"]["1"]["precision_returned"] == 0
    assert detail["at_k"]["4"]["precision_returned"] == .5
    assert detail["at_k"]["4"]["recall"] == .5
    assert detail["missing"][0]["stage"] == "diagnostics_unavailable"
    assert detail["observed_candidate_relevant"] is None
    assert not detail["trace_available"]
    empty = memory_diagnostics({"memories": []}, {}, [])
    assert empty["reciprocal_rank"] is None and empty["at_k"]["1"]["precision_returned"] is None
    with pytest.raises(KeyError):
        memory_diagnostics({"memories": [{"id": "unknown"}]}, {}, [])


def test_relevance_filter_is_reported_without_hiding_a_recall_miss():
    packet = {"memories": [],
              "memory_topic_support": {"status": "unsupported_in_candidates"},
              "memory_candidates_debug": {"version": 1, "truncated": False,
                  "linked_memory_limit": 4, "candidates": [
                      {"id": "useful", "selection_filter": "unsupported_topic",
                       "link_candidate_rank": 1, "link_outcomes": {"candidate": 1}}]}}
    detail = memory_diagnostics(packet, {"useful": "useful"}, ["useful"])
    assert detail["missing"][0]["stage"] == "relevance_withheld"
    assert detail["missing"][0]["candidate"]["selection_filter"] == "unsupported_topic"
    assert detail["at_k"]["8"]["recall"] == 0
    assert diagnostics(packet, "")["memory_topic_support"] == packet["memory_topic_support"]
    baseline = report(memory(gold=("useful",), returned=("useful",)))
    candidate = report(memory(gold=("useful",), returned=()))
    candidate["cases"][0]["memory_diagnostics"] = detail
    assert quality.gate(candidate, baseline)["status"] == "failed"


def memory(name="positive", gold=("constraint",), returned=("constraint",)):
    return {"id": name, "kind": "memory", "variant": "on", "category": "test", "split": "regression",
            "error": None, "content_preserved": True, "seconds": .1, "tokens": 100,
            "relevant": sorted(gold), "retrieved_relevant": sorted(set(gold) & set(returned)),
            "irrelevant": sorted(set(returned) - set(gold))}


def source(name="source", delivered=(True, False)):
    return {"id": name, "kind": "source", "variant": "baseline", "category": "behavior", "split": "regression",
            "error": None, "content_preserved": None, "seconds": .2, "tokens": 200,
            "delivered": list(delivered)}


def report(*cases):
    cases = list(cases) or [memory(), source()]
    return {"format": quality.FORMAT, "contract": {"case_ids": sorted(c["id"] for c in cases)},
            "protocol": {"binary_sha256": "baseline"}, "cases": cases}


def test_paired_gate_accepts_improvements_without_rewarding_missing_evidence():
    before = report()
    candidate = copy.deepcopy(before)
    candidate["cases"][1]["delivered"] = [True, True]
    assert quality.gate(candidate, before)["status"] == "passed"
    candidate["cases"][0]["retrieved_relevant"] = []
    result = quality.gate(candidate, before)
    assert result["status"] == "failed"
    assert result["findings"][0]["reason"] == "Lost relevant memories"


def test_more_recall_does_not_hide_irrelevant_memory_or_source_loss():
    before = report()
    candidate = copy.deepcopy(before)
    candidate["cases"][0]["irrelevant"] = ["unrelated"]
    candidate["cases"][1]["delivered"] = [False, True]
    reasons = {f["reason"] for f in quality.gate(candidate, before)["findings"]}
    assert reasons == {"Added irrelevant memories", "Lost required source evidence"}


@pytest.mark.parametrize("change", ["missing", "duplicate", "empty", "wrong_labels"])
def test_invalid_observations_cannot_pass_or_become_a_baseline(change):
    candidate = report()
    if change == "missing":
        candidate["cases"].pop()
    elif change == "duplicate":
        candidate["cases"].append(candidate["cases"][0])
    elif change == "empty":
        candidate["cases"] = []
    else:
        candidate["cases"][0]["retrieved_relevant"] = ["unlabelled"]
    with pytest.raises(ValueError):
        quality.gate(candidate)


def test_changed_corpus_or_removed_variant_is_not_a_comparable_trial():
    before = report()
    candidate = report(memory())
    reasons = {f["reason"] for f in quality.gate(candidate, before)["findings"]}
    assert any("inventory" in r for r in reasons)
    assert any("contract" in r for r in reasons)


@pytest.mark.parametrize("parity", [False, None])
def test_missing_or_failed_content_parity_blocks_even_an_unbaselined_run(parity):
    candidate = report(memory())
    candidate["cases"][0]["content_preserved"] = parity
    assert quality.gate(candidate)["status"] == "failed"


def test_input_drift_cannot_be_frozen_or_pass_a_gate():
    candidate = report()
    candidate["integrity_errors"] = ["Run input changed during measurement: graphicle"]
    assert quality.gate(candidate)["status"] == "failed"


def test_failed_calls_remain_in_quality_denominators():
    positive = memory()
    negative = memory("negative", (), ())
    failed_positive = {**memory("failed-positive"), "error": "timeout", "tokens": None}
    failed_negative = {**memory("failed-negative", (), ()), "error": "crash", "tokens": None}
    metrics = quality.summary([positive, negative, failed_positive, failed_negative])["memory/on"]
    assert metrics["planned"] == 4 and metrics["completed"] == 2
    assert metrics["recall"] == metrics["macro_recall"] == metrics["empty_memory_accuracy"] == .5
    assert metrics["precision_completed"] == 1 and metrics["error_rate"] == .5
    assert metrics["latency_ms_completed"]["n"] == 2
    assert quality.summary([failed_negative])["memory/on"]["precision_completed"] is None
    evidence = quality.summary([source(delivered=(True,)), {**source("failed", (False,)), "error": "crash"}])
    assert evidence["source/baseline"]["completeness"] == .5
    assert evidence["source/baseline"]["evidence_recall"] == .5


def test_percentiles_are_nearest_rank_and_keep_sample_size():
    assert quality.percentile([], .95) is None
    assert quality.percentile(list(range(1, 101)), .95) == 95
    assert quality.percentile([100], .99) == 100


def test_diagnostics_preserve_nested_phases_and_bound_candidate_claims():
    packet = {"candidates_debug": [{"id": "file:code.py", "byte_start": 5, "byte_end": 20}]}
    labels = [{"file": "code.py", "byte_start": 5, "byte_end": 21}]
    stderr = "phase=outer ms=12 rss_mb=10.1 peak_rss_mb=11.0\nphase=inner ms=8 rss_mb=10.0 peak_rss_mb=11.0\nignored\n"
    detail = diagnostics(packet, stderr, labels)
    assert [p["ms"] for p in detail["phases"]] == [12, 8]
    assert detail["required_spans"][0]["contained_in_observed_candidate"] is False
    assert diagnostics({}, "", labels)["required_spans"][0]["contained_in_observed_candidate"] is None
    labels[0]["byte_end"] = 20
    assert diagnostics(packet, "", labels)["required_spans"][0]["contained_in_observed_candidate"] is True


def test_inline_source_labels_are_exact_utf8_bytes(tmp_path):
    project = {"id": "unicode", "split": "regression", "files": {"code.py": "# reçu\nreturn 'é'\n"},
               "queries": [{"id": "q", "intent": "behavior", "required": [{"file": "code.py", "quote": "return 'é'"}]}]}
    prepared = quality.source_project(project, tmp_path)
    label = prepared["queries"][0]["required"][0]
    raw = (tmp_path / "code.py").read_bytes()
    assert raw[label["byte_start"]:label["byte_end"]].decode() == "return 'é'"
    assert label["line"] == 2
    assert prepared["queries"][0]["origin"]["kind"] == "authored_fixture"
    project["files"]["code.py"] += "return 'é'\n"
    with pytest.raises(ValueError, match="unique"):
        quality.source_project(project, tmp_path)


def cli(*args):
    return subprocess.run([sys.executable, str(ROOT / "tools/quality.py"), *map(str, args)],
                          capture_output=True, text=True, timeout=300)


def test_cli_freeze_is_explicit_and_comparison_returns_failure(tmp_path):
    path, baseline = tmp_path / "report.json", tmp_path / "baseline.json"
    candidate = report()
    candidate["cases"][0]["memory_diagnostics"] = {"debug_only": True}
    path.write_text(json.dumps(candidate))
    assert cli("freeze", "--report", path, "--out", baseline).returncode == 0
    assert "memory_diagnostics" not in quality.read(baseline)["cases"][0]
    assert cli("freeze", "--report", path, "--out", baseline).returncode == 2
    assert cli("compare", "--report", path, "--baseline", baseline).returncode == 0
    candidate["cases"][0]["retrieved_relevant"] = []
    path.write_text(json.dumps(candidate))
    failed = cli("compare", "--report", path, "--baseline", baseline)
    assert failed.returncode == 1 and "Lost relevant" in failed.stdout
    candidate["cases"][0]["error"] = "setup failed"
    path.write_text(json.dumps(candidate))
    assert cli("freeze", "--report", path, "--out", tmp_path / "bad.json").returncode == 2


def test_manifest_is_portable_and_has_both_quality_dimensions():
    entries = quality.load_suite(quality.DEFAULT_SUITE)
    assert {e["kind"] for e in entries} == {"source", "memory"}
    assert sum(len(p["queries"]) for e in entries for p in e["data"]["projects"]) == 195
    assert {quality.project_language(p) for e in entries for p in e["data"]["projects"]} == {
        "python", "javascript", "typescript", "go", "java", "kotlin", "rust",
        "hcl", "yaml", "mixed:gotemplate+yaml", "bash"}
    infrastructure = [e for e in entries if e["id"].startswith("infrastructure-")]
    assert {quality.project_coverage(p) for e in infrastructure for p in e["data"]["projects"]} == {
        "hcl", "terraform", "helm", "kubernetes", "bash"}
    assert all(e["data"]["format"] != "graphicle.real-query-seed.v1" for e in entries)


def test_language_normalization_does_not_double_count_mixed_projects():
    project = {"id": "mixed", "split": "regression", "files": {"a.go": "", "b.ts": ""},
               "queries": [{"id": "q", "intent": "location", "required": [{}],
                            "origin": {"kind": "captured_tool_query"}}]}
    rows = [{"query": "q", "variants": {"routed": {"tokens": 10, "required_delivered": [True]}}}]
    cases = quality.normalize({"id": "real", "kind": "source"}, project, rows)
    assert len(cases) == 1
    assert cases[0]["language"] == "mixed:go+typescript"
    assert cases[0]["origin"] == "captured_tool_query"
    assert quality.project_language({"files": {"unrecognized.xyz": ""}}) == "unknown"


def test_infrastructure_coverage_does_not_override_syntax_or_double_count():
    helm = {"coverage_group": "helm", "files": {"Chart.YAML": "", "templates/_helpers.tpl": ""}}
    assert quality.project_language(helm) == "mixed:gotemplate+yaml"
    assert quality.project_coverage(helm) == "helm"
    assert quality.project_language({"files": {"main.tf": "", "production.tfvars": ""}}) == "hcl"
    assert quality.project_language({"files": {"run.sh": "", "lib.bash": ""}}) == "bash"
    for group in ("", "helm|yaml", "../helm", ["helm"], 1):
        with pytest.raises(ValueError, match="Coverage groups"):
            quality.project_coverage({**helm, "coverage_group": group})


def test_helm_gain_cannot_hide_a_kubernetes_regression():
    before = report({**memory("helm", ("one", "two"), ("one",)), "coverage": "helm"},
                    {**memory("k8s"), "coverage": "kubernetes"})
    after = copy.deepcopy(before)
    after["cases"][0]["retrieved_relevant"] = ["one", "two"]
    after["cases"][1]["retrieved_relevant"] = []
    assert quality.gate(after, before)["status"] == "failed"


def test_aggregate_gain_cannot_hide_a_language_regression():
    before = report({**memory("python"), "language": "python"},
                    {**memory("go", ("one", "two"), ("one",)), "language": "go"})
    after = copy.deepcopy(before)
    after["cases"][0]["retrieved_relevant"] = []
    after["cases"][1]["retrieved_relevant"] = ["one", "two"]
    assert quality.summary(before["cases"])["memory/on"]["tp"] == quality.summary(after["cases"])["memory/on"]["tp"]
    assert quality.gate(after, before)["status"] == "failed"


def test_inline_source_cannot_claim_captured_provenance():
    data = quality.read(ROOT / "fixtures/quality_sources.json")
    data["projects"][0]["queries"][0]["origin"] = {"kind": "captured_tool_query"}
    with pytest.raises(ValueError, match="cannot claim"):
        quality.validate_source(data)


def test_pinned_real_source_is_validated_and_relocatable(tmp_path):
    import hashlib
    source_root = tmp_path / "checkout"
    source_root.mkdir()
    body = "package p\nfunc Read() int { return 7 }\n"
    (source_root / "a.go").write_text(body)
    quote = "return 7"
    start = body.index(quote)
    project = {"id": "real", "split": "held_out", "source_root": "missing-checkout",
               "files": {"a.go": hashlib.sha256(body.encode()).hexdigest()}, "queries": [{
                   "id": "behavior", "question": "What does Read return?", "intent": "behavior",
                   "origin": {"kind": "authored_real_case"}, "required": [{"file": "a.go",
                   "quote": quote, "byte_start": start, "byte_end": start + len(quote), "line": 2}]}]}
    dataset = {"format": "graphicle.real-query-seed.v1", "provenance": "Test data", "projects": [project]}
    (tmp_path / "data.json").write_text(json.dumps(dataset))
    manifest = tmp_path / "suite.json"
    manifest.write_text(json.dumps({"format": "graphicle.quality-suite.v1", "datasets": [
        {"id": "pinned", "kind": "source", "file": "data.json"}]}))
    entries = quality.load_suite(manifest, {"pinned/real": str(source_root)})
    assert entries[0]["data"]["projects"][0]["source_root"] == str(source_root)
    with pytest.raises(ValueError, match="Unknown source-root"):
        quality.load_suite(manifest, {"pinned/real": str(source_root), "pinned/missing": str(source_root)})
    (source_root / "a.go").write_text(body + "// changed")
    with pytest.raises(ValueError, match="hash changed"):
        quality.load_suite(manifest, {"pinned/real": str(source_root)})


def test_existing_output_directory_cannot_be_shared_by_runs(tmp_path):
    with pytest.raises(FileExistsError):
        quality.run(Path("unused"), quality.DEFAULT_SUITE, tmp_path)


@pytest.mark.skipif(not os.environ.get("GRAPHICLE_BIN"), reason="requires an explicitly built binary")
def test_real_cli_scorecard_diagnostics_and_gate(tmp_path):
    dataset = quality.read(ROOT / "fixtures/symbol_link_precision.json")
    dataset["projects"] = [dataset["projects"][0]]
    dataset["projects"][0]["queries"] = dataset["projects"][0]["queries"][:1]
    (tmp_path / "memory.json").write_text(json.dumps(dataset))
    (tmp_path / "source.json").write_text((ROOT / "fixtures/quality_sources.json").read_text())
    suite = {"format": "graphicle.quality-suite.v1", "datasets": [
        {"id": "memory", "kind": "memory", "file": "memory.json"},
        {"id": "source", "kind": "source", "file": "source.json"}]}
    manifest = tmp_path / "suite.json"
    manifest.write_text(json.dumps(suite))
    output = tmp_path / "run"
    run = cli("run", "--binary", os.environ["GRAPHICLE_BIN"], "--suite", manifest, "--out", output)
    assert run.returncode == 0, run.stdout + run.stderr
    result = quality.read(output / "SCORECARD.json")
    assert len(result["cases"]) == 15
    assert result["integrity_errors"] == []
    assert result["contract"]["memory_debug_candidates"] == 400
    for variant in ("off", "counted", "on"):
        detail = result["summary"]["memory/" + variant]["diagnostics"]
        assert detail["traced"] == detail["planned"] == 1
    assert result["gate"]["status"] == "unbaselined"
    assert result["by_language"]["python"] == result["summary"]
    assert result["by_origin"]["authored_fixture"] == result["summary"]
    assert set(result["memory_grounding"]["memory/billing"]) == {"settlement", "audit", "receipt", "shipping"}
    assert all(link["state"] == "resolved" for links in result["memory_grounding"]["memory/billing"].values()
               for link in links)
    assert set(result["unmeasured"]) == set(quality.UNMEASURED)
    assert (output / "SCORECARD.md").exists()
    rendered = (output / "SCORECARD.md").read_text()
    assert "| python | memory/on | 1 / 1 |" in rendered
    assert "| authored_fixture | source/routed | 6 / 6 |" in rendered
    details = list(output.glob("cases/**/*.diagnostics.json"))
    assert len(details) == 15
    assert any(quality.read(p)["phases"] for p in details)
    assert any(quality.read(p).get("memory_quality", {}).get("trace_available") for p in details)
    baseline = tmp_path / "baseline.json"
    assert cli("freeze", "--report", output / "SCORECARD.json", "--out", baseline).returncode == 0
    assert cli("compare", "--report", output / "SCORECARD.json", "--baseline", baseline).returncode == 0
