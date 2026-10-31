"""Historical checkout policy and tracked distractors are part of the test."""
import hashlib
from pathlib import Path
import subprocess
import sys

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from full_repository_eval import excluded, materialize


@pytest.mark.parametrize("name,mode", [
    (".env.production", "100644"), ("app/secrets/a", "100644"),
    ("app/.envrc", "100644"), ("keys/test.pem", "100644"),
    (".claude/settings.local.json", "100644"), ("a", "120000"), ("sub", "160000"),
    ("../outside", "100644"), ("/absolute", "100644"),
    ("C:/outside", "100644"),
])
def test_excludes_before_blob_reads(name, mode):
    assert excluded(name, mode)


def test_materialization_requires_an_immutable_commit_pin(tmp_path):
    with pytest.raises(ValueError, match="commit pin"):
        materialize(tmp_path, "HEAD", tmp_path / "checkout")
    assert not (tmp_path / "checkout").exists()


def test_materializes_exact_revision_and_keeps_tracked_ignored_distractors(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    def git(*args):
        return subprocess.check_output(["git", "-C", str(repo), *args]).decode().strip()
    git("init", "-q")
    (repo / ".gitignore").write_text("distractor.go\n")
    (repo / "main.go").write_text("package old\n")
    (repo / "distractor.go").write_text("package noise\n")
    (repo / ".env").write_text("TEST_PLACEHOLDER=not-a-credential\n")
    (repo / "link.go").symlink_to("main.go")
    git("add", "-f", ".")
    git("-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm",
        "fixture\n\nCo-authored-by: Codex <noreply@openai.com>")
    commit = git("rev-parse", "HEAD")
    (repo / "main.go").write_text("package newer\n")
    destination = tmp_path / "checkout"
    result = materialize(repo, commit, destination)
    assert (destination / "main.go").read_text() == "package old\n"
    assert (destination / "distractor.go").read_text() == "package noise\n"
    assert not (destination / ".env").exists()
    assert not (destination / "link.go").exists()
    assert result["files"]["main.go"]["sha256"] == hashlib.sha256(b"package old\n").hexdigest()
    tracked = subprocess.check_output(["git", "-C", str(destination), "ls-files", "--cached", "--ignored", "--exclude-standard"]).decode()
    assert "distractor.go" in tracked
