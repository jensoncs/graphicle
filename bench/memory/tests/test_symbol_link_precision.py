"""Precision denominators, negative controls, and the real CLI evaluation path."""
import copy
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("GRAPHICLE_BIN", ROOT.parents[1] / "crates/graphicle/target/debug/graphicle"))


@pytest.fixture
def evaluation(monkeypatch):
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    import symbol_link_precision
    return symbol_link_precision


def packet(*ids):
    return {"memories": [{"id": i} for i in ids], "evidence": [], "tokens": 100, "model_calls": 0}


def test_precision_recall_and_empty_results_are_distinct(evaluation):
    known = {"a": "relevant", "b": "distractor"}
    scored = evaluation.score(packet("a", "b"), known, ["relevant"])
    assert (scored["precision"], scored["recall"], scored["tp"], scored["fp"]) == (0.5, 1.0, 1, 1)
    missing = evaluation.score(packet(), known, ["relevant"])
    assert missing["precision"] is None and missing["recall"] == 0 and missing["fn"] == 1
    empty = evaluation.score(packet(), known, [])
    assert empty["precision"] is None and empty["recall"] is None and empty["empty_correct"]
    noisy = evaluation.score(packet("b"), known, [])
    assert noisy["precision"] == 0 and noisy["recall"] is None and not noisy["empty_correct"]


@pytest.mark.parametrize("bad", [
    {}, {"memories": [], "evidence": []},
    {**packet(), "model_calls": 1}, {**packet(), "tokens": 3001}, packet("a", "a"),
    {**packet(), "tokens": True}, {**packet(), "markdown": "x" * 12001},
])
def test_invalid_packets_never_become_successful_empty_recall(evaluation, bad):
    with pytest.raises(ValueError):
        evaluation.score(bad, {"a": "relevant"}, [])


def test_unknown_returned_memory_is_an_error_not_dropped(evaluation):
    with pytest.raises(KeyError):
        evaluation.score(packet("unknown"), {}, [])


def test_local_inference_requires_explicit_opt_in_and_accounting(evaluation):
    local = {**packet("a"), "model_calls": 1,
             "local_reranker": {"model_key": "pinned", "input_tokens": 12}}
    with pytest.raises(ValueError):
        evaluation.score(local, {"a": "a"}, ["a"])
    assert evaluation.score(local, {"a": "a"}, ["a"], allow_local=True)["recall"] == 1
    with pytest.raises(ValueError, match="Remote"):
        evaluation.score({**local, "remote_model_calls": 1}, {"a": "a"}, ["a"], True)
    with pytest.raises(ValueError, match="accounting"):
        evaluation.score({**packet(), "model_calls": 1}, {}, [], True)


def test_local_assets_are_verified_before_evaluation(tmp_path, evaluation):
    model = tmp_path / "model"
    model.mkdir()
    assets = {}
    for name in ("model.onnx", "tokenizer.json"):
        data = name.encode()
        (model / name).write_bytes(data)
        assets[name] = {"sha256": hashlib.sha256(data).hexdigest()}
    (model / "MODEL.json").write_text(json.dumps({"model": "test", "pooling": "logits", "files": assets}))
    ort = tmp_path / "runtime.dylib"
    ort.write_bytes(b"runtime")
    local = evaluation.local_reranker(model, ort, "blend")
    assert local["request"] == {"local_reranker_profile": "reranker", "rerank_mode": "blend"}
    assert len(local["assets"]) == 4
    (model / "model.onnx").write_bytes(b"changed")
    with pytest.raises(ValueError, match="digest mismatch"):
        evaluation.local_reranker(model, ort, "blend")


@pytest.mark.skipif(not (os.environ.get("GRAPHICLE_LOCAL_RERANKER") and os.environ.get("ORT_DYLIB_PATH")
                        and BINARY.exists()), reason="requires explicit local model assets and binary")
def test_shallow_source_rerank_keeps_the_four_linked_memory_slots(tmp_path, evaluation):
    local = evaluation.local_reranker(Path(os.environ["GRAPHICLE_LOCAL_RERANKER"]),
                                     Path(os.environ["ORT_DYLIB_PATH"]), "blend")
    local["request"].update({"rerank_depth": 1, "limit": 1})
    project = json.loads((ROOT / "fixtures/symbol_link_priority.json").read_text())["projects"][0]
    # A whole-pipeline question keeps all six links eligible. An explicit stage
    # now correctly narrows expansion and would no longer exercise the cap.
    project["queries"] = [{"id": "all-stages", "category": "prose_capacity",
                           "question": "What decisions govern this pipeline?",
                           "relevant_memories": [m["id"] for m in project["memories"]],
                           "rationale": "All stage contracts apply to the whole pipeline."}]
    case = evaluation.evaluate_project(project, BINARY, tmp_path / "debug", local, diagnose=True)[0]
    assert case["counted_preserves_content"]
    assert all("error" not in variant for variant in case["variants"].values())
    report = case["variants"]["on"]["symbol_links"]
    assert report["reranker"]["candidates"] == 4
    assert report["lifted_memories"] == 4
    assert report["over_limit"] == 2
    trace = json.loads((tmp_path / "debug" / project["id"] / "all-stages" / "on.packet.json").read_text())["memory_candidates_debug"]
    rescored = [c for c in trace["candidates"] if c["reranker"] is not None]
    assert len(rescored) == 4
    assert all(isinstance(c["reranker"]["score"], (int, float)) for c in rescored)
    assert all(isinstance(c["reranker"]["input_truncated"], bool) for c in rescored)
    plain = evaluation.evaluate_project(project, BINARY, tmp_path / "plain", local)[0]
    for variant in evaluation.VARIANTS:
        assert plain["variants"][variant]["returned"] == case["variants"][variant]["returned"]
    # A deeper candidate prefix distinguishes the final four-note cap from the
    # pre-reranker prefix cap exercised above.
    local["request"]["rerank_depth"] = 8
    deep = evaluation.evaluate_project(project, BINARY, tmp_path / "deep", local, diagnose=True)[0]
    assert deep["counted_preserves_content"]
    detail = deep["variants"]["on"]["memory_diagnostics"]
    assert len(detail["missing"]) == 2
    assert all(m["stage"] == "linked_final_cap" for m in detail["missing"])


def test_setup_failure_records_every_planned_variant(tmp_path, monkeypatch, evaluation):
    project = json.loads(evaluation.DEFAULT_DATASET.read_text())["projects"][0]
    monkeypatch.setattr(evaluation.subprocess, "run", lambda *a, **k:
                        SimpleNamespace(returncode=1, stderr="sync failed", stdout=""))
    cases = evaluation.evaluate_project(project, Path("unused"), tmp_path)
    assert len(cases) == len(project["queries"])
    for case in cases:
        assert set(case["variants"]) == set(evaluation.VARIANTS)
        assert all("sync failed" in v["error"] for v in case["variants"].values())


def test_counted_comparison_ignores_diagnostics_but_not_delivered_content(evaluation):
    original = {**packet("a"), "markdown": "a", "symbol_links": {"status": "disabled"}}
    counted = {**original, "symbol_links": {"status": "counted", "withheld": 1}}
    assert evaluation.packet_content(original) == evaluation.packet_content(counted)
    counted["markdown"] = "different evidence"
    assert evaluation.packet_content(original) != evaluation.packet_content(counted)


def test_failures_remain_in_denominators_and_pairs_report_exclusions(evaluation):
    good = evaluation.score(packet("a"), {"a": "a"}, ["a"])
    empty = evaluation.score(packet(), {}, [])
    cases = [
        {"relevant_memories": ["a"], "variants": {v: good for v in evaluation.VARIANTS}},
        {"relevant_memories": ["a"], "variants": {v: {"error": "crash"} for v in evaluation.VARIANTS}},
        {"relevant_memories": [], "variants": {v: empty for v in evaluation.VARIANTS}},
        {"relevant_memories": [], "variants": {v: {"error": "timeout"} for v in evaluation.VARIANTS}},
    ]
    result = evaluation.summarize(cases)
    for row in result["variants"].values():
        assert row["macro_recall"] == row["empty_memory_accuracy"] == 0.5
        assert row["planned"] == 4 and row["completed"] == 2 and row["failed"] == 2
        assert row["micro_precision_completed"] == 1
    assert result["on_minus_off"]["paired"] == result["on_minus_off"]["excluded"] == 2
    failed = evaluation.summarize(cases[1:2])
    assert failed["variants"]["on"]["micro_precision_completed"] is None
    assert failed["on_minus_off"]["mean_tp_delta"] is None


@pytest.mark.parametrize("mutation", ["path", "gold", "duplicate_query", "split", "rationale"])
def test_dataset_validation(evaluation, mutation):
    data = json.loads(evaluation.DEFAULT_DATASET.read_text())
    evaluation.validate(data)
    data = copy.deepcopy(data)
    project = data["projects"][0]
    if mutation == "path":
        project["updates"] = {"../escape.py": "oops"}
    elif mutation == "gold":
        project["queries"][0]["relevant_memories"] = ["unlabelled"]
    elif mutation == "duplicate_query":
        project["queries"].append(project["queries"][0])
    elif mutation == "split":
        del project["split"]
    else:
        del project["queries"][0]["rationale"]
    with pytest.raises(ValueError):
        evaluation.validate(data)


@pytest.mark.skipif(not BINARY.exists(), reason="build the graphicle binary first")
def test_cli_scores_real_packets_and_stale_memories(tmp_path, evaluation):
    dataset = json.loads(evaluation.DEFAULT_DATASET.read_text())
    dataset["projects"] = [p for p in dataset["projects"] if p["id"] in ("billing", "stale")]
    source = tmp_path / "dataset.json"
    source.write_text(json.dumps(dataset))
    command = [sys.executable, str(ROOT / "tools/symbol_link_precision.py"), "--binary", str(BINARY),
               "--dataset", str(source), "--out", str(tmp_path / "run")]
    proc = subprocess.run(command, capture_output=True, text=True, timeout=300)
    assert proc.returncode == 0, proc.stderr + proc.stdout
    result = json.loads((tmp_path / "run/RESULTS.json").read_text())
    assert result["protocol"]["model_calls"] == 0
    assert result["protocol"]["split"] == "regression"
    assert len(result["protocol"]["dataset_sha256"]) == 64
    assert result["counted_content_mismatches"] == 0
    for row in result["summary"]["variants"].values():
        assert row["completed"] == 8 and row["failed"] == 0
    for row in result["by_category"]["stale"]["variants"].values():
        assert row["empty_memory_accuracy"] == 1
    assert (tmp_path / "run/cases/billing/linked-rationale/on.packet.json").exists()
    rerun = subprocess.run(command, capture_output=True, text=True, timeout=15)
    assert rerun.returncode != 0 and "must be empty" in rerun.stderr
