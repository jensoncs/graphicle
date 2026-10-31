import copy
import json
from pathlib import Path
import sys

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from note_abstention_eval import curve, provenance


def controls():
    return json.loads((Path(__file__).resolve().parents[1] / "fixtures/note_no_answer_controls.json").read_text())


def test_authored_controls_cannot_pass_as_independent_captured_queries():
    data = controls()
    provenance(data)
    with pytest.raises(ValueError, match="captured queries"):
        provenance(data, True)
    data["provenance_kind"] = "captured"
    with pytest.raises(ValueError, match="capture reference"):
        provenance(data, True)


def test_captured_mode_requires_labels_for_every_query():
    data = controls()
    data["provenance_kind"] = "captured"
    for project in data["projects"]:
        for query in project["queries"]:
            query.update(capture_reference="test-fixture-only", labelled_by="fixture-reviewer", independent_labels=True)
    provenance(data, True)
    bad = copy.deepcopy(data)
    bad["projects"][-1]["queries"][-1]["independent_labels"] = False
    with pytest.raises(ValueError):
        provenance(bad, True)


def test_fixed_cap_cannot_hide_a_formatting_recall_regression():
    cases = [{"project":"direct-contracts", "id":"all-direct", "returned":["a","b","c","d"],
              "relevant_memories":["a","b","c","d"]},
             {"project":"negative", "id":"absent", "returned":["noise"], "relevant_memories":[]}]
    result = curve(cases)
    assert result["3"]["safeguards"]["direct-contracts/all-direct"] is False
    assert result["4"]["safeguards"]["direct-contracts/all-direct"] is True
    assert result["4"]["empty_correct"] == 0
    assert result["0"]["recall"] == 0
    assert result["0"]["empty_correct"] == 1
