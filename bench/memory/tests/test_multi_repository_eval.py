"""Repository identity and pinned bytes are independent completeness gates."""
import hashlib
from pathlib import Path
import sqlite3
import sys

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from full_repository_eval import indexed_sources
from multi_repository_eval import score_scoped


def fixture():
    files = {"atlas": {"same.go": b"client()"}, "nexus": {"same.go": b"server()"}}
    repositories = {"atlas": "repo-a", "nexus": "repo-n"}
    query = {"projects": list(files), "intent": "behavior", "required": []}
    packet = {"evidence": [], "tokens": 20, "markdown": "context"}
    for pid, entries in files.items():
        for name, raw in entries.items():
            query["required"].append({"project": pid, "file": name, "byte_start": 0,
                                      "byte_end": len(raw), "line": 1})
            packet["evidence"].append({"repository_id": repositories[pid], "id": "file:" + name,
                                        "sha256": hashlib.sha256(raw).hexdigest(),
                                        "byte_start": 0, "byte_end": len(raw), "text": raw.decode()})
    return packet, query, files, repositories


def test_same_path_in_two_repositories_needs_both_bodies():
    packet, query, files, repositories = fixture()
    assert score_scoped(packet, query, files, repositories)["complete"]
    packet["evidence"].pop()
    result = score_scoped(packet, query, files, repositories)
    assert not result["complete"]
    assert result["required_delivered"] == 1
    assert result["per_project"]["atlas"]["complete"]
    assert not result["per_project"]["nexus"]["complete"]


@pytest.mark.parametrize("mutation", ["wrong_repo", "missing_repo", "stale", "forged", "tokens", "chars", "remote"])
def test_rejects_invalid_provenance_and_budgets(mutation):
    packet, query, files, repositories = fixture()
    if mutation == "wrong_repo":
        packet["evidence"][0]["repository_id"] = "other"
    elif mutation == "missing_repo":
        del packet["evidence"][0]["repository_id"]
    elif mutation == "stale":
        packet["evidence"][0]["sha256"] = "old"
    elif mutation == "forged":
        packet["evidence"][0]["text"] = "forged()"
    elif mutation == "tokens":
        packet["tokens"] = 3001
    elif mutation == "chars":
        packet["markdown"] = "x" * 12001
    else:
        packet["remote_model_calls"] = 1
    with pytest.raises(ValueError):
        score_scoped(packet, query, files, repositories)


def test_definition_metadata_cannot_replace_behavior():
    packet, query, files, repositories = fixture()
    packet["symbols"] = [{"definitions": [{"relpath": "same.go", "line": 1}]}]
    packet["evidence"] = []
    assert not score_scoped(packet, query, files, repositories)["complete"]


def test_shared_store_requires_repository_filter(tmp_path):
    db = sqlite3.connect(tmp_path / "reviews.sqlite3")
    db.executescript('CREATE TABLE repo_heads(repository_id TEXT,path TEXT,sha256 TEXT);'
                     'CREATE TABLE repo_objects(repository_id TEXT,sha256 TEXT,content BLOB);')
    for pid, raw in [("a", b"first"), ("b", b"second")]:
        sha = hashlib.sha256(raw).hexdigest()
        db.execute('INSERT INTO repo_heads VALUES(?,?,?)', (pid, "same.go", sha))
        db.execute('INSERT INTO repo_objects VALUES(?,?,?)', (pid, sha, raw))
    db.commit()
    db.close()
    assert indexed_sources(tmp_path, "a") == {"same.go": b"first"}
    assert indexed_sources(tmp_path, "b") == {"same.go": b"second"}
    with pytest.raises(ValueError, match="select one repository"):
        indexed_sources(tmp_path)


def test_requests_keep_labels_out_and_use_scope_specific_options():
    from multi_repository_eval import make_request
    local = {"request": {"rerank_mode": "blend"}}
    single = make_request({"projects": ["atlas"], "question": "why", "required": ["gold"]}, local, False, 2)
    assert single["scope"] == "project" and "max_projects" not in single
    assert "required" not in single and "projects" not in single
    multi = make_request({"projects": ["atlas", "nexus"], "question": "why"}, local, True, 2)
    assert multi["scope"] == "workspace" and multi["max_projects"] == 2
    assert multi["max_tokens"] == single["max_tokens"] == 3000


def test_defaults_arm_leaves_retrieval_switches_unset():
    from multi_repository_eval import make_request
    request = make_request({"projects": ["atlas", "nexus"], "question": "why"},
                           {"request": {"local_reranker_profile": "reranker", "rerank_mode": "blend"}},
                           None, 2, defaults=True)
    assert request == {"question": "why", "scope": "workspace", "max_tokens": 3000,
                       "max_chars": 12000, "local_reranker_profile": "reranker", "rerank_mode": "blend"}
