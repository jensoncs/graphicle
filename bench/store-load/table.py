#!/usr/bin/env python3
"""Print a Markdown comparison: table.py RUN_DIR [RUN_DIR ...]."""
import json
import sys
from pathlib import Path


def number(value, digits=1):
    return f"{value:.{digits}f}" if isinstance(value, (float, int)) else "n/a"


def phase(reads, name):
    result = reads.get(name, {})
    return (f"{number(result.get('p50_ms'), 0)} / {number(result.get('p99_ms'), 0)} ms, "
            f"{number(result.get('reads_per_s'))}/s, "
            f"{result.get('failed', 0)}/{result.get('calls', 0)} failed")


def main():
    print("| Run | Outcome | Seed (s) | Index (s) | Reads before | Reads during indexing | Reads after | Peak RSS (MiB) | WAL (MiB) | fsyncs |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for directory in sys.argv[1:]:
        record = json.loads((Path(directory) / "run.json").read_text())
        index, rss = record.get("index", {}), record.get("peak_rss_mib", {})
        outcome = "ok"
        if not record.get("completed") or index.get("exit") != 0 or record.get("sampling_errors") or record.get("cleanup_errors"):
            outcome = "incomplete/failed"
        elif record.get("failures"):
            outcome = "read failures"
        fsyncs = ", ".join(f"{name} {sum(counts.values()) if counts is not None else 'n/a'}"
                           for name, counts in record.get("fsyncs", {}).items())
        label = str(record.get("label") or directory).replace("|", "\\|").replace("\n", " ")
        reads = record.get("reads", {})
        print(f"| {label} | {outcome} | {number(record.get('seed', {}).get('seconds'))} | {number(index.get('seconds'))}"
              f" | {phase(reads, 'warmup')} | {phase(reads, 'indexing')} | {phase(reads, 'after')}"
              f" | server {number(rss.get('server'))}, indexer {number(rss.get('indexer'))}"
              f" | peak {number(record.get('wal_peak_mib'))}, end {number(record.get('wal_end_mib'))} | {fsyncs} |")


if __name__ == "__main__":
    main()
