#!/usr/bin/env python3
"""Explain an instrumented compare_indexed_coverage.py replay.

Use after the comparison has completed its response and integrity checks.
The fixture has ten projects, no reviewed memories, and no recursive memory
seed searches. Reject different workloads instead of double-counting nested
searches. Durations are inclusive, integer milliseconds; derived remainders
include instrumentation, rounding, unmeasured work and local destruction.
RSS checkpoints describe the process, not allocated/live heap ownership.
"""
from __future__ import annotations

import argparse
import json
import re
import statistics
from collections import defaultdict
from pathlib import Path

from large_repositories import save, sha


PREPARE = (
    "indexed_heads", "context_index", "indexed_documents", "indexed_connectors",
    "indexed_document_lookup", "context_index_missing", "indexed_memory_sources",
    "indexed_memories", "indexed_state", "indexed_episode", "snapshot_file_edges",
)
SEARCH = (
    "indexed_code_arm", "context_identifiers", "indexed_property",
    "indexed_rank_prepare", "context_lexical", "context_speaker",
    "indexed_selected_rows", "indexed_assembly_snapshot", "indexed_assembly_inputs",
    "context_assemble",
)
DETAILS = (
    "repository_document_rows", "repository_document_values", "repository_document_stream",
    "memory_visibility_lookup",
    "indexed_property_snapshot", "context_memory_graph", "indexed_load_contents",
)


def profile(root, row, files):
    path = root / (row["label"] + ".log")
    events = [dict(re.findall(r"(\w+)=([^\s]+)", line))
              for line in path.read_text().splitlines() if line.startswith("phase=")]
    by_name = defaultdict(list)
    for event in events:
        by_name[event["phase"]].append(event)
    for name in ("context_indexed", "indexed_prepare", "indexed_search",
                 "indexed_search_ready", "indexed_finalize", "indexed_catalogue",
                 "indexed_property_snapshot", "indexed_assembly_snapshot", "workspace_project"):
        assert len(by_name[name]) == 10, (row["label"], name, len(by_name[name]))
    for name in ("workspace_total", "workspace_ready", "workspace_cross_step", "workspace_fusion"):
        assert len(by_name[name]) == 1, (row["label"], name)
    assert all(int(e["memories"]) == 0 for e in by_name["indexed_catalogue"])
    assert sum(int(e["documents"]) for e in by_name["indexed_catalogue"]) == files
    assert not by_name["graph_saved_read"] and not by_name["snapshot_graph"]

    def ms(name):
        return sum(int(e["ms"]) for e in by_name[name])

    def rest(parent, children):
        value = ms(parent) - sum(children)
        assert value >= 0, (row["label"], parent, value)
        return value

    # These buckets partition context_indexed without adding nested details.
    costs = {name: ms(name) for name in PREPARE}
    costs["prepare_other"] = rest("indexed_prepare", costs.values())
    costs.update({name: ms(name) for name in SEARCH})
    costs["search_other"] = rest("indexed_search_ready", (ms(n) for n in SEARCH))
    costs["search_return_cleanup"] = rest("indexed_search", [ms("indexed_search_ready")])
    costs["indexed_finalize"] = ms("indexed_finalize")
    costs["packet_return_cleanup_and_other"] = rest(
        "context_indexed", [ms("indexed_prepare"), ms("indexed_search"), ms("indexed_finalize")])
    # Expose the clone separately from the property check and its cleanup.
    costs["indexed_property_snapshot"] = ms("indexed_property_snapshot")
    costs["property_other_and_cleanup"] = rest("indexed_property", [costs["indexed_property_snapshot"]])
    del costs["indexed_property"]
    assert sum(costs.values()) == ms("context_indexed")

    retained = []
    project_rss = None
    for event in events:
        if event["phase"] == "workspace_project":
            project_rss = float(event["rss_mb"])
        elif event["phase"] == "workspace_retained":
            retained.append({"projects": int(event["projects"]),
                             "documents": int(event["documents"]), "rss_mib": project_rss})
    assert [e["projects"] for e in retained] == list(range(1, 11))
    assert retained[-1]["documents"] == files
    workspace = {name: ms(name) for name in
                 ("workspace_project", "workspace_cross_step", "workspace_fusion")}
    workspace["workspace_setup_and_other"] = rest("workspace_ready", workspace.values())
    workspace["workspace_return_cleanup"] = rest("workspace_total", [ms("workspace_ready")])
    assert sum(workspace.values()) == ms("workspace_total")
    return {
        "label": row["label"], "workload": row["workload"],
        "seconds": row["seconds"], "peak_rss_bytes": row["peak_rss_bytes"],
        "log_sha256": sha(path), "context_indexed_ms": ms("context_indexed"),
        "workspace_total_ms": ms("workspace_total"), "indexed_costs_ms": costs,
        "workspace_costs_ms": workspace,
        "nested_details_ms": {name: ms(name) for name in DETAILS},
        "phase_calls": {name: len(entries) for name, entries in by_name.items()},
        "workspace_retained": retained,
        "workspace_ready_rss_mib": float(by_name["workspace_ready"][0]["rss_mb"]),
        "workspace_returned_rss_mib": float(by_name["workspace_total"][0]["rss_mb"]),
    }


def summarize(profiles, field, parent):
    total = sum(p[parent] for p in profiles)
    return {name: {
        "mean_ms": statistics.mean(p[field][name] for p in profiles),
        "median_ms": statistics.median(p[field][name] for p in profiles),
        "min_ms": min(p[field][name] for p in profiles),
        "max_ms": max(p[field][name] for p in profiles),
        "share_of_aggregate_parent_percent": 100 * sum(p[field][name] for p in profiles) / total,
    } for name in profiles[0][field]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--comparison", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--arm", choices=("baseline", "candidate"), default="candidate")
    args = parser.parse_args()
    root = args.comparison.resolve(strict=True)
    result = json.loads((root / "results.json").read_text())
    assert result["complete_packets_equal_across_binaries"]
    profiles = [profile(root, row, result["files"]) for row in result["rows"]
                if row["arm"] == args.arm and not row["warmup"]]
    summary = {}
    for mode in ("local", "cross"):
        selected = [p for p in profiles if p["workload"] == mode]
        assert len(selected) == result["samples"] >= 3
        summary[mode] = {
            "indexed_costs": summarize(selected, "indexed_costs_ms", "context_indexed_ms"),
            "workspace_costs": summarize(selected, "workspace_costs_ms", "workspace_total_ms"),
            "nested_details": summarize(selected, "nested_details_ms", "context_indexed_ms"),
            "median_context_indexed_ms": statistics.median(p["context_indexed_ms"] for p in selected),
            "median_workspace_total_ms": statistics.median(p["workspace_total_ms"] for p in selected),
        }
    save(args.output, {
        "comparison": str(root / "results.json"), "comparison_sha256": sha(root / "results.json"),
        "analyser_sha256": sha(Path(__file__)), "arm": args.arm,
        "profiles": profiles, "summary": summary,
        "notes": [
            "Indexed and workspace buckets partition their own parent durations; never add both tables.",
            "Nested details overlap those buckets. ms=0 can mean below millisecond resolution.",
            "Cleanup differences include diagnostic overhead and rounding, not only deallocation.",
            "RSS includes mappings, allocator retention and caches; it is not live heap size.",
            "Compare optimization costs using both arms of the same replay; this is not a latency/concurrency qualification.",
        ],
    })
    for mode, values in summary.items():
        print(mode, "median indexed ms:", values["median_context_indexed_ms"])
        for name, metrics in sorted(values["indexed_costs"].items(),
                                    key=lambda item: -item[1]["median_ms"])[:8]:
            print(f"  {name}: {metrics['median_ms']} ms, "
                  f"{metrics['share_of_aggregate_parent_percent']:.1f}% of indexed time")


if __name__ == "__main__":
    main()
