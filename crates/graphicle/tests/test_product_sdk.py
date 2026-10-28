"""Product SDK workflow over a real local service, with no model or cloud calls.

GRAPHICLE_BIN selects the program (default: this crate's debug build, Graphicle
Core). Core review uses pinned source arrays and a build profile; legacy
extension contracts remain accessible through the generic command method.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT.parents[1] / "sdk/python"))
from graphicle_sdk import Client, LocalServer, GraphicleError


@pytest.fixture
def product(tmp_path, monkeypatch):
    monkeypatch.delenv("GRAPHICLE_BILLING_CONFIG", raising=False)
    source = Path(os.environ.get("GRAPHICLE_BIN", ROOT / "target/debug/graphicle"))
    assert source.is_file(), "Build from crates/graphicle/ first (its .cargo config bundles parsers)"
    binary = tmp_path / "graphicle"
    shutil.copy2(source, binary)
    # The shared target can be rebuilt by another process; keep a private executable.
    try:
        with LocalServer(binary, tmp_path / "data") as client:
            yield client, client.project("product-sdk", "Product SDK")
    finally:
        binary.unlink(missing_ok=True)


def sync(project, files, commit):
    policy = hashlib.sha256(b"complete SDK fixture v1").hexdigest()
    begin = project.command("sync.prepare", expected_snapshot=project.status()["repository"]["snapshot"],
                            metadata={"commit": commit, "policy_sha256": policy, "scope_complete": True},
                            manifest=[{"path": path, "sha256": hashlib.sha256(content.encode()).hexdigest(), "bytes": len(content.encode())}
                                      for path, content in files.items()])
    if begin["sync_id"] is not None:
        project.command("sync.upload", sync_id=begin["sync_id"], files=[{"path": path, "content": content} for path, content in files.items()])
        project.command("sync.commit", sync_id=begin["sync_id"])
    return {**project.project_graph(), "pin": {"repository_id": project.id, "directory": "fixture", "commit": commit,
                                               "snapshot": begin["snapshot"], "policy_sha256": policy}}


def test_python_client_runs_the_product_workflow_and_budget_recovery(product):
    client, project = product
    first = sync(project, {"auth.py": "def auth():\n    return 'before'\n"}, "a" * 40)
    second = sync(project, {"auth.py": "def auth():\n    return 'after'\n"}, "b" * 40)
    assert project.overview()["graph"]["current_recorded"] is True
    assert len(project.episodes()["episodes"]) == 2
    assert "before" in project.graph_source(first["episode"], "auth.py")["quote"]
    assert project.graph(limit=10, edge_limit=10)["nodes"]
    latest = project.episodes()["current_episode"]
    info = client.command("build.info")
    if info.get("change_impact_scope") == "pinned-go-snapshot-sets-v1":
        impact = project.impact([first["pin"]], [second["pin"]],
                                profile={"goos": "linux", "goarch": "amd64"}, max_tokens=4000)
        assert impact["format"] == "graphicle.review.v1"
        assert impact["diffs"] and impact["impact"] == "incomplete"
        assert any(i["reason"] == "unsupported_source_language_or_contract" for i in impact["coverage"]["issues"])
    elif info["change_impact"]:
        assert project.command("impact.report", base=first["episode"], head=latest, budget_tokens=4000)["changed_nodes"]
    else:
        with pytest.raises(GraphicleError) as missing:
            project.command("impact.report", base=first["episode"], head=latest, budget_tokens=4000)
        assert missing.value.status == 501
    packet = project.context("What does auth return?")
    assert packet["evidence"] and "request_hash" not in packet
    assert project.export_memories()["format"] == "graphicle.memories.v1"
    client.set_budget(client.usage()["requests"])
    with pytest.raises(GraphicleError) as error:
        project.context("What does auth return?")
    assert error.value.status == 429
    assert project.export_memories()["format"] == "graphicle.memories.v1"
    client.set_budget(None)
    assert project.context("What does auth return?")["evidence"]
    assert client.usage()["model_usage"]["calls"] == 0


def test_python_budget_validation_does_not_send_a_request():
    client = Client("https://example.invalid", "workspace", "fixture")
    for value in [-1, 1.2, True, "100", 1_000_000_001]:
        with pytest.raises(ValueError):
            client.set_budget(value)


def test_python_impact_contract_rejects_episode_ids_before_transport(monkeypatch):
    from graphicle_sdk import Project, AsyncProject
    import asyncio
    client = Client("https://example.invalid", "workspace", "fixture")
    calls = []
    monkeypatch.setattr(client, "command", lambda operation, **args: calls.append((operation, args)) or {"packet_id": "receipt"})
    project = Project(client, "anchor")
    pin = {"repository_id": "anchor", "directory": "fixture", "commit": "a" * 40,
           "snapshot": "b" * 64, "policy_sha256": "c" * 64, "modules": ["."]}
    profile = {"goos": "linux", "goarch": "amd64", "tags": ["pilot"], "tests": False}
    for base, head in [("base-episode", "head-episode"), ({}, [pin]), (["episode"], [pin])]:
        with pytest.raises(TypeError, match="pinned snapshot arrays"):
            project.impact(base, head, profile=profile)
    with pytest.raises(TypeError):
        project.impact([pin], [pin])
    with pytest.raises(TypeError, match="budget_tokens"):
        project.impact([pin], [pin], profile=profile, budget_tokens=4000)
    assert calls == []
    project.impact([pin], [pin], profile=profile, max_depth=0, max_tokens=4000, previous_packet_id="d" * 64)
    assert calls[-1] == ("impact.report", {"repository_id": "anchor", "base": [pin], "head": [pin],
                                         "profile": profile, "max_depth": 0, "max_tokens": 4000,
                                         "previous_packet_id": "d" * 64})
    asyncio.run(AsyncProject(project).impact([pin], [pin], profile=profile))
    assert calls[-1][1] == {"repository_id": "anchor", "base": [pin], "head": [pin], "profile": profile}


@pytest.mark.skipif(shutil.which("node") is None, reason="Node is required for cross-SDK integration")
def test_python_and_typescript_pinned_review_agree_over_real_core(product):
    client, project = product
    if client.command("build.info").get("change_impact_scope") != "pinned-go-snapshot-sets-v1":
        pytest.skip("This integration targets Core's pinned review contract")
    files = {"go.mod": "module example/sdk\ngo 1.23\n",
             "main.go": "package sdk\nfunc Target() int { return 1 }\nfunc Caller() int { return Target() }\n"}
    first = sync(project, files, "a" * 40)
    files["main.go"] = files["main.go"].replace("return 1", "return 2")
    second = sync(project, files, "b" * 40)
    profile = {"goos": "linux", "goarch": "amd64", "tests": False}
    packet = project.impact([first["pin"]], [second["pin"]], profile=profile, max_tokens=16000)
    assert packet["format"] == "graphicle.review.v1"
    assert packet["coverage"]["complete"] and packet["changes"]["count"] > 0
    assert any(e["name"].endswith("Caller") for e in packet["entries"])
    script = ROOT.parents[1] / "sdk/typescript/test/pinned-review.mjs"
    completed = subprocess.run([shutil.which("node"), str(script)], input=json.dumps({
        "url": client.url, "workspace": client.workspace, "capability": client._capability,
        "project": project.id, "base": [first["pin"]], "head": [second["pin"]], "profile": profile,
    }), text=True, capture_output=True, timeout=180)
    assert completed.returncode == 0, completed.stderr
    result = json.loads(completed.stdout)
    assert result["packet_id"] == packet["packet_id"]
    assert result["repeated_entries"] == 0 and result["repeated_diffs"] == 0
    assert client.usage()["model_usage"]["calls"] == 0
