#!/usr/bin/env python3
"""Replay the frozen real-query seed against complete historical Git trees.

Uses a disposable local clone, normal Graphicle sync policy and unchanged
requests. Never checks out credential paths or symlinks. Records omissions and
checks citations against indexed text, including locally converted documents.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import sqlite3
import subprocess
import time

from real_query_eval import safe_path, score, validate_questions
from symbol_link_precision import REQUEST, digest, local_reranker, save


def excluded(name, mode):
    parts = PurePosixPath(name).parts
    return (mode not in ("100644", "100755") or not parts
            or PurePosixPath(name).is_absolute() or ".." in parts
            or "\\" in name or ":" in name or any(p.startswith(".env") or p in
                ("secrets", ".git", ".ssh", ".aws", ".azure", ".codex", ".graphicle") for p in parts)
            or parts[-1].endswith((".pem", ".key"))
            or parts[-1] in ("credentials", "credentials.json", "secrets.json", ".netrc",
                              ".npmrc", ".pypirc", "local-session.json")
            or name.endswith(".claude/settings.local.json"))


def materialize(repo, commit, destination):
    """Read named safe blobs only; preserve the real tracked-file index/ignores."""
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("A full SHA-1 commit pin is required")
    subprocess.run(["git", "clone", "--shared", "--no-checkout", "--quiet", str(repo), str(destination)], check=True)
    subprocess.run(["git", "-C", str(destination), "update-ref", "HEAD", commit], check=True)
    subprocess.run(["git", "-C", str(destination), "read-tree", commit], check=True)
    raw = subprocess.check_output(["git", "-C", str(repo), "ls-tree", "-rz", "--full-tree", commit])
    inventory, omitted = {}, []
    with subprocess.Popen(["git", "-C", str(repo), "cat-file", "--batch"],
                          stdin=subprocess.PIPE, stdout=subprocess.PIPE) as proc:
        for entry in raw.split(b"\0"):
            if not entry:
                continue
            header, name = entry.split(b"\t", 1)
            mode, kind, oid = header.decode().split()
            name = name.decode()
            if excluded(name, mode) or kind != "blob":
                omitted.append({"path": name, "reason": "credential_path_or_non_regular"})
                continue
            proc.stdin.write(oid.encode() + b"\n")
            proc.stdin.flush()
            got, kind, length = proc.stdout.readline().decode().split()
            assert got == oid and kind == "blob"
            content = proc.stdout.read(int(length))
            assert len(content) == int(length) and proc.stdout.read(1) == b"\n"
            # Bind the materialized bytes to the requested Git object as well.
            assert hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest() == oid
            path = destination / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            inventory[name] = {"git_blob": oid, "sha256": hashlib.sha256(content).hexdigest(), "bytes": len(content)}
        proc.stdin.close()
        assert proc.wait() == 0
    return {"commit": commit, "files": inventory, "excluded_before_read": omitted}


def indexed_sources(store, repository=None):
    with sqlite3.connect(f"file:{store / 'reviews.sqlite3'}?mode=ro", uri=True) as db:
        sql = "SELECT h.path,h.sha256,o.content FROM repo_heads h JOIN repo_objects o ON o.repository_id=h.repository_id AND o.sha256=h.sha256"
        rows = db.execute(sql + (" WHERE h.repository_id=?" if repository else "") + " ORDER BY h.path",
                          (repository,) if repository else ()).fetchall()
    files = {}
    for name, sha, content in rows:
        if name in files:
            raise ValueError("Duplicate indexed path; select one repository")
        raw = content.encode() if isinstance(content, str) else content
        assert hashlib.sha256(raw).hexdigest() == sha
        files[name] = raw
    return files


def run_project(project, repo, binary, out, local):
    commit = project.get("commit") or project["pin"][0]["commit"]
    root, store = out / "checkout", out / "store"
    out.mkdir(parents=True)
    manifest = materialize(repo, commit, root)
    save(out / "SOURCE-MANIFEST.json", manifest)
    for name, sha in project["files"].items():
        assert manifest["files"][name]["sha256"] == sha, f"Frozen source differs: {name}"
    config = out / "models.json"
    save(config, {"models": {"reranker": local["profile"]}})
    env = {k: v for k, v in os.environ.items() if k in ("PATH", "TMPDIR", "TMP", "TEMP")}
    env.update(GRAPHICLE_NO_UPDATE_NOTIFIER="1", ORT_DYLIB_PATH=local["ort"])

    def call(*args, timeout=300):
        proc = subprocess.run([str(binary), "--data-dir", str(store), "--config", str(config),
                               *map(str, args)], cwd=root, env=env, capture_output=True, timeout=timeout)
        if proc.returncode:
            raise RuntimeError(proc.stderr.decode(errors="replace")[-1800:])
        return json.loads(proc.stdout)

    started = time.monotonic()
    synced = call("sync", root, timeout=1800)
    save(out / "SYNC.json", synced)
    files = indexed_sources(store)
    validate_questions(project, files)
    index = {name: {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw),
                    "matches_git_bytes": name in manifest["files"] and manifest["files"][name]["sha256"] == hashlib.sha256(raw).hexdigest()}
             for name, raw in files.items()}
    save(out / "INDEX.json", {"files": index, "materialized_not_indexed": sorted(set(manifest["files"]) - set(files)),
                              "sync_seconds": time.monotonic() - started})
    print(project["id"], "indexed", len(files), "of", len(manifest["files"]), "materialized files", flush=True)
    cases = []
    for i, query in enumerate(project["queries"]):
        case = {"project": project["id"], "query": query["id"], "intent": query["intent"],
                "origin": query["origin"]["kind"], "split": project["split"], "variants": {}}
        for routed in ([False, True] if i % 2 == 0 else [True, False]):
            variant = "routed" if routed else "baseline"
            args = {**REQUEST, **local["request"], "symbol_links": True,
                    "identifier_route": routed, "question": query["question"]}
            prefix = out / "cases" / query["id"] / variant
            request = prefix.with_suffix(".request.json")
            save(request, args)
            try:
                started = time.monotonic()
                packet = call("call", "context", "--project", root, "--arguments", request)
                elapsed = time.monotonic() - started
                save(prefix.with_suffix(".packet.json"), packet)
                case["variants"][variant] = {**score(packet, query, files), "seconds": elapsed}
            except Exception as error:
                case["variants"][variant] = {"error": str(error), "complete": False}
        cases.append(case)
        save(out / "RESULTS.json", {"cases": cases, "indexed_files": len(files), "materialized_files": len(manifest["files"])})
        print(project["id"], query["id"], {v: r["complete"] for v, r in case["variants"].items()}, flush=True)
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repo", "binary", "dataset", "out", "reranker", "ort"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        parser.error("Output must be empty")
    dataset = json.loads(args.dataset.read_text())
    for project in dataset["projects"]:
        if len(safe_path(project["id"]).parts) != 1:
            parser.error("Project IDs must be single safe path components")
    local = local_reranker(args.reranker, args.ort, "blend")
    binary, repo, out = args.binary.resolve(strict=True), args.repo.resolve(strict=True), args.out.resolve()
    protocol = {"dataset_sha256": digest(args.dataset), "binary_sha256": digest(binary), "local": local,
                "runner_sha256": digest(__file__), "scorer_sha256": digest(Path(__file__).with_name("real_query_eval.py")),
                "scope": "Full historical Git tree, normal source policy; credential paths and symlinks excluded before reads; omissions recorded",
                "queries": "unchanged frozen seed; labels never supplied in requests"}
    save(out / "PROTOCOL.json", protocol)
    cases, errors = [], []
    for project in dataset["projects"]:
        try:
            cases.extend(run_project(project, repo, binary, out / project["id"], local))
        except Exception as error:
            errors.append({"project": project["id"], "error": str(error)})
            print(project["id"], str(error), flush=True)
        save(out / "RESULTS.json", {"protocol": protocol, "cases": cases, "setup_errors": errors})
    return int(bool(errors) or any("error" in v for c in cases for v in c["variants"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
