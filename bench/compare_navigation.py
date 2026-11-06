#!/usr/bin/env python3
"""Compare fresh-process navigation on an existing qualified graph fixture.

Both binaries read the same retained store. No sync or source mutation occurs.
Check complete packets and independently validate the caller/callee spans.
"""
from __future__ import annotations

import argparse
import ast
import json
import os
import platform
import statistics
from pathlib import Path

from large_repositories import run, save, sha


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=5)
    args = parser.parse_args()
    if args.samples < 3:
        parser.error("Use at least three measured samples")
    binaries = {"baseline": args.baseline.resolve(strict=True),
                "candidate": args.candidate.resolve(strict=True)}
    identities = {arm: sha(binary) for arm, binary in binaries.items()}
    helpers = ("compare_navigation.py", "large_repositories.py")
    provenance = {name: sha(Path(__file__).with_name(name)) for name in helpers}
    fixture = args.fixture.resolve(strict=True)
    fixture_files = ("results.json", "final-heads.json", "admit.json")
    fixture_hashes = {name: sha(fixture / name) for name in fixture_files}
    prior = json.loads((fixture / "results.json").read_text())
    assert prior["fresh_cache_equal"] is True
    heads = json.loads((fixture / "final-heads.json").read_text())
    repository = json.loads((fixture / "admit.json").read_text())["repository_id"]
    episode = next(row["graph"]["episode"] for row in reversed(prior["rows"]) if "graph" in row)
    path = next(p for p in sorted(heads) if p.startswith("unit_"))
    index = int(Path(path).stem.removeprefix("unit_"))
    prefix = Path(path).stem + "_"
    raw = (fixture / "sources" / path).read_bytes()
    assert sha(fixture / "sources" / path) == heads[path]
    source = raw.decode()
    declarations = {node.name: ast.get_source_segment(source, node)
                    for node in ast.parse(source).body if isinstance(node, ast.FunctionDef)}
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    save(root / "models.json", {})
    request = root / "neighbors.request.json"
    save(request, {"tool": "get_neighbors", "node_id": prefix + f"run_{index}",
                   "relation_filter": "calls"})
    rows, expected = [], None
    for sample in range(args.samples + 1):
        order = list(binaries)
        if sample % 2:
            order.reverse()
        for arm in order:
            row, packet = run(binaries[arm], root, fixture / "store", f"{sample}-{arm}",
                              ["call", "graph.navigate", "--project", repository,
                               "--arguments", str(request)])
            assert packet["episode"] == packet["current_episode"] == episode
            assert not packet.get("nodes_omitted") and not packet.get("nodes_unmatched")
            assert {node["node_id"] for node in packet["nodes"]} == {prefix + f"run_{index}", prefix + "process"}
            for node in packet["nodes"]:
                assert node["checked"] and node["relpath"] == path and node["sha256"] == heads[path]
                assert 0 <= node["byte_start"] < node["byte_end"] <= len(raw)
                name = node["node_id"].removeprefix(prefix)
                assert raw[node["byte_start"]:node["byte_end"]].decode().strip() == declarations[name].strip()
            if expected is None:
                expected = packet
                save(root / "expected.json", packet)
            assert packet == expected, f"Complete navigation packet differs: {sample}/{arm}"
            row.update(arm=arm, sample=sample, warmup=sample == 0)
            rows.append(row)
            save(root / "observations.json", rows)
            print(json.dumps(row), flush=True)
    assert identities == {arm: sha(binary) for arm, binary in binaries.items()}
    assert provenance == {name: sha(Path(__file__).with_name(name)) for name in helpers}
    assert fixture_hashes == {name: sha(fixture / name) for name in fixture_files}
    assert (fixture / "sources" / path).read_bytes() == raw
    summary = {}
    for arm in binaries:
        measured = [row for row in rows if row["arm"] == arm and not row["warmup"]]
        summary[arm] = {"median_seconds": statistics.median(row["seconds"] for row in measured),
                        "max_peak_rss_bytes": max(row["peak_rss_bytes"] for row in measured)}
    save(root / "results.json", {"binary_sha256": identities, "harness_sha256": provenance,
         "fixture": str(fixture), "fixture_sha256": fixture_hashes, "source_files": len(heads),
         "probe": path, "samples": args.samples, "rows": rows, "summary": summary,
         "platform": platform.platform(), "cpu_count": os.cpu_count(), "engine_max_workers_env": 4,
         "method": "Same retained store, fresh CLI and measurement processes, warm OS cache, alternating arm order, one excluded warmup per arm. Every complete packet is identical; caller/callee spans independently checked against Python AST.",
         "limitations": "One synthetic Python caller/callee query. Measures resident graph startup as well as navigation; does not qualify warm-server latency, arbitrary navigation tools, or other languages. No source or graph build is measured."})


if __name__ == "__main__":
    main()
