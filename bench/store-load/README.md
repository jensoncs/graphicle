# SQLite store under load

Backported from upstream Graphicle 0.1.14 (`ddf11b1`), adapted for this
branch's SQLite store. Python 3.10+ and its standard library are sufficient.
The runner supports Linux and macOS.

`run.py` starts a Graphicle server, sends concurrent MCP reads against a
seed checkout, and indexes a second checkout in a separate process against
the same store. It measures the periods before, during and after indexing.
The binary decides the implementation being tested, so use separate runs
to compare a control binary and the SQLite backport.

## Run

Build the binary from `crates/graphicle` so Cargo loads the bundled-parser
configuration. Pass its absolute path. The seed must contain tracked code
files; both input checkouts should be disposable benchmark copies because
Graphicle may write its generated project files there.

For a normal-checkout comparison, use committed fixtures and verify
`git status --porcelain` is empty before each run. If exporting files into
a new Git repository, commit that export on a scratch branch: merely staging
it leaves every file in Git's change list and makes the per-request folder
freshness checks much more expensive. Record the source revisions and Git
state alongside results; use matching dirty trees only for an intentional
dirty-checkout stress test. Keep freshness enabled for the main comparison.

For performance comparisons, prepare both inputs the same way and alternate
binary order across repeated runs (for example candidate, control, control,
candidate). Record cache preparation and keep other builds/tests out of the
measurement window. Report seed sync separately: it has no HTTP readers or
request accounting, and Go extraction compiles its helper with a private,
fresh compiler cache in each process. Keep phase diagnostics equally enabled
or disabled; use a separate on/off comparison to measure their overhead.
This runner uses closed-loop readers, so faster responses also produce more
requests and compete more with indexing. Report throughput with latency and
indexing time; one slower indexing run alone does not establish a regression.

```sh
python3 -I bench/store-load/run.py \
  --bin /absolute/path/to/graphicle \
  --seed /tmp/seed-checkout --index /tmp/large-checkout \
  --data-dir /tmp/sqlite-load-data-01 --out /tmp/sqlite-load-results-01 \
  --label sqlite-backport --clients 10 \
  --warmup 60 --after 120 --sample 1

python3 -I bench/store-load/table.py /tmp/sqlite-load-results-01
```

Both `--data-dir` and `--out` must be new directories, outside the input
checkouts and outside each other. Existing folders, including empty ones,
are refused. The runner never deletes a folder to make room for a run.
Each repeated run needs fresh paths. Treat the generated data folder like
any Graphicle store: it contains the indexed sources and a local owner key.

The default is 33 concurrent readers. Start with 2–10 on a laptop. A quick
smoke run can use `--clients 2 --warmup 2 --after 2 --sample 0.2` and small
checkouts. `--index-limit` bounds each sync (eight hours by default), and
`--request-timeout` bounds an HTTP request (30 seconds by default). Child
process groups are stopped on errors, timeouts and Ctrl-C. Failed runs keep
their logs and `run.json` whenever the output directory has been created.

## Results

- `run.json`: version, settings, indexing outcome, latency percentiles,
  read throughput and failures by phase and tool, memory, WAL size and
  optional fsync counts, plus the server's write-lock diagnostics before and
  after the workload. Indexer lock waits are logged when phase timings are on.
- `samples.csv`: periodic server/indexer RSS, database/WAL/shared-memory
  sizes and completed/failed read counts; present once sampling starts.
- `calls.csv`: each tool call's start time, duration, status and phase.
  A call belongs to the phase in which it started.
- `seed-sync.log`, `server.log`, `indexer.log`: process diagnostics.
  `seed.json`, `indexer.json` and `tools.json` preserve the corresponding
  outputs. These files do not contain the local owner key.

Exit status is 0 for a successful run, 1 for a run/indexing/measurement
failure, or 2 when indexing finished but some reads failed. Invalid CLI
arguments also exit 2 before a run starts. Missing measurements are `null`,
not zero; reports contain strict JSON, including phases with no calls.

On Linux, RSS uses `/proc` and includes observed `VmHWM`; `strace`, when
installed, measures server/indexer fsync calls. `--no-strace` disables it.
On macOS, RSS is sampled with `ps`, so brief peaks may be missed; fsync
counts are unavailable. Sandboxes that block `ps` also leave memory values
unknown. WAL peaks are sampled on both platforms. A short
index may finish between samples, leaving its memory measurement unknown.

The tool mix is weighted toward graph queries and includes recall and
source reads. Missing tools are recorded and omitted. Arguments come from
tracked code paths and simple symbol-name extraction, so inspect per-tool
errors: an invalid/ambiguous symbol is not a storage failure. A fixed RNG
seed selects the workload but thread scheduling still changes the run.

This SQLite backport omits PostgreSQL metrics, cgroup controls and historical
upstream benchmark results. No PostgreSQL connection is used, even if the
parent environment sets `GRAPHICLE_DATABASE_URL`.

## Lock-wait diagnostics

The running server's `runtime.status` response contains `write_lock`:
`slow_waits` counts write transactions that waited at least two seconds,
including unsuccessful attempts; `longest_wait_seconds` and `longest_wait_at`
identify the worst wait. Existing hold-time fields remain separate. These
are process-local counters; query the running server rather than launching
a fresh CLI process to inspect its history. `GRAPHICLE_PHASE_TIMINGS=1`
also logs slow waits at their source location. Direct writes outside
`Store::transaction`, including request counters and migrations, are excluded.
Counter writes instead log `request_tally_lock`, `request_tally_commit` and
`request_tally_checkpoint` phases with `GRAPHICLE_PHASE_TIMINGS=1`; these
separate accounting-lock waits from checkpoint work after that lock is released.

## Harness checks

```sh
python3 -I -m unittest discover -s bench/store-load -p 'test_*.py' -v
```
