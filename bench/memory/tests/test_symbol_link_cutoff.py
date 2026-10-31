"""Cutoff calibration must preserve recall, replay the cap and avoid test-set fitting."""
import json
import math
from pathlib import Path

import pytest


@pytest.fixture
def cutoff(monkeypatch):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[1] / "tools"))
    import symbol_link_cutoff
    return symbol_link_cutoff


def observation(candidates, gold, fixed=()):
    return {"project": "fixture", "query": "q", "gold": list(gold), "fixed": list(fixed),
            "candidates": [{"memory": name, "score": score} for name, score in candidates]}


def test_cutoff_is_inclusive_and_calibrated_on_raw_scale(cutoff):
    o = observation([("noise", -2), ("useful", 0.2)], ["useful"])
    result = cutoff.calibrate([o])
    assert result["cutoff"] == 0.2
    assert result["selected"]["precision"] == result["selected"]["macro_recall"] == 1
    assert cutoff.returned(o, 0.2) == ["useful"]
    shifted = observation([("noise", 998), ("useful", 1000.2)], ["useful"])
    assert cutoff.calibrate([shifted])["cutoff"] == 1000.2


def test_low_scoring_relevant_note_prevents_precision_at_its_expense(cutoff):
    mixed = observation([("noise", -2), ("useful", 0.2)], ["useful"])
    safeguard = observation([("paraphrased", -3)], ["paraphrased"])
    result = cutoff.calibrate([mixed, safeguard])
    assert result["cutoff"] is None
    assert any(row["recall_losses"] for row in result["sweep"])


def test_filtering_precedes_cap_and_can_recover_a_later_note(cutoff):
    o = observation([(f"noise-{i}", -5 + i) for i in range(4)] + [("useful", 1)], ["useful"])
    assert "useful" not in cutoff.returned(o, None)
    result = cutoff.calibrate([o])
    assert result["cutoff"] == 1
    assert result["selected_outcomes"][0]["returned"] == ["useful"]


def test_ordinary_retrieval_is_never_thresholded(cutoff):
    o = observation([("noise", 0)], ["ordinary"], fixed=["ordinary"])
    assert cutoff.returned(o, 100) == ["ordinary"]
    assert cutoff.calibrate([o])["selected"]["precision"] == 1


def test_lowest_effective_cutoff_wins_a_tie(cutoff):
    o = observation([("first", 1), ("second", 2)], ["first", "second"])
    assert cutoff.calibrate([o])["cutoff"] is None


@pytest.mark.parametrize("value", [math.nan, math.inf, -math.inf])
def test_nonfinite_scores_and_cutoffs_are_rejected(cutoff, value):
    with pytest.raises(ValueError):
        cutoff.calibrate([observation([("useful", value)], ["useful"])])
    with pytest.raises(ValueError):
        cutoff.returned(observation([], ["ordinary"], fixed=["ordinary"]), value)


def test_no_positive_calibration_data_is_rejected(cutoff):
    with pytest.raises(ValueError):
        cutoff.calibrate([observation([("noise", 0)], [])])


@pytest.mark.parametrize("split,matching", [("held_out", True), ("development", False)])
def test_cli_refuses_evaluation_data_or_a_mismatched_dataset(cutoff, tmp_path, monkeypatch, split, matching):
    dataset = tmp_path / "dataset.json"
    dataset.write_text("{}")
    run = tmp_path / "run"
    run.mkdir()
    (run / "PROTOCOL.json").write_text(json.dumps({
        "split": split, "dataset_sha256": cutoff.digest(dataset) if matching else "different"}))
    monkeypatch.setattr("sys.argv", ["calibrate", "--dataset", str(dataset), "--run", str(run),
                                    "--out", str(tmp_path / "selection.json")])
    with pytest.raises(SystemExit) as error:
        cutoff.main()
    assert error.value.code == 2
    assert not (tmp_path / "selection.json").exists()


def test_frozen_selection_cannot_be_overwritten(cutoff, tmp_path, monkeypatch):
    frozen = tmp_path / "selection.json"
    frozen.write_text('{"cutoff": -5}')
    monkeypatch.setattr("sys.argv", ["calibrate", "--dataset", "unused", "--run", "unused",
                                    "--out", str(frozen)])
    with pytest.raises(SystemExit) as error:
        cutoff.main()
    assert error.value.code == 2
    assert frozen.read_text() == '{"cutoff": -5}'
