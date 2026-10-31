"""Evidence completeness cannot be satisfied by a title or a wrong revision."""
import hashlib
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from real_query_eval import load_sources, safe_path, score, validate_questions


def fixture():
    raw = b"func Read() {\nreturn nil, err\n}\n"
    files = {"read.go": raw}
    q = {"id": "failure", "intent": "behavior", "origin": {"kind": "authored_real_case"},
         "required": [{"file": "read.go", "byte_start": 14, "byte_end": 27,
                       "line": 2, "quote": "return nil, e"}]}
    # Derive the exact boundary independently of the scorer.
    a = raw.index(b"return")
    q["required"][0].update(byte_start=a, byte_end=a + len(b"return nil, err"), quote="return nil, err")
    e = {"id": "file:read.go", "sha256": hashlib.sha256(raw).hexdigest(),
         "byte_start": 0, "byte_end": len(raw), "text": raw.decode()}
    return files, q, {"tokens": 20, "evidence": [e]}


def test_complete_requires_exact_source_and_all_labels():
    files, q, packet = fixture()
    validate_questions({"queries": [q]}, files)
    assert score(packet, q, files)["complete"]
    e = packet["evidence"][0]
    e.update(byte_end=13, text=files["read.go"][:13].decode())
    assert not score(packet, q, files)["complete"]


def test_location_citation_never_substitutes_for_behavior():
    files, q, packet = fixture()
    packet.update(evidence=[], symbols=[{"definitions": [{"relpath": "read.go", "line": 2}]}])
    assert not score(packet, q, files)["complete"]
    q["intent"] = "location"
    assert score(packet, q, files)["complete"]


def test_verified_graph_definition_can_locate_but_not_explain_clipped_code():
    files, q, packet = fixture()
    raw = files["read.go"]
    q["required"] = [{"file": "read.go", "byte_start": 0, "byte_end": 13,
                      "line": 1, "quote": raw[:13].decode()}]
    e = packet["evidence"][0]
    e.update(byte_end=3, text=raw[:3].decode(), code_graph={
        "relpath": "read.go", "line": 1, "byte_start": 0, "byte_end": len(raw)})
    q["intent"] = "location"
    assert score(packet, q, files)["complete"]
    q["intent"] = "behavior"
    assert not score(packet, q, files)["complete"]
    q["intent"] = "location"
    e["code_graph"]["byte_start"] = 1
    assert not score(packet, q, files)["complete"]
    e["code_graph"] = None
    assert not score(packet, q, files)["complete"]


def test_adjacent_windows_can_complete_a_required_span():
    files, q, packet = fixture()
    original = packet["evidence"][0]
    raw = files["read.go"]
    packet["evidence"] = [dict(original, byte_start=a, byte_end=b, text=raw[a:b].decode())
                          for a, b in [(0, 20), (20, len(raw))]]
    assert score(packet, q, files)["complete"]
    packet["evidence"][1].update(byte_start=21, text=raw[21:].decode())
    assert not score(packet, q, files)["complete"]


def test_repeated_evidence_requires_declaration_disambiguation():
    files, q, _ = fixture()
    files["read.go"] += b"func Other() {\nreturn nil, err\n}\n"
    with pytest.raises(ValueError, match="Repeated"):
        validate_questions({"queries": [q]}, files)
    q["required"][0]["after"] = "func Read()"
    validate_questions({"queries": [q]}, files)


@pytest.mark.parametrize("field,value", [("sha256", "old"), ("text", "invented"), ("byte_end", 9999)])
def test_bad_citation_fails_instead_of_becoming_a_miss(field, value):
    files, q, packet = fixture()
    packet["evidence"][0][field] = value
    with pytest.raises(ValueError):
        score(packet, q, files)


@pytest.mark.parametrize("name", ["../x", "/x", ".env", "a/secrets/x", "a\\b"])
def test_source_paths_are_confined(name):
    with pytest.raises(ValueError):
        safe_path(name)


def test_source_drift_and_symlink_escape_are_rejected(tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    path = root / "x.go"
    path.write_text("changed")
    project = {"source_root": str(root), "files": {"x.go": "old"}}
    with pytest.raises(ValueError, match="hash"):
        load_sources(project)
    path.unlink()
    outside = tmp_path / "outside"
    outside.write_text("external")
    path.symlink_to(outside)
    with pytest.raises(ValueError, match="escaped"):
        load_sources(project)
