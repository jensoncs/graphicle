from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from truncation_eval import evaluate, visible, scored_visibility
import hashlib
import pytest


def test_visibility_needs_the_whole_required_span():
    assert visible({"visible":[[2,5],[6,9]]}, 2, 9)
    assert not visible({"visible":[[2,5],[6,9]]}, 2, 10)
    assert not visible({"visible":[]}, 0, 1)


def test_later_branch_visibility_is_scored_in_the_correct_repository():
    text = "prefix\n\treturn FAILURE;\n"
    query = {"id":"failure", "required":[{"repository_id":"server", "file":"a.go", "byte_start":7, "byte_end":len(text)}]}
    evidence = [{"repository_id":"server", "id":"file:a.go", "byte_start":0, "text":text}]
    runs = {"results":[[
        {"kind":"full", "visible":[[0,6]], "score":1, "tokens":256, "seconds":1},
        {"kind":"window", "visible":[[8,23]], "score":2, "tokens":60, "seconds":1}]]}
    result = evaluate(query, evidence, runs)
    assert result["arms"]["full"]["required_visible"] == 0
    assert result["arms"]["window"]["required_visible"] == 1
    evidence[0]["repository_id"] = "client"
    assert evaluate(query, evidence, runs)["arms"]["window"]["required_visible"] == 0


def test_actual_inputs_require_pinned_bytes_and_all_nonwhitespace_tokens():
    raw = b"check(); return failure;"
    query = {"projects":["server"], "required":[{"project":"server","file":"a.go","byte_start":0,"byte_end":len(raw)}]}
    item = {"id":"file:a.go","byte_start":0,"byte_end":len(raw),"passage_sha256":hashlib.sha256(raw).hexdigest(),"visible_token_byte_ranges":[[0,8],[9,len(raw)]]}
    packet = {"reranker_inputs_debug":[item],"evidence":[]}
    files, repos = {"server":{"a.go":raw}}, {"server":"repo"}
    assert scored_visibility(packet,query,files,repos)["required_visible"] == 1
    item["visible_token_byte_ranges"] = [[0,8],[16,len(raw)]]
    assert scored_visibility(packet,query,files,repos)["required_visible"] == 0
    item["passage_sha256"] = "stale"
    with pytest.raises(ValueError,match="pinned source"):
        scored_visibility(packet,query,files,repos)


def test_actual_inputs_do_not_borrow_other_repository_or_delivered_evidence():
    raw = b"return failure;"
    query = {"projects":["client","server"],"required":[{"project":"server","file":"a.go","byte_start":0,"byte_end":len(raw)}]}
    item = {"id":"file:a.go","byte_start":0,"byte_end":len(raw),"passage_sha256":hashlib.sha256(raw).hexdigest(),"visible_token_byte_ranges":[[0,len(raw)]]}
    packet = {"projects":[{"repository_id":"c","reranker_inputs_debug":[item]}],"evidence":[{"text":raw.decode()}]}
    assert scored_visibility(packet,query,{p:{"a.go":raw} for p in query["projects"]},{"client":"c","server":"s"})["required_visible"] == 0
