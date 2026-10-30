#!/usr/bin/env python3
"""Score labelled configured service dependencies in a materialized review packet.

Labels: {"head": [exact SnapshotPin objects], "callers": [
  {"repository_id": "...", "binding": "...", "protocol": "http",
   "targets": [["target repository id", "target binding id"]]}]}
An empty targets list is a negative/unknown-target case. Labels must be reviewed
independently of the resulting packet. No source retrieval or network calls.
"""
import argparse
import json
from pathlib import Path


def score(packet, labels):
    def pins(values):
        return sorted(json.dumps(v, sort_keys=True, separators=(",", ":")) for v in values)

    if pins(packet["head"]) != pins(labels["head"]):
        raise ValueError("Labelled pins do not match the packet head")
    if packet.get("previous_packet_id") and packet.get("unchanged"):
        raise ValueError("Materialize the delta before scoring")
    expected, observed, protocols = {}, {}, {}
    for case in labels["callers"]:
        caller = (case["repository_id"], case["binding"])
        if caller in expected or case["protocol"] not in ("http", "grpc", "event"):
            raise ValueError("Duplicate caller or invalid protocol in labels")
        expected[caller] = {tuple(target) for target in case["targets"]}
        if any(len(target) != 2 for target in expected[caller]):
            raise ValueError("Targets must be repository/binding pairs")
        protocols[caller] = case["protocol"]
    if not expected:
        raise ValueError("No independently labelled callers")
    for entry in packet["entries"]:
        boundary = entry.get("service_boundary")
        if entry["side"] != "head" or not boundary or boundary["direction"] != "outbound":
            continue
        caller = (entry["repository_id"], boundary["binding"])
        if caller not in expected:
            continue
        if boundary["transport"]["protocol"] != protocols[caller]:
            raise ValueError("Protocol mismatch for labelled caller")
        targets = {(t["owner"]["repository_id"], t["binding"])
                   for t in boundary["resolution"]["targets"]}
        if caller in observed and observed[caller] != targets:
            raise ValueError("Conflicting build views for caller")
        observed[caller] = targets

    def metrics(callers):
        tp = fp = fn = negative = unresolved = omitted = 0
        for caller in callers:
            want, got = expected[caller], observed.get(caller, set())
            tp += len(want & got)
            fp += len(got - want)
            fn += len(want - got)
            negative += not want and caller in observed and not got
            unresolved += caller in observed and not got
            omitted += caller not in observed
        return {"true_positives": tp, "false_positives": fp, "false_negatives": fn,
                "precision": tp / (tp + fp) if tp + fp else None,
                "recall": tp / (tp + fn) if tp + fn else None,
                "correct_negative_callers": negative,
                "unresolved_callers": unresolved, "omitted_callers": omitted}

    return {"scope": "configured boundary resolution on independently labelled callers",
            "inventory_recall_measured": False, "runtime_behavior_measured": False,
            "labelled_callers": len(expected), "overall": metrics(expected),
            "by_protocol": {p: metrics([c for c in expected if protocols[c] == p])
                            for p in sorted(set(protocols.values()))},
            "packet_coverage_complete": packet["coverage"]["complete"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packet", type=Path, required=True)
    parser.add_argument("--labels", type=Path, required=True)
    args = parser.parse_args()
    result = score(json.loads(args.packet.read_text()), json.loads(args.labels.read_text()))
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
