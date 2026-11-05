# Review pilot protocol measurements

Measured on 2026-11-02 with deterministic local fixtures, Rust 1.94.1, and no
model or external service calls. Implementation and integration requirements:
[pilot protocol](../pilot-protocol.md).

| Fixture | Previous behavior / comparison | New result |
| --- | --- | --- |
| Status: 1,400 JS files, long paths, shared imports | Full graph response about 9,124,246 bytes | Compact status 356 bytes; over 99.99% smaller |
| Missing uploads: 3,000 unique files with long escaped paths | Entire missing-path array 3,189,001 bytes | 25 pages; largest response 132,604 bytes, about 95.84% smaller than the unpaged array |
| Admission: 1,000 files per side, eight changed files, base already synced in context project | Re-uploading both sides: 2,425,852 raw bytes / 3,239,296 base64 characters | Reuse 1,000 objects; upload eight changed files: 9,760 raw bytes / 13,024 base64 characters, 99.60% less content |
| Context: 12 matching documents, 512-token / 2,000-character budget | Budget losses had no per-source fusion-score diagnostics | Ten packing candidates, three kept, seven budget-dropped; minimum kept fusion score 0.010638297872340425 |

Status comparison includes the full graph, not a claim that every admitted file
always appears in a status manifest. The test asserts the full response exceeds
1 MiB and compact status stays below 2 KiB. Response sizes can vary slightly with
runtime metadata. The missing-page test checks every path appears exactly once,
including when earlier pages are uploaded before requesting the next page.

Admission content figures exclude JSON envelopes and manifests. Base64 figures
are calculated from exact fixture bytes using four characters per three-byte
group. Actual server-side reuse and changed-file uploads are exercised through
prepare/upload/commit. The test also verifies compact commit counts. Binary
preservation, donor snapshot membership, cross-workspace denial, revoked donor
access, and rollback after a later donor failure have separate assertions.

Ranking instrumentation does not change the ranker. It makes losses measurable;
it does not prove that the seven omitted windows were useless. No Sentinel or
Nexus production pilot was rerun, and no wall-clock speedup is claimed.

## Reproduction and validation

Use the repository's prepared parser bundle. The focused local build included
JavaScript, TypeScript, TSX, Python, Rust, and Go. From `crates/graphicle`:

```sh
cargo test --locked --test it pilot_protocol:: -- --nocapture
cargo test --locked --test it pinned_review::
cargo test --locked --test it core_limits::
cargo test --locked --test it async_requests::
cargo test --locked --test it product_workspace::
cargo test --locked --lib
```

The pilot suite prints `PILOT_*_MEASUREMENT` JSON records. Every packet produced
through the pinned-review fixture is validated against the versioned risk schema,
including basic analysis, Go symbols, history, service boundaries, and deltas.

Additional validation: memory `native_contracts`, `rrf_review`, and
`structured_recall` suites (58 tests), TypeScript SDK tests and types (18 runtime
tests), and Python SDK compact-status, authorization, deadlines, retry, and origin
checks (39 tests). Runtime unit tests: 437 passed; pinned reviews: 35 passed;
sync limits, including 12,000-file local sync: nine passed. The seven pilot
protocol tests and 17 HTTP/async regression tests also passed. In total, these
focused suites passed 620 tests, plus the TypeScript type check.

Repository-wide checks are not a clean release gate: an initial all-integration
run used an insufficient three-language parser bundle and failed broadly. The
relevant suites were rerun with the six-language bundle above. Strict Clippy
reports pre-existing findings in `identifier_arm.rs`, `evidence.rs`, `query.rs`
tests, and `symbol_links.rs`; formatting checks also report untouched code.
`cargo deny check` could not run because `cargo-deny` is not installed. No
dependency or lockfile was changed.

Reviewed memories remain excluded from the source-only pilot baseline; this is
an intentional protocol decision, not an unimplemented admission step.

## Source admission follow-up

The following deterministic fixtures exercise the upstream admission fixes;
they are not a rerun of the Odin or Nexus production probes.

| Fixture | Before / control | Updated behavior |
| --- | --- | --- |
| Changed Go function; three withheld credentials per side | `scope_complete:false` without a list: `impact:incomplete` | Exhaustive list: three inputs ruled outside the Go build per side; `impact:potential_impact`, including on a compiler-cache hit |
| Credential matching `go:embed assets/*` | Incomplete admission | Still incomplete, one unresolved withheld input |
| Withheld Go source, `vendor/modules.txt`, `go.mod`, or unknown JSON | Incomplete admission | Still incomplete; missing source is not declared irrelevant using the partial graph |
| Missing external Go package | External error also attributed as local `incomplete_discovery` | Separate `unavailable_external_package`; local discovery warning removed, actual local type-check failure retained; impact still incomplete |
| Exact tree with different commits/policies | One tree may have several admission assertions | Lists stay independent by binding; legacy and older-writer assertions remain conservative |

The existing 1,000-file reuse fixture was rerun: 1,000 objects reused and eight
changed files uploaded, 9,760 raw bytes instead of 2,425,852 (99.60% less content).
No new sync acceleration is claimed. Donor reuse still copies bytes into the
target and does not copy parse/chunk records; direct context-project pins avoid
fresh scratch admission. Missing pages still perform per-object checks across
the manifest. Production phase timings are needed to attribute the reported
252-second Nexus sync to those costs.

Focused tests: `go test go_types.go go_review.go go_semantic.go go_types_test.go`,
runtime `cargo test --lib --test it review::`, and
`cargo test --test it pilot_protocol:: -- --nocapture`. New coverage includes
path validation, multipart overlap, embedded credentials, commit/policy isolation,
cached-fact reassessment, schema migration, and invalidation by older writers.
The runtime now declares the already-locked `glob` dependency directly to match
Go embed patterns conservatively.

## Diagnostic follow-up (2026-11-03)

One synthetic Go package with 1,000 calls to an undefined function produces
1,000 `incomplete_package` reference limitations. The new per-side summary
reports **one** incomplete package instance, **one** failed type-check instance,
and **1,000** affected references, with a bounded `undefined: MissingImplementation`
compiler example. Impact remains incomplete. An exact compiler-cache replay
retains the same diagnostic summary.

Separate fixtures cover unavailable external imports, healthy snapshots with no
diagnostics, message/count bounds, temporary-path removal, preservation of source
errors when dependency discovery is noisy, and Python file/cause accounting even
when no diagnostic examples fit. The fields and sampling contract are documented
in [risk measurements](../pilot-protocol.md#risk-measurements).

This establishes that reported limit totals can be amplified by reference count;
it does not establish how many packages failed in the production probes or whether
generated source was missing. Those conclusions require the exact admitted
snapshots or compiler diagnostics from the next probe. The change adds visibility,
not a claim of improved risk completeness or Nexus latency.

## Confirmed-fix regression fixtures (2026-11-03)

- A non-module repository with a Go example skips the compiler and reports
  `go_source_without_module`; the spurious external-package/type-check pair is
  absent. A repository with two nested modules and no top-level `go.mod` analyses
  both modules, with no root-discovery failure.
- An app → local database package → external dependency chain has two failed
  local package instances before dependency admission. Admitting a matching
  `vendor/modules.txt` and dependency source reduces failed local packages,
  type-check failures and incomplete-package references to zero. A changed
  caller then reports `potential_impact`. Selecting package subdirectories also
  uses the owning module's vendor tree. This tests Graphicle's consumption of
  prepared sources; cache verification and source preparation belong to the
  collector, including Sentinel #603.
- Twenty Python modules containing direct lexical `os.path.join` calls produce
  zero environment-incomplete file warnings. A changed deployment environment
  value still selects its actual reader and caller and excludes those twenty
  unrelated modules. Environment expansion, binding escape, dynamic access,
  conditional imports and monkey-patching retain uncertainty. Basic Python
  analysis still reports incomplete impact.
- The 1,000-reference Go fixture accounts for all 2,000 failed-package references
  across base and head in `coverage.issue_histogram`, despite the 20-example
  issue list. Group counts plus `other_count` equal the full issue count; more
  than 128 distinct groups retain explicit overflow accounting.

These are local synthetic regressions, not results for the eight production PRs.
The parser revision advances to 9, so existing source parses refresh once on
their next sync. Native facts also get a new cache identity for module discovery.

Validation: 54 runtime review unit tests, 76 review integration tests (including
focused reruns), and four Go helper tests passed. The inline service-flow test
previously assumed the complete composed path always fit its fixed flow budget;
it now requires explicit omission accounting when it does not fit. The compact
provenance test still requires the full composed path. Production flow packing
was not changed.

## Supplemental diff context (2026-11-04)

The Sentinel handoff reports 524 of 623 delivered entries overlapping its diff
on build 54920c9b. The opt-in `provided_diff` implementation is locally tested
against that baseline; these measurements are synthetic, not a new Sentinel probe.

The fixture changes a 300-statement Go function, with a caller and dependency
in separate unchanged files. Declaring the exact base/head changed-file ranges
removes both duplicate function entries and their raw hunk. Full graph impact
stays `potential_impact`; packet/reuse completeness stays conservative.

| Token budget | Additional entries, default | With provided diff |
| --- | ---: | ---: |
| 3,500 | 2 | 1 |
| 5,000 | 1 | 3 |
| 6,500 | 2 | 4 |

Counts include both pinned sides, not distinct files. At 3,500 tokens the added
accounting crosses a packing threshold; this is not a universal count increase.
The improvement at the larger budgets needs confirmation using Sentinel's real
prompt-visible ranges and unchanged admission/budget settings.

Tests also cover partial ranges, gaps, merging adjacent ranges, invalid hashes
and bounds, base/head separation, identical files in different repositories,
delta invalidation/restoration, and unchanged behavior when the option is absent.
Validation: 65 review unit tests, 100 review integration tests, the Python SDK
transport check and TypeScript type checks passed, with focused reruns after edits.
Nexus latency, interface-dispatch completeness and Dora's file-level-only output
remain open; this option addresses duplicate context, not those analysis gaps.

## Partial-hunk ranking correction (2026-11-04)

Sentinel's probe on 861fa831 did not establish useful added context: outside-hunk
tokens were 0 → 22, 62 → 62, 56 → 0 and 5 → 0 on Argus, Odin,
Metadata Service and Nexus. Whole-function overlap classification counts some
unseen lines as overlapping, so these are not duplicate-token measurements.
Range exclusions worked, but changed declarations still had first packing
priority. The original whole-declaration fixture did not cover that failure.

Nonempty `provided_diff` now enables `additional_context_then_overlap_v1`:
graph-linked nonoverlapping entries first, partially overlapping entries and
remaining hunks next, uncertain/history-only entries last. Ordering within each
tier retains the existing semantic priorities and distances. Raw hunks have no
reservation in this mode; optional history and flow annotations use residual
space after source selection. Partial declarations stay intact and eligible.
Analysis impact, admission checks and conservative reuse proofs are unchanged.

The existing 300-statement function fixture now also supplies only the edited
line on each side, keeping both changed functions eligible:

| Token budget | Outside entries, no ranges | Outside entries, partial ranges | Overlapping entries selected with ranges |
| --- | ---: | ---: | ---: |
| 3,500 | 2 | 2 | 0 |
| 5,000 | 1 | 3 | 0 |
| 6,500 | 2 | 4 | 0 |

These are local synthetic measurements on the new implementation, not a replay
of production PRs or a comparison against the old build with the same ranges.
Counts include both pinned sides, not distinct files. The current-head caller
and dependency are included at every tested budget. The earlier whole-file
fixture still returns only one additional entry at 3,500 tokens versus two
without ranges; envelope/compaction thresholds remain a small-budget limitation.

`coverage.provided_diff` now distinguishes eligible partial overlaps and selected
outside/overlapping entries, including unchanged delta entries. Build info
advertises the ranking policy. Existing phase diagnostics can separately time
range validation, annotation and packing; no latency improvement is claimed.
Nexus's roughly 400-second probe remains above the 180-second live bound.

Validation: 67 review unit tests and 100 review integration tests passed,
including byte-boundary/hash/side isolation, partial-hunk budget checks,
uncertain-scope ordering, packet bounds, schema validation and stable delta
selection. Production usefulness and latency require the same pinned probe
with unchanged admission, budgets and prompt-visible ranges.

## Review cache refresh measurements (2026-11-04)

Compared fcddad5 with two cache optimizations: defer loading dependency exports
until a full-analysis miss, and refresh verified cache rows without encoding or
resubmitting unchanged payloads. Base-created exports still serve head analysis.
Native replacements invalidate the saved checksum; eviction or a changed stored
checksum triggers a normal save. Snapshot bindings are retained on refresh.

Local macOS Rust 1.94.1 **unoptimized test binaries**, sequential runs in both
orders. Each arm has 14 measured iterations across two runs, after one warm-up
per run. Source and payload sizes are identical; generated repository IDs vary
between fixtures. These measurements are not the eight-PR production probe.

| Operation | Baseline median | Changed median | Reduction |
| --- | ---: | ---: | ---: |
| Warm review, 32 MiB historical dependency pool | 967.4 ms | 139.6 ms | 85.6% |
| Warm review, empty dependency pool (control) | 144.7 ms | 138.9 ms | 4.0% |
| Refresh 32 MiB dependency payloads only | 845.1 ms | 47.2 ms | 94.4% |
| Refresh 16 MiB analysis payload only | 837.2 ms | 15.1 ms | 98.2% |

The review fixture runs real pinned Go analysis once, then measures
`review::run` on full-analysis hits. Historical export payloads are synthetic,
unused blobs to exercise the bounded loader. The persistence measurements use
opaque synthetic payloads and exclude loading/checksum validation, which still
happens when reading the cache. They are separate microbenchmarks, not components
to add together. The empty-pool control remains roughly unchanged.

No vendoring, admission/sync or CLI startup is included. Release-build and
production savings remain unmeasured; these results do not establish that Nexus
fits the 180-second pilot budget. Cold type-checking work is unchanged.

[Raw samples and measurement metadata](review-cache-refresh-benchmark.json).
Reproduce with the ignored `warm_review_cache_benchmark` integration test and
`cache_persistence_benchmark` unit test, using `--ignored --nocapture
--test-threads=1` and the same build profile for both revisions. Run timing tests
separately from compilation and other test workloads.

Validation: 74 unit tests and 125 integration tests passed across the broad
review run and focused rerun. Coverage includes unchanged-payload write guards,
full hits with an unusable optional export pool, cached/fresh packet equality,
native corruption repair, eviction/replacement fallback, access revocation,
snapshot removal and compiler-input invalidation. The broad run exposed an
existing 3,500-token packing assertion that also failed on the unchanged
baseline: randomized IDs can make the second affordable outside entry the base
caller. That test now requires the current dependency at 5,000/6,500 tokens and
retains the outside-context/current-caller requirements at 3,500. Packing code
was not changed. `git diff --check` passed.

## Batched missing-list pagination (2026-11-04)

The prior implementation decoded the manifest into Rust values and executed one
object-existence statement per unique digest on every page. The new implementation
uses one SQLite statement to group representative paths, probe the existing size
index, count current missing objects, and return at most a page plus one lookahead
path. Rust retains the same JSON byte bound and cursor encoding. The query is
read-only, uses existing schema, and requires no client or pending-sync migration.

Compared baseline 1c957bd with this change using local macOS Rust 1.94.1
unoptimized test binaries and bundled SQLite. Both arms use identical deterministic
manifests, an in-memory object-metadata table, and 256 paths per page. All objects
are unique and missing; timed runs enumerate pages without uploads. Runs execute
sequentially in before/after/after/before order, giving two samples per size per arm.

| Files | Pages | Baseline median | Changed median | Time reduction | SQL executions, before → after |
| --- | ---: | ---: | ---: | ---: | ---: |
| 5,000 | 20 | 0.372 s | 0.217 s | 41.6% | 100,000 → 20 |
| 20,000 | 79 | 5.981 s | 3.903 s | 34.7% | 1,580,000 → 79 |
| 50,000 | 196 | 38.323 s | 22.885 s | 40.3% | 9,800,000 → 196 |

Execution counts come from SQLite's `SQLITE_STMTSTATUS_RUN`. They count the
missing-object statement only, not authorization or sync-record reads. Batching
does **not** eliminate SQLite's underlying index probes: each page still scans
and groups the manifest to produce a live `missing_total`. No persistent manifest
index or cached missing inventory was added. The time improvement comes from
removing repeated Rust-side manifest decoding and per-object query calls.

[Raw samples and method](sync-pagination-benchmark.json). Reproduce with the
ignored unit test `missing_pagination_benchmark --ignored --nocapture
--test-threads=1`, using the same build profile and identical fixture on both
revisions. Timing excludes authorization, admission, uploads, source parsing,
projection, vendoring and native review analysis. Production and release-build
savings remain unmeasured; this does not establish Nexus's end-to-end latency.

Validation: four pagination unit tests, seven pilot-protocol integration tests,
and 31 sync integration tests passed. A test-only copy of the old implementation
checks exact response equality across uploads between pages, object removal,
duplicate hashes, wrong sizes, repository isolation, cursor validation, Unicode
and escaped names, byte bounds, and empty results. A native query-plan test checks
that availability uses the covering size index without reading source payloads.
Invalid manifest roots still fail closed. `git diff --check` passed.

## Compiler snapshot materialization (2026-11-04)

Compared baseline a8e1a80 with exclusive creation of scratch files and reuse of
parent-directory creation. Previously each source used a temporary file followed
by no-clobber publication and repeated parent-directory creation. The new tree
remains private and disposable; the compiler starts only after all writes succeed.
Unix roots are explicitly mode 0700, and files remain mode 0600, subject to umask.
Input validation, cache identities and analysis behavior are unchanged.

Local macOS Rust 1.94.1 **unoptimized test binaries**, sequential
before/after/after/before runs, two samples per size per arm. Each synthetic file
is 1,024 bytes, with 100 files per leaf directory. Input preparation is outside
timing. Every file's bytes are checked outside timing, and tree removal is checked
after cleanup. These are fresh trees, not a claim of cold OS disk caches.

| Files | Creation median, before → after | Creation reduction | Cleanup median, before → after | Creation + cleanup median, before → after | Total reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| 5,000 | 2.478 → 0.930 s | 62.4% | 0.858 → 0.895 s | 3.336 → 1.826 s | 45.3% |
| 20,000 | 9.884 → 3.916 s | 60.4% | 3.483 → 3.508 s | 13.368 → 7.424 s | 44.5% |
| 50,000 | 23.709 → 9.691 s | 59.1% | 8.605 → 8.515 s | 32.314 → 18.206 s | 43.7% |

Totals use the median of each run's creation plus cleanup. Cleanup is essentially
unchanged and is outside the existing `review_materialize` phase. The benchmark
excludes admission, sync, Go type-checking and CLI startup. It does not measure a
full review, a release build, or production VM contention. Production savings and
whether Nexus fits the pilot deadline remain unmeasured. Do not add these results
to the cache or pagination microbenchmarks, which use different fixtures.

[Raw samples and method](snapshot-materialization-benchmark.json). Reproduce with
the ignored `snapshot_materialization_benchmark --ignored --nocapture
--test-threads=1` test and the same fixture/build profile on both revisions. Run
timing tests separately from compilation and other test workloads.

Validation: six unit tests and 27 integration tests passed, covering exact
text/binary bytes, Unix permissions, cleanup, file/directory collisions, leaf
symlink rejection, compiler caches, and Go/source admission. A native Go fixture
with a vendored dependency and binary `embed.FS` asset produces exactly the same
compiler facts with the previous and new materialization algorithms. All four
benchmark runs passed their full byte and cleanup checks. `git diff --check`
passed.

## Shared review source loading and batched retention (2026-11-04)

Base and head now load in one authorized SQLite read snapshot, sharing verified
immutable content by repository and digest. Sharing includes identical objects
under different paths, but never crosses repositories or requests. Each manifest
still validates its own paths, sizes, pin bindings and source budgets. The final
authorized transaction checks distinct repository/digest/size tuples in batches
of 256, using bound SQL parameters and the existing covering size index. Scope,
snapshot and authorization checks remain in that transaction. There is no schema,
protocol, admission-limit or analysis-scope change.

Compared the source-object calls and retention query from 25b8e38 with the new
helpers in one local macOS Rust 1.94.1 **unoptimized test binary**. The fixture has
unique 1,024-byte text objects in an in-memory SQLite table, with 1%, 10% or 100%
changed between sides. Both arms retain their results in Arc holders for the same
measurement layout; the baseline loads every occurrence independently. Each case
warms both arms once, then runs before/after/after/before, giving two samples per
arm. Every resulting source byte is checked outside timing.

| Objects per side | Changed | Source load median, before → after | Retention median, before → after | Combined median, before → after | Combined time change |
| --- | ---: | ---: | ---: | ---: | ---: |
| 20,000 | 1% | 1.204 → 0.643 s | 57.7 → 45.2 ms | 1.262 → 0.688 s | 45.5% lower |
| 20,000 | 10% | 1.218 → 0.717 s | 57.5 → 46.7 ms | 1.276 → 0.764 s | 40.1% lower |
| 20,000 | 100% | 1.214 → 1.290 s | 63.7 → 66.0 ms | 1.278 → 1.356 s | 6.1% higher |
| 50,000 | 1% | 3.080 → 1.660 s | 161.9 → 128.3 ms | 3.242 → 1.788 s | 44.8% lower |
| 50,000 | 10% | 3.056 → 1.828 s | 163.1 → 132.2 ms | 3.219 → 1.960 s | 39.1% lower |
| 50,000 | 100% | 3.091 → 3.262 s | 180.3 → 187.5 ms | 3.271 → 3.450 s | 5.5% higher |

Sharing reduces retained source payload bytes by 49.5% at 1% changed and 45% at
10% changed; it saves none when everything changes. These are exact distinct
content-allocation byte counts, **not peak RSS**: database memory, fixture inputs,
allocator overhead, source metadata and pool bookkeeping are excluded. The
all-changed control exposes the bookkeeping cost and must accompany the gains.

Source timing includes lookup, SHA verification, allocation and disposal of the
request-local pool. Retention timing includes deduplication, SQL construction,
binding and indexed checks. Batching reduces statement executions, not the
necessary object checks. For the 20,000-object, 1%-changed fixture, the calculated
execution count falls from 40,000 to 79 for 20,200 distinct objects. A regression
test also measures actual statement runs across batch boundaries.

This is an isolated helper benchmark, not a full review. The 50,000-object case
deliberately exceeds the unchanged 20,000-file per-side admission limit to stress
the helpers. Manifest decoding, authorization and pin validation, native analysis,
packing, CLI startup and production VM contention are excluded. Release-build,
end-to-end production latency and peak RSS improvements remain unmeasured; these
results do not establish that Nexus fits its pilot deadline.

[Raw samples and method](review-source-io-benchmark.json). Reproduce with
`source_io_benchmark --ignored --nocapture --test-threads=1`, with no compilation
or other test workloads running alongside it.

Validation: seven new source/retention unit tests, 70 other review unit tests and
85 pinned-review integration tests passed (162 total). The new tests compare
complete loaded inputs with the previous loader; verify text/binary identity and
sharing across renamed paths; preserve repository, credential, binding, size and
per-side budget boundaries; check corruption on new reads; and exercise concurrent
deletion between sides with final rejection. Retention tests compare the previous
query's outcomes across batch boundaries, missing objects, wrong repositories and
sizes, and verify covering-index use without reading source payloads. The broader
suite covers compiler facts/caches, source admission, context selection and
packet behavior. The benchmark passed every byte comparison. `git diff --check`
passed.

## Local release replay across all four optimizations (2026-11-04)

Compared fcddad5 with 4e0b996: cache refresh, missing-list batching, scratch-tree
materialization and shared source loading/batched retention. Both were built with
Rust 1.94.1's release profile on the same local macOS ARM machine. No builds or
other test workloads ran alongside the measurements; unrelated machine activity
was not controlled. These are two samples per arm, not a production latency SLA
or a statistical confidence estimate.

The eight production probe archives were unavailable locally. Instead, this uses
two older real archives: Nexus #963 (Go scope `xlr8`) and platform-rbac #638 (root
Go module). The archived commits, source hashes and build profiles are preserved.
The controlled requests use 8,000 tokens, compact provenance, no business history
and no `provided_diff`. Nexus has 4,933/4,934 paths and about 51 MiB per side;
platform-rbac has 4,586/4,587 paths and about 59 MiB per side. More than 99% of head
paths are unchanged. All source objects passed size and hash verification.

Each arm starts with an empty project, syncs base and head through local HTTP,
then runs cold and warm CLI reviews. Before/after/after/before order gives eight
fresh syncs and 16 review calls. Cold means empty Graphicle native-analysis and
package caches, not cold OS/filesystem caches. Every warm call hit both full
native-analysis caches. Server initialization/lifecycle, seed preparation,
repository collection, vendoring and model calls are excluded. `Sync + cold`
adds the two measured stages; it is not the full Sentinel review duration.

| Case | Fresh sync, before → after | Cold review, before → after | Warm review, before → after | Sync + cold, before → after |
| --- | ---: | ---: | ---: | ---: |
| Nexus #963 | 15.15 → 15.28 s | 32.03 → 30.42 s (5.0% lower) | 2.40 → 2.02 s (15.7% lower) | 47.18 → 45.70 s (3.1% lower) |
| platform-rbac #638 | 16.27 → 16.45 s | 31.33 → 29.99 s (4.3% lower) | 2.13 → 1.96 s (8.2% lower) | 47.60 → 46.44 s (2.4% lower) |

Values are medians. Sync did not improve in these release-build cases. Discovery
(prepare plus missing pages) rose from 0.324 to 0.363 s on Nexus and 0.279 to
0.346 s on platform-rbac, despite the earlier unoptimized pagination benchmark's
gain. This small local cohort does not establish the release-build behavior at
20,000–50,000 files. Sync commit/indexing still takes about 12–14 seconds and
dominates first admission; missing-list discovery is a small part of that cost.

| Cold-review phase | Nexus, before → after | platform-rbac, before → after |
| --- | ---: | ---: |
| Source loading, both sides | 253 → 131 ms | 280 → 146 ms |
| Scratch-tree creation, both sides | 4.640 → 2.211 s | 4.240 → 2.097 s |
| Native Go facts, both sides | 22.740 → 23.629 s | 22.982 → 23.782 s |
| Compare | 320 → 326 ms | 345 → 347 ms |
| Pack | 158 → 168 ms | 157 → 162 ms |

The source-loading and scratch-creation improvements hold in the complete review,
but native Go analysis takes 78–79% of candidate cold-review time. It was slightly
slower in these samples and was not optimized by these changes. The aggregate
gain is consequently much smaller than the isolated microbenchmark gains.
Cache reuse and native Go profiling are the next large opportunities; these
results do not justify claiming that Nexus #1009 now fits 180 seconds.

| Case | Cold command peak RSS, before → after | Warm command peak RSS, before → after |
| --- | ---: | ---: |
| Nexus #963 | 839.2 → 838.8 MiB | 603.2 → 512.0 MiB (15.1% lower) |
| platform-rbac #638 | 642.0 → 632.6 MiB | 613.9 → 528.8 MiB (13.9% lower) |

Memory uses `os.wait4` child `ru_maxrss` (macOS bytes), not a simultaneous
process-tree total. Per-phase Graphicle high-water RSS is also retained in the
raw samples. An initial baseline-only attempt was excluded before candidate
measurement because `/usr/bin/time -l` failed on a denied `kern.clockrate` query
after Graphicle completed successfully; the entire ABBA series was then run
using `wait4`.

All 16 packets were identical within each case after excluding only
`analysis_cache` and `tokens`, including packet IDs, entries, diffs, ordering,
coverage and impact. All respected the 8,000-token bound (maximum observed 7,651).
Nexus retained four entries and platform-rbac five. Impact stayed `incomplete`
with the same issue counts (2,822 and 12,973); performance changes do not close
the archived inputs' analysis gaps. No model was called and nothing was deployed.

[Raw samples, phase timings, binary hashes and method](local-pinned-review-performance-20261104.json).
The reusable [replay tool](../../crates/graphicle/tools/benchmark_pinned_review.py)
accepts matching binaries, a JSON case list (`name`, `source_store`, `request`),
and a new output directory. It reads archived stores without modifying them and
keeps source bytes, credentials and raw packets in private scratch directories.
Its current fixture scope is one repository and at most 5,000 paths per side.
The eight-PR production comparison still requires its archived stores/requests.

## Cache reuse with a new head (2026-11-04)

Tested the existing release binary at 4e0b996 on later archived heads of the same
two repositories. Base pins stayed unchanged; Nexus changed three owned paths and
platform-rbac one. All 4,523 Nexus and 4,144 platform-rbac dependency/configuration
paths were unchanged, including vendor files and Go module/workspace files.
Every archived object passed hash and size verification. Normal sync admission
into private copies preserved the previously populated caches.

Each measured review started from an independent copy of the previous review's
state. The order was cold, packages-only, normal-reuse, normal-reuse,
packages-only, cold, with both repositories in every pass. Cold removes both
Graphicle caches; packages-only retains exports but removes full analyses;
normal-reuse retains both. No run hit the new head's full-analysis cache, and only
normal-reuse hit the unchanged base's full-analysis cache. Cold still benefits
from package reuse between base and head within its own review.

| Case | Cold review | Dependency exports only | Exports + unchanged base analysis |
| --- | ---: | ---: | ---: |
| Nexus #963 | 28.24 s | 25.82 s (8.6% lower) | 17.26 s (38.9% lower) |
| platform-rbac #638 | 26.52 s | 22.70 s (14.4% lower) | 15.05 s (43.3% lower) |

Values are medians of two samples. Nexus ranges were 28.21–28.27, 25.63–26.02
and 17.24–17.28 seconds; platform-rbac ranges were 26.35–26.69, 22.27–23.13
and 14.98–15.12 seconds. Native-facts medians fell from 21.51 to 19.07 to
12.54 seconds on Nexus, and 20.42 to 16.61 to 10.87 seconds on platform-rbac.
Command peak RSS medians were 823.4 / 705.3 / 587.0 MiB for Nexus and
637.7 / 565.7 / 565.8 MiB for platform-rbac, using `os.wait4` as above.

Nexus saved 1,127 exports totaling 57.0 MiB, but `review_cache::load_packages`
loads at most 32 MiB, reserving half the 64 MiB pool for newly checked packages.
It reused 578 exports on the first analyzed side, with 549 misses. Platform-rbac's
995 exports fit in 20.1 MiB and all hit. This bounds Nexus reuse, but does not
explain all native-analysis time: platform-rbac still spends 10.87 seconds there
with full export reuse and cached base facts. A separate experiment on the load
policy should measure time, memory and packet equality before changing it.

All 12 packets were equal within each case after excluding only `analysis_cache`
and `tokens`; all respected 8,000 tokens (maximum 7,629). The new heads produced
four/five entries and unchanged `incomplete` impact across arms, with 2,856/12,976
issues. This establishes cache reuse without semantic drift on these fixtures,
not a fix for their analysis gaps.

This measures existing behavior; no production code or cache policy changed.
The immediate benefit requires retaining the repository store/anchor across
reviews so existing cache entries remain available. Admission, store preparation,
collection, vendoring and model calls are outside the timed review. OS caches
were not flushed. These older local archives are not the eight production PRs;
two samples do not establish production latency or that Nexus #1009 meets its
deadline. The initial attempt stopped during untimed setup on an older binding
schema; the driver now handles absent `withheld_paths`, and all reported samples
come from the complete second run.

[Raw samples, pins, phase timings and aggregates](next-head-cache-reuse-20261104.json).
The [cache-reuse driver](../../crates/graphicle/tools/benchmark_review_cache_reuse.py)
accepts `--binary`, `--cases` and a new `--output` directory. Each case supplies
`name`, `seed_store`, `archive_store`, `previous_request` and `next_snapshot`.
Original stores are read-only; cloning, admission and cache deletion occur only
in private scratch copies. Credentials and source contents stay out of the
published evidence.

## Go subphase profiling and cache-load experiment (2026-11-04)

`GRAPHICLE_PHASE_TIMINGS=1` now also emits `go_phase=<name> ns=<duration>` to
stderr. `helper` measures preparation of the native executable, including its
first compilation in a process. Native phases are exclusive wall time: nested
imports pause their parent's timer, including recursive fingerprinting and
type-checking. The phases are `preflight`, `discovery` (`go list`), `metadata`,
`fingerprint`, `export_load`, `export_save`, `parse`, `type_check`, `collect`
(symbols/references), `link`, `encode`, `import` (remaining importer work), and
`other`. Sum native phases once, then add helper preparation; the enclosing
`review_native_facts` also includes transport, request decoding and process
overhead. Timings are absent from packets and cached facts. Rust forwards only
known phase names and numeric durations, never arbitrary subprocess stderr.

The helper source changed, so its compiler-cache identity changes too. A rollout
of this instrumentation requires native caches to be rebuilt once. For this
experiment, prior A→B reviews were re-primed with the new helper, then independent
copies reviewed A→C. Both variants contain identical instrumentation and differ
only at `review_cache::load_packages`: the existing `PACKAGE_BYTES / 2` (32 MiB)
load threshold versus `PACKAGE_BYTES` (64 MiB). The pool/store limits remain
64 MiB, with unchanged entry bounds, access checks and fingerprint validation.
The experimental threshold is confined to a saved binary; the working tree
retains 32 MiB loading and its reservation for newly checked dependencies.

The [comparison driver](../../crates/graphicle/tools/benchmark_review_cache_limit.py)
takes `--baseline`, `--candidate`, `--cases` and a new `--output` directory.
Each case has `name`, `seed_store`, `previous_request` and `next_request`; the
seed must already admit all requested snapshots. It clears compiler caches only
in private copies, primes the same baseline helper identity, and compares each
case in 32/64/64/32 order for normal reuse and exports-only reuse. One cold-cache
control per policy/case follows. Every new head must miss full-analysis reuse,
and the base must hit only for normal reuse. It checks whole-packet equality
excluding only `analysis_cache` and `tokens`, and enforces the token budget.

As above, these are older local Nexus #963 and platform-rbac #638 archives with
unchanged dependencies, not the eight production probe PRs. Two samples per
reuse arm and one cold control do not establish production latency. CLI review
is timed; priming, cloning, collection, vendoring, admission and models are
excluded. No compilation or tests run alongside measured reviews, and OS caches
are not flushed. A full 64 MiB load leaves less room for new exports; the
unchanged-dependency cases do not establish behavior when dependencies change.

| Case / cache state | 32 MiB load | 64 MiB load | Command peak RSS, 32 → 64 |
| --- | ---: | ---: | ---: |
| Nexus, unchanged base + exports | 17.45 s | 16.58 s (5.0% lower) | 584.3 → 654.4 MiB (+12.0%) |
| platform-rbac, unchanged base + exports | 15.16 s | 15.31 s (1.0% higher) | 560.7 → 564.7 MiB |
| Nexus, exports only | 27.20 s | 25.79 s (5.2% lower) | 707.9 → 712.7 MiB |
| platform-rbac, exports only | 24.93 s | 26.78 s (7.4% higher) | 565.7 → 569.8 MiB |
| Nexus, cold control (one sample) | 31.06 s | 28.74 s | 826.2 → 843.5 MiB |
| platform-rbac, cold control (one sample) | 28.78 s | 27.29 s | 621.1 → 623.3 MiB |

Reuse values are medians. Nexus normal-reuse samples were 17.46/17.44 seconds
at 32 MiB and 16.77/16.39 at 64 MiB. Its head reused 609/1,127 exports at
32 MiB and all 1,127 at 64 MiB. The newly primed pool loads a different
recency-ordered subset than the previous experiment's 578 hits. platform-rbac
reused all 995 at either limit.
The cold controls differ by more than the small Nexus reuse gain despite
starting without exports. The platform-rbac exports-only arm also slowed despite
its exports fitting under both limits. These observations limit causal claims
about end-to-end latency from this small sample. Some runs rejected one optional
export and rechecked it; packets stayed equivalent. All counters and ranges are
retained in the evidence.

| Normal-reuse native work (median) | Nexus, 32 → 64 MiB | platform-rbac, 32 → 64 MiB |
| --- | ---: | ---: |
| Helper preparation | 4.06 → 4.02 s | 4.02 → 4.15 s |
| Package discovery | 3.25 → 3.43 s | 3.61 → 3.64 s |
| Fingerprinting | 1.40 → 1.47 s | 1.31 → 1.33 s |
| Export loading | 0.33 → 0.58 s | 0.29 → 0.29 s |
| Parsing | 0.87 → 0.04 s | 0.07 → 0.07 s |
| Type-checking, excluding recursive imports | 0.45 → 0.03 s | 0.04 → 0.04 s |
| Export creation | 0.72 → <0.01 s | <0.01 → <0.01 s |
| Symbol/reference collection | 0.11 → 0.11 s | 0.15 → 0.15 s |
| Enclosing native-facts phase | 12.40 → 11.01 s | 10.69 → 10.96 s |

The enlarged cache removes Nexus parsing/checking/export work, but helper
preparation, discovery and fingerprinting still dominate. The native-facts
reduction is 1.39 seconds; the complete review reduction is only 0.86 seconds.
Loading more exports also increases export-loading work, transport and memory.
This does not justify raising the default: retain the 32 MiB reservation.

The next experiment should amortize helper compilation. `go_types::helper`
already caches executable bytes in a process-local `OnceLock`; a fresh CLI
process pays the roughly four-second compilation again. The existing
`impact.report` / `graph.impact` API routes to the same review implementation and
can retain the process between calls. Measure that path with a genuinely new
head and the same packet/cache checks before adding a persistent executable
cache or changing deployment. Package discovery is the next sizable native
phase; type-checking and reference extraction are small with complete exports.

All 20 packets were equal within each case except cache metadata/token counts,
all new heads missed the full-analysis cache, and all stayed within 8,000 tokens
(maximum 7,673). Exclusive subphase sums plus helper preparation fit within the
enclosing native-facts timings in every run. The 12 Go top-level tests passed,
covering invalidation, export/transport bounds, timing opt-in without fact drift,
and nested timing accounting. The Rust diagnostic-filter test passed through
`rustc --test` using the exact function/test module from `go_types.rs`; this is
not a full Rust unit-suite run. Both release builds, driver syntax/help and
`git diff --check` passed. No production settings were changed or deployed.

[All samples, pins, binary/source hashes, policy patch and aggregates](go-cache-load-profile-20261104.json).

## Persistent review process experiment (2026-11-04)

The existing HTTP `graph.impact` command and CLI `review` route to the same
`review::run` implementation. This experiment uses the instrumented 32 MiB
release binary from the previous section; no runtime code or configuration
default changes are needed to measure process reuse.

The [driver](../../crates/graphicle/tools/benchmark_review_server_reuse.py) accepts
`--binary`, `--cases` and a new `--output` directory. Cases provide `name`,
`seed_store`, `previous_request` and `next_request`. In each round, a private
server reviews A→B after its full-analysis cache is cleared, warming the helper
and rebuilding the previous facts. SQLite backups then copy that exact cache
state to independent CLI and fresh-server stores. The comparison requests A→C
in all three arms: unchanged base hit, new head miss. A checksum covers cached
facts, exports, access bindings and row/LRU ordering before each arm. Priming,
cloning and HTTP server startup are outside request timing; CLI time includes
process launch. HTTP timing includes the request and response decoding.

There are two rounds per repository: CLI / fresh HTTP / warm HTTP, then the
reverse order. Warm HTTP keeps the priming process alive. No source collection,
vendoring, models, builds or tests run alongside the measured reviews. The
retained server is idle while its controls run. OS caches are not flushed.
This is a local sequential experiment on the older Nexus #963 and platform-rbac
#638 archives, not a production latency, concurrency or long-term memory test.

The first attempt exposed a material workload difference: both source archives
contain three `project` jobs in `retry`. `serve` starts a workflow worker, which
ran graph projection during priming and retained a large graph in the process.
The mixed-workload attempt completed six measured reviews before interruption;
warm-server process high-water RSS reached 4,869.6 MiB for Nexus and 5,138.6 MiB
for platform-rbac. Those timings are excluded from the isolated comparison.

For the isolated rerun, queued/retry/running jobs are cancelled **only in the
private benchmark clones**, before any server starts. Original stores are
unchanged. Fixtures with connectors are refused so they cannot schedule new
work. Every HTTP call checks that no runnable jobs remain and no graph-projection
phase appears in its logs. Cancelling jobs is a benchmark control, not a proposed
production integration strategy. Normal `serve` runs queued jobs by design;
the stdio MCP path also drains waiting project graphs. A review-only persistent
deployment needs to account for that background work rather than assuming a
normal server is equivalent to the current CLI pilot.

Memory comparisons use the same phase instrumentation in all arms: maximum
sampled current RSS during the measured review, and process high-water RSS.
The warm-server high-water mark includes priming; it is not a per-request
allocation delta. CLI `wait4` peak RSS is retained separately. No idle-memory or
unbounded-duration retention claim is made.

| Case | Fresh CLI | Fresh HTTP | Warm HTTP | Warm versus CLI |
| --- | ---: | ---: | ---: | ---: |
| Nexus #963 | 18.04 s | 19.14 s | 13.45 s | 25.4% lower |
| platform-rbac #638 | 16.62 s | 17.42 s | 11.70 s | 29.6% lower |

Values are medians of two samples. Warm HTTP reduced time by 29.7% versus
fresh HTTP on Nexus and 32.8% on platform-rbac. CLI ranges were 17.55–18.52
and 15.89–17.36 seconds; warm-HTTP ranges were 13.20–13.71 and 11.56–11.84
seconds. Every measured request missed the new head's full-analysis cache.
Within each round, all three arms also had identical dependency hit/miss/reject
counters. This gain is not a repeated review of already analyzed head facts.

| Case | Helper preparation, CLI → warm HTTP | Native-facts phase, CLI → warm HTTP |
| --- | ---: | ---: |
| Nexus #963 | 4.20 s → 1.1 ms | 13.42 → 8.61 s |
| platform-rbac #638 | 4.40 s → 1.1 ms | 12.36 → 7.41 s |

The existing process-local helper cache removes compilation on subsequent calls.
Switching to fresh HTTP processes did not improve these samples; retaining the
process did. The HTTP request boundary includes response JSON decoding while
the established CLI benchmark parses its output after timing, so the fresh-HTTP
control is the closer comparison for attributing process reuse.

| Case | Maximum sampled current RSS, CLI / fresh HTTP / warm HTTP | Process high-water RSS, CLI / fresh HTTP / warm HTTP |
| --- | ---: | ---: |
| Nexus #963 | 565.0 / 571.5 / 735.5 MiB | 580.1 / 586.7 / 735.5 MiB |
| platform-rbac #638 | 546.6 / 553.7 / 612.3 MiB | 560.8 / 567.9 / 612.3 MiB |

These are medians of each run's measurements. Warm-process sampled RSS rose
30.2% on Nexus and 12.0% on platform-rbac relative to fresh CLI. This experiment
does not separate allocator retention from other process caches or establish
whether memory stabilizes over many reviews.

All 12 isolated packets matched within each case after excluding only
`analysis_cache` and `tokens`; all stayed within 8,000 tokens (maximum 7,671).
All base analyses hit, all new heads missed, and cache checksums and per-round
package counters matched. No models were called. Driver syntax/help, evidence
checks and `git diff --check` passed. Original archive jobs/stores were untouched;
all temporary servers stopped. The 32 MiB cache default and production settings
remain unchanged. The next integration requirement is to isolate or explicitly
account for projection work in a retained review service before replacing the
CLI pilot; the isolated benchmark alone does not validate that deployment.

[Samples, cache-state checks, aggregates and excluded mixed-workload observations](persistent-review-service-20261104.json).

## Worker isolation and bounded review soak (2026-11-05)

Added `graphicle serve --no-worker`. This leaves the built-in workflow workers
and their schedulers out of the HTTP process; ordinary `serve` and `open` retain
their existing behavior. The mode preserves queued jobs for a separate worker.
It does not restrict explicit API operations or extension background tasks.
The [pinned-review guide](../pinned-review.md#persistent-review-serving) documents
the lifecycle and Python SDK usage.

A real-process [lifecycle test](../../crates/graphicle/tests/test_serve_no_worker.py)
admits Python snapshots through HTTP, checks a review and an unauthorized-call
refusal, verifies that jobs remain queued across multiple polling intervals,
processes those jobs with separate `worker --once` calls, and restarts ordinary
`serve` to verify automatic processing of subsequent jobs and link work. It
passed. The initial local build missed the crate-local static-parser settings;
the admission test caught that before the soak. The measured binary was rebuilt
from `crates/graphicle` with Rust 1.94.1, its `.cargo/config.toml`, prepared parser
sources, and the macOS 15.4 SDK. The 32 MiB export-load default remains unchanged.

The [soak driver](../../crates/graphicle/tools/benchmark_review_soak.py) derives
eleven synthetic heads per repository from the older archived Nexus #963 and
platform-rbac #638 heads. It appends an unused Go constant to an owned, non-test
source file, changing its value for every head. These are synthetic pins, not
real Git commits or eleven independent PRs. Vendor/module inputs are unchanged;
stale dependency-preparation metadata is omitted rather than claiming a fresh
collection/verification. Every head goes through ordinary sync admission.
No original archive or checkout is modified.

Head zero warms the process. Ten distinct measured heads then run as four
sequential requests, two pairs at concurrency two, and two sequential recovery
requests. Each has a fresh CLI control from an independent clone of the same
post-warm cache state. Nexus runs CLI controls first; platform-rbac runs the
retained service first. The service naturally accumulates caches between calls,
whereas each CLI control starts from the common seed. This measures a bounded
sequence, not an isolated helper-cache ablation. Every measured call must hit
the unchanged base and miss its new head's full-analysis cache. Complete packets
must match their same-pin controls excluding only `analysis_cache` and `tokens`.

Projection jobs are left intact throughout this experiment, with fourteen jobs
in each private store after admission. The driver checks job IDs, kinds, states,
attempts and leases before and after each server batch; it refuses fixtures with
connectors. It also checks that no background graph-projection phase ran. The
server log is retained beyond the SDK's recent-line buffer. Concurrent batches
report combined phase/RSS measurements; their interleaved phases are not assigned
to individual requests. Individual request latency and batch wall time are
recorded separately.

Admission, source collection, vendoring, cloning and warmup are outside review
timing. No models were called and no builds/tests ran alongside timed reviews.
HTTP includes response decoding; CLI includes process launch but parses its
output after timing, as in the earlier tests. OS caches were not flushed and
unrelated host activity was uncontrolled. RSS measurements cover the server
process, not the sum of its Go subprocesses. First-phase RSS already includes
source loading; no idle-RSS claim is made. This finite local test cannot establish
long-term memory stability, arbitrary concurrency, production deadlines or the
eight-PR production probe's inclusion rate.

All twenty same-pin pairs matched across forty measured reviews. Every unchanged
base hit the full-analysis cache and every new head missed it. All packets stayed
within 8,000 tokens (maximum 7,708). All fourteen jobs in each server store stayed
unchanged, and no background graph-projection phases ran.

For the six serial heads per repository (four initial and two recovery calls):

| Case | Fresh CLI median | Retained server median | Reduction |
| --- | ---: | ---: | ---: |
| Nexus #963 | 16.99 s | 14.07 s | 17.2% |
| platform-rbac #638 | 17.98 s | 13.27 s | 26.2% |

Concurrency two preserved packet correctness but increased request latency:

| Case | Initial serial median | Concurrent request median | Two-request batch times | Recovery median |
| --- | ---: | ---: | ---: | ---: |
| Nexus #963 | 13.24 s | 25.08 s | 25.53 / 24.65 s | 15.13 s |
| platform-rbac #638 | 13.53 s | 18.93 s | 19.23 / 18.77 s | 13.27 s |

Observed throughput increased by 10.1% on Nexus and 38.3% on platform-rbac
relative to the initial serial phase. Those phases use different synthetic heads,
so this is not a paired same-head throughput comparison.

Maximum sampled server RSS during the initial serial phase was 778.5 MiB on
Nexus and 694.0 MiB on platform-rbac. Concurrent batches reached 1,205.5 MiB and
1,157.2 MiB respectively. The final recovery requests still reached 1,207.4 MiB
and 1,105.6 MiB: memory did not return to its earlier serial level within this
test. These observations do not establish a leak or long-term stability.

Start integration with one in-flight review per process. Worker isolation is
verified; a live eight-PR probe and longer memory/idle qualification remain
before production rollout. No production settings were changed.

[Raw samples, pins, source hashes, aggregates and validation results](review-worker-isolation-soak-20261105.json).

## Longer retained-process memory investigation (2026-11-05)

The [memory driver](../../crates/graphicle/tools/benchmark_review_memory.py)
extends the same archived Nexus #963 and platform-rbac #638 fixtures to thirty
unseen synthetic heads per repository, with one request at a time. It uses the
same binary as the worker-isolation soak. No runtime behavior or production
settings change. The shared admission helper now accepts a head count; its
existing eleven-head default is unchanged.

Each case has a one-minute pre-admission baseline, one priming review, three
groups of ten new-head reviews separated by one-minute idle pauses, ten cached
replays of the final head, and another idle pause. The server then restarts
against the same persisted store, idles, replays the same cached head, and idles
again. Fresh CLI controls for heads 1, 15 and 30 run only after both servers have
stopped, from independent copies of the pre-review cache state. Packet equality
excludes only `analysis_cache` and `tokens`; budgets and base/head cache behavior
are checked separately. Jobs remain queued, no connectors or models run, and
original archive stores are read-only.

Native macOS `libproc` samples only the known server and its recursively
discovered children every 250 ms. It records RSS, physical footprint and thread
counts without collecting command lines. Process-tree sums can double-count
shared pages; short-lived children between samples and descendants reparented
outside the tree can be missed. The samples are not an exact high-water mark.
Physical footprint and RSS are different OS accounting measures, neither a count
of live Rust objects. Heap inspection through `vmmap` was unavailable under the
local process permissions. Sampling was checked with a known live 48 MiB
allocation and parent/child discovery before the timed run.

The fixture uses constant-sized source trees and unchanged dependency inputs.
It does not cover repository churn, large varied edits, concurrent admission,
production Linux allocator behavior, or an indefinitely running service.

The main probe completed ninety reviews: sixty new heads, two priming calls,
twenty cached replays, two cached replays after restart, and six fresh CLI
controls. All twenty-eight same-pin packet comparisons passed. All expected
base/head cache states and token budgets passed; maximum server packet size was
7,708 / 8,000 tokens. Queued/retry jobs were unchanged and no background graph
projection or model calls ran.

| Case | New-head batch peak server RSS, heads 1–10 / 11–20 / 21–30 | Maximum sampled new-head process-tree RSS | New-head batch median latency |
| --- | ---: | ---: | ---: |
| Nexus #963 | 945.8 / 775.7 / 795.1 MiB | 1,379.9 MiB | 12.88 / 12.57 / 12.72 s |
| platform-rbac #638 | 776.0 / 702.4 / 701.7 MiB | 1,013.7 MiB | 10.25 / 10.28 / 10.89 s |

These are sampled peaks within the new-head requests, not memory limits or
exact process high-water marks. Platform-rbac's final batch was about 6% slower
than its first; no causal performance conclusion is drawn from this uncontrolled
host run. Both repositories' later batches remained below their initial peak
server RSS.

| Case | Idle RSS after heads 10 / 20 / 30 | Idle physical footprint after heads 10 / 20 / 30 |
| --- | ---: | ---: |
| Nexus #963 | 735.0 / 727.2 / 759.0 MiB | 194.9 / 182.0 / 212.0 MiB |
| platform-rbac #638 | 649.3 / 631.5 / 596.6 MiB | 171.8 / 145.3 / 100.0 MiB |

Every completed idle window had zero sampled children and ended with twenty
server threads, the same as its startup baseline. In several windows an RSS
drop coincided with the thread count falling at about ten seconds. The HTTP
command handler uses `spawn_blocking`; the locked Tokio 1.53.3 implementation's
default blocking-thread keep-alive is ten seconds. This is consistent with
thread-associated retention being released on retirement, but does not establish
ownership of all remaining memory. Full compiler facts stayed at four entries;
dependency exports stayed at 1,127 / 57.0 MiB on Nexus and 995 / 20.1 MiB on
platform-rbac at all three checkpoints.

The ten cached replays took median 2.31 s on Nexus and 2.00 s on platform-rbac.
After their idle pause, RSS was 684.7 and 720.1 MiB respectively. A fresh process
using the same store produced the same packet in 2.31 and 2.02 s, then settled
at 483.0 and 504.6 MiB idle RSS. Physical footprint tells a different story:
Nexus was 136.7 MiB before restart and 152.0 MiB after, while platform-rbac was
207.1 and 197.6 MiB. Lower RSS after restart does not by itself demonstrate that
live application allocations were accumulating.

Platform-rbac's ten cached replays ended on rising RSS, so an additional
[cached-burst driver](../../crates/graphicle/tools/benchmark_review_cached_memory.py)
ran fifty uninterrupted cached requests in a fresh process with a private copy
of the final store. Every packet matched the original and every call hit both
full-analysis caches. This tested whether the short run had stopped during a
temporary rise:

| Cached requests | Peak sampled server RSS | Peak sampled physical footprint | Median latency |
| --- | ---: | ---: | ---: |
| 1–10 | 705.4 MiB | 457.6 MiB | 2.07 s |
| 11–20 | 941.4 MiB | 559.9 MiB | 2.09 s |
| 21–30 | 1,011.5 MiB | 630.0 MiB | 2.05 s |
| 31–40 | 925.5 MiB | 541.9 MiB | 2.05 s |
| 41–50 | 668.1 MiB | 257.7 MiB | 2.04 s |

Memory fell during continued requests, without a restart or an intentional idle
gap. After sixty seconds idle it settled at 569.2 MiB RSS / 156.8 MiB physical
footprint. Cache counts/payload sizes and job states remained unchanged.

Across both experiments, 140 reviews and 78 semantic packet comparisons passed.
The five process traces contain 7,025 samples; maximum interval was 264 ms and
maximum sampling work was 2.49 ms. All temporary servers stopped. No confirmed
leak or steadily rising retained baseline was found in these workloads. High RSS
retention and later reclamation are observable; exact allocation ownership remains
unproven. No allocator tuning or automatic RSS-based restart policy is introduced.
Keep one in-flight review per process and qualify process-tree/cgroup memory on
Sentinel's Linux host before setting production memory limits. Longer varied
new-head traffic without periodic idle pauses remains outside this test.

[Per-request measurements, idle endpoints, pins, hashes and checks](retained-review-memory-20261105.json).

## Captured-store soak corrections (2026-11-05)

Sentinel reported that swapping `fcddad5b` for `4da9de65` on its fresh-store path
produced identical packets on five Go-changing heads, with timings within 3%.
The other three heads changed no Go and were skipped. The reported step times
were metadata-service #316 157 → 161 s, argus #141 138 → 141 s, Nexus #1028
476 → 479 s, siren #174 98 → 100 s and odin #199 155 → 155 s. Repeated source
admission/preparation dominated that path. These are supplied probe results,
not local reruns, and do not establish an end-to-end gain from a binary swap.

Sentinel's Siren soak did complete: it reported median warm new-head review
latency of 6.3 s for retained HTTP versus 13.3 s for fresh CLI, twenty matching
packet pairs, 7.0 s per concurrent pair and normal recovery runs. That review-call
gain does not include source admission. The four other cases stopped in the
harness: three manifests exceeded the per-request 5,000-entry cap, and Nexus's
captured store did not already contain cached base facts.

The shared [soak helpers](../../crates/graphicle/tools/benchmark_review_soak.py)
now send manifest parts of at most 5,000 entries, preserve metadata and expected
snapshot on the first part, and continue using the returned `sync_id` until
`manifest_complete:true`. Synthetic heads still require exactly one missing
object: their edited file. Existing retained objects are not uploaded again.

Warmup now accepts a cold captured base, analyzes head zero, and replays the
same request to require both sides to hit the full-analysis cache and the packet
to match. Both calls are untimed preparation, recorded in `prime_untimed` and
`prime_untimed.verification`. An uncacheable fixture still fails verification.
Measured heads retain the strict invariant: base hit, previously unseen head
miss, correct pins and bounded tokens. CLI controls are cloned only after warmup
verification. The memory driver uses the same helper and keeps warmup in
`prime_untimed`, outside its measured `rows` array. Historical evidence above
retains the earlier priming method and row counts.

The regression also exposed Python SQLite resource warnings in the benchmark
helpers. Their read and backup connections now close explicitly; a SQLite
transaction context alone does not close the connection. Write contexts retain
their existing commit/rollback behavior. This changes benchmark resource handling,
not the Graphicle server's allocator or store implementation.

The [regression test](../../crates/graphicle/tests/test_review_soak.py) covers
manifest boundaries at 0, 5,000, 5,001 and 10,001 entries and rejects invalid
measured cache states, pins and budgets. Its real-process case starts from a
5,003-file captured store with zero compiler fact sets and runs the full soak,
checking all ten same-pin CLI/server pairs, warmup verification, the original
store's cache/job state and server cleanup.

Run it with Go on `PATH` and a built binary that supports `serve --no-worker`:

```sh
GRAPHICLE_BIN=/path/to/graphicle python3 -m unittest discover \
  -s crates/graphicle/tests -p test_review_soak.py -v
```

These are benchmark-only changes; Sentinel can rerun the four blocked cases
with the updated Python tools and the same binary. Persistent-store integration
and live end-to-end qualification remain pending; concurrency two is not promoted
based on the Siren result alone.

## Sentinel retained-service handoff (2026-11-05)

The implementation and integration contract are documented in
[persistent review serving](../pinned-review.md#persistent-review-serving).
This work targets service transport, cancellation, and store reuse; it does not
claim a measured improvement to Sentinel's source collection or dependency
preparation times.

| Item | Graphicle change | Remaining qualification |
| --- | --- | --- |
| G1: Unix transport | Core `serve --socket`, private directory/0600 socket, inode-safe cleanup; Python SDK Unix HTTP transport | Sentinel's sandbox/service-manager integration and Linux qualification |
| G2: request deadline | Unix `timeout_ms`, cooperative phase/file checks, bounded subprocess and SQLite waits; interrupted helper compilation stays retryable | Set a longer transport limit; a lost response still requires supervision; CPU/filesystem checks are cooperative |
| G3: snapshot context | Reuse existing `context` on the review project; SDK secondary retrieval now filters to `file:` sources, disables memory retrieval, and checks the head snapshot | Serialize admission/retrieval per project or handle `stale_snapshot`; historical snapshot queries are not added |
| G4: incomplete impact | No weakening of the v2 risk contract; packet trimming already does not change `impact` | Representative current production packet/histogram needed to diagnose interface gaps; unresolved dispatch and dependency effects remain honest analysis limits |
| G5: captured-store soak | Multipart manifests and verified cold-cache warmup, detailed above | Rerun the four previously blocked production captures |
| G6: dependency closure selection | Deferred | Collector must continue supplying verified pinned dependencies |

G4 inspection confirms two distinct values: `impact` is calculated from all
analysis issues before packing; `coverage.complete` also reflects delivery
omissions. Interface candidate expansion emits potential implementation edges
but does not prove a closed set of runtime receivers. It deliberately retains
`interface_dispatch` (or its candidate-budget limit). Dependency API resolution
also does not prove library implementation effects/callbacks, reported as
`dependency_effects_unmodeled`. Dropping those warnings would change the meaning
of the risk schema, not fix a transport or packing defect. A simple fully
resolved Go change can produce `potential_impact`; that does not establish
complete coverage for the supplied production runs.

Validation on the local macOS build:

- Four Unix socket tests passed: private-parent/path refusal, permissions,
  capability enforcement, no TCP client path, live duplicate-start protection,
  graceful restart, and replacement-inode preservation.
- Two retained-service tests passed: same-head context with unchanged queued
  jobs/snapshots and stale-head refusal; cancellation of an intentionally stalled
  Go compilation and its child, timeout of a concurrent initializer waiter,
  successful retry in the same server, and subsequent full-cache hits without
  recompiling the helper.
- Three subprocess unit tests passed, compiled directly from `process.rs` using
  the release dependency artifacts. They cover nested request scopes/restoration,
  subprocess group timeout, pipe draining, and bounded output.
- The 39 SDK origin/deadline/retry and secondary-evidence checks passed.
- The updated 5,003-file soak passed on the final binary: 20 measured reviews,
  ten matching same-pin CLI/server pairs, strict cache/pin checks, cold-base
  warmup and verification, unchanged original stores/jobs, and no SQLite
  `ResourceWarning` with warnings enabled.
- The existing full/delta budget-invariance test passed for all three impact
  values at 2,000, 8,000 and 64,000 tokens. This validates packing semantics,
  not production call-graph completeness.

An initially overbroad HTTP lock test demonstrated that ingress accounting
reserves the request in a write transaction **before** review dispatch. A lock
held before admission therefore returns the existing storage refusal, not the
review execution timeout. The execution deadline does not cover HTTP admission
or completion accounting. The documentation makes this boundary explicit;
transport failures still require the collector's supervision policy. The
review-layer lock check exercises the actual bounded execution wait directly.

The direct review-layer storage test also passed: a held writer lock produces
`timeout` within the execution budget, then releasing the lock lets the next
request return the same packet identity. Both targeted Rust review tests passed
together. The release binary built successfully; `git diff --check` and Python
syntax checks passed. Changes are local and uncommitted; no service was deployed.

## G4 investigation: global completeness versus useful impact evidence (2026-11-05)

**Finding:** current `impact` is gated by completeness of the entire admitted
analysis. It is not a severity score or a predicate saying that no useful impact
was found. Fixing interface resolution alone cannot make typical dependency-using
Go snapshots complete under the current v2 contract.

The investigation used the unchanged runtime at source revision `48e9816`, a
locally built binary identified by SHA-256 in the evidence, and 16 synthetic
review calls across 15 fixtures. It also inspected two older **PR-derived soak
heads**, Nexus #963 and platform-rbac #638. These are not Sentinel's latest
122-run production cohort. Their original packets and stores were read only.
Each native fact set was matched to the exact base/head snapshot via the cache's
input table; every unresolved-call cause count matched the packet histogram.
No model calls or production operations were performed.

[Machine-readable results, pins, hashes and full histograms](g4-investigation-20261105.json).
[Reproducer](../../crates/graphicle/tools/investigate_review_coverage.py):

```sh
python3 crates/graphicle/tools/investigate_review_coverage.py \
  --binary /path/to/graphicle --output /private/tmp/new-g4-investigation
```

Optional `--archive name=/path/to/packet.json` and
`--archive-store name=/path/to/store` inspect retained facts for that packet.
Missing or ambiguous cache matches are reported instead of substituting another
revision. Use a new output directory; it contains private synthetic stores and
raw fixture packets. The report does not contain workspace capabilities.

### What the fixtures establish

All cases attest complete source admission. Except for the explicit packing
case, no entries or diffs were omitted for budget.

| Fixture | `impact` | Analysis issues, base + head |
| --- | --- | ---: |
| Simple Go function change | `potential_impact` | 0 |
| Identical simple Go source | `no_changes_in_scope` | 0 |
| Same change plus unchanged `tooling/settings.json` | `incomplete` | 2 unsupported inputs |
| Same change plus interface dispatch in a separate, unimported package | `incomplete` | 2 interface calls |
| Identical source containing that interface call | `incomplete` | 2 interface calls; zero changed declarations |
| Same change plus a broken, unimported package | `incomplete` | 4 package/type failures |
| Concrete receiver, explicit interface conversion, or single-write local interface binding | `potential_impact` | 0 in each of three cases |
| Interface stored in a local struct literal | `incomplete` | 2 interface calls |
| Interface returned by a simple constructor | `incomplete` | 2 interface calls |
| Resolved `strings.TrimSpace`, either in changed code or an unimported package | `incomplete` | 2 dependency-effect limits in each case |
| Resolved, admitted vendored pure function | `incomplete` | 2 dependency-effect limits |

The packing control returned `potential_impact` at both 4,000 and 64,000 tokens,
with zero analysis issues. At 4,000 it omitted 24 candidates and delivery coverage
was incomplete; at 64,000 nothing was omitted and coverage was complete. Thus
packing is not the remaining cause of G4.

The unrelated-interface fixture exposes a useful existing distinction: its
changed head entry has `review_basis.complete=true` and
`reason=complete_static_component`, while global `impact` is incomplete.
Component reasoning already exists. This delivered-context/reuse proof can be
invalidated by packing, so it must not be substituted for an analysis-only risk
verdict.

### What the archived cases establish

Counts below combine base and head. Native examples were checked against the
matching cached facts, including the synthetic soak-head revision.

| Case | All issues | Interface dispatch | References in incomplete packages | Other issues |
| --- | ---: | ---: | ---: | ---: |
| Nexus #963-derived head | 2,846 | 2,179 | 0 | 667 |
| platform-rbac #638-derived head | 12,972 | 1,626 | 10,959 | 387 |

Nexus has zero Go type-check failures in this capture. Its other limits include
268 depth limits, 188 unsupported inputs and two dependency-effect limits.
platform-rbac has 21 failed package instances on each side, with no unavailable
external packages; a retained diagnostic reports `undefined: IsMemberBlocked`.
These are not evidence that another module cache or another interface index
would solve its package failures. The source/build discrepancy needs its own
investigation before attributing the error to the collector or repository.

For Nexus, 1,534 of 2,179 unresolved interface sites (70.4%) already have
implementation candidates. Its largest interface-bearing files include the XDS
metrics modules; common spellings include `err.Error`, `resp.Body.Close`, and
`ctx.Done`. For platform-rbac, 436 of 1,626 sites (26.8%) have candidates;
`pkg/db/mock/store.go` contributes 330 sites **per side**, and `db.PrepareContext`
contributes 230 per side. These are observed source spellings, not a safe-API
classification. Generated mocks are ordinary `.go` files: `tests:false` does not
establish that they are outside a production dependency closure.

Deleting every interface warning would still leave both archived results
incomplete. A method-set candidate list also cannot establish an exhaustive set
of runtime receivers, particularly for values supplied by dependencies or callers.

### Responsible code and recommended next work

- `review.rs::analyze_go` adds `dependency_effects_unmodeled` for each consumer
  build with dependency packages or resolved boundary calls. Even a standard
  library call can trigger it. This is intentional API-only modeling, not a
  missing-module failure.
- `review_admission.rs::assess` starts with all analysis issues. `review.rs::compare`
  combines both sides, adds unsupported inputs from the full source inventory,
  and declares completeness only when the combined issue list is empty. It does
  not restrict this verdict to selected entries or changed components.
- `go_review.go::localValueBindings` and `boundExpression` already resolve safe
  local initializers and concrete-to-interface conversions. Extending them for
  constructor results and struct-field flows requires tracking writes, aliases,
  escapes and interprocedural effects; the fixture is not permission to guess a
  receiver for general constructor calls.
- `go_review.go::expandInterfaces` already emits `may_call` implementation edges
  and deliberately retains the unresolved interface call.
- `review_reuse.rs::annotate` already maps package-local unresolved calls to
  connected import components, but other issue classes currently block all
  component proofs. `Scopes::finish` additionally checks delivered context.

Recommended order:

1. **Separate the intended questions before changing the verdict.** Sentinel can
   already consume `changes`, `entries` and their selection reasons as positive
   static evidence while retaining `impact=incomplete`. Neither those fields
   nor `potential_impact` supplies defect severity or a low-risk clearance.
2. **For a practical completeness signal, specify its scope explicitly.** A
   possible future contract is completeness of changed owned-Go components and
   dependency API resolution, with implementation effects/callbacks explicitly
   outside that scope. Preserve v2's global verdict; changing its meaning needs
   a versioned contract and comparison probes. Reuse the existing component
   ownership machinery and report analysis coverage before packing. Unknown
   ownership, missing inputs and unknown cross-boundary edges must remain
   blockers; absence of a resolved graph edge is not proof of irrelevance.
3. **Then improve confirmed receiver-flow precision.** The new field and
   constructor fixtures reproduce gaps that local-initializer work would not
   fix. Add counterexamples for reassignment, alias mutation, escaped pointers,
   multiple constructor returns and dependency-supplied implementations before
   allowing a complete resolution.
4. **Re-measure real causes.** Obtain the latest 122-run packets/histograms and
   inspect package failures, generated mocks and non-Go inputs against their
   actual build/admission contracts. Do not suppress code merely because its
   path contains `mock`, or label arbitrary JSON/YAML irrelevant.

No runtime implementation, risk schema, source exclusions, or Sentinel behavior
was changed by this investigation. Validation checked all 16 completed review
calls, token bounds, full histogram sums, budget independence, the existing
component proof, and exact-pin archive/native count agreement. Python syntax and
`git diff --check` also passed.

### G4 implementation: scoped Go analysis coverage (2026-11-05)

Implemented recommendation 2 above on the working tree based on `48e9816`.
Pinned packets now include `analysis_coverage` with the independent format
`graphicle.changed-go-coverage.v1`. The existing import-component construction is
shared with review reuse; its graph and fingerprint rules are unchanged. The
assessment runs before packing, using full file changes and issues. It certifies
only owned Go import components and resolved dependency APIs, explicitly excluding
dependency implementation/callback effects, non-Go semantics and runtime behavior.
Unknown ownership, compiler/admission failures and unresolved calls in changed
components still block; changed unmapped files and dependency binding/layout
changes are also conservative blockers. This does not grant review skipping.

The [comparison artifact](g4-scoped-coverage-20261105.json) records binary/source
hashes and all 16 baseline/candidate synthetic packet comparisons:

| Measurement | Result |
| --- | --- |
| Global v2 impact and full issue histograms | Unchanged on all 16 packets |
| Scoped coverage | 11 complete, 3 incomplete, 2 not applicable |
| Previously globally incomplete, now scoped complete | 5: unchanged unrelated JSON, disconnected interface package, unrelated stdlib use, changed stdlib caller, vendored dependency API caller |
| Actual blockers retained | Broken package, interface field dispatch, constructor-returned interface dispatch |
| Delivered head review-basis summaries | Unchanged on all 16 packets |
| New field size | 519–525 bytes of compact JSON in these fixtures |
| Entry counts | Unchanged on 15 packets; at 4,000 tokens, packing fixture delivered 2 code entries versus 3 before, with 24 total entry/diff omissions in both runs |

The fixed metadata consumes response budget. Packet identities change, and tight
budgets can deliver fewer entries; this is not a latency or capacity improvement.
Token totals also depend on generated identifiers and selected context, so their
before/after difference is not an isolated tokenizer cost for the new field.

Validation: 5 targeted Rust unit tests (including schema rejection of
contradictory claims, base-only imports, build-world separation and full/delta
packing), 3 existing review-reuse integration tests, and 3 real-service Python
tests covering 24 scenarios / 27 calls passed. The service tests cover direct and
reverse imports, partial admission, changed config/module/vendor/embed inputs,
binding changes, capability advertisement and budget/delta/provided-diff
invariance. The separate 16-packet comparison made no model calls.

Reproduce the new service checks with a built binary:

```sh
GRAPHICLE_BIN=/path/to/graphicle python3 -m unittest discover \
  -s crates/graphicle/tests -p test_review_analysis_coverage.py -v
python3 crates/graphicle/tools/investigate_review_coverage.py \
  --binary /path/to/graphicle --output /tmp/new-g4-coverage-run
```

No production deployment or latest 122-run re-probe was performed. Genuine
interface-dispatch gaps remain unresolved. Sentinel should record the new scoped
field alongside global impact and delivery coverage, then measure how often
changed production components remain blocked and by which issue bucket.

### G4 follow-up: interface receiver origins (2026-11-05)

Investigated the remaining field and constructor gaps on source revision
`37bece6`, without changing runtime resolution or relaxing coverage. The
[reproducer](../../crates/graphicle/tools/investigate_interface_receivers.py)
builds the current native helper from bundled sources with offline module policy,
then checks 27 synthetic cases. An optional Graphicle binary measures the same
cases as pinned base/head reviews, changing `Concrete.Get` in each package.
Reviewed code is parsed/type-checked, never executed. The packet binary is the
same binary recorded by the preceding scoped-coverage validation. Source and
binary hashes, complete fixture sources, native call identities, candidates and
packet coverage are in [the results](g4-interface-receivers-20261105.json).

| Group | Cases | Native receiver result | Scoped packet result |
| --- | ---: | --- | --- |
| Already supported controls | 5 | Exact `Concrete.Get` target | Complete |
| Interface-returning constructors | 5 | `interface_dispatch` | Incomplete |
| Interface-valued fields | 5 | `interface_dispatch` | Incomplete |
| Mutation, unknown-input and richer-proof cases | 12 | `interface_dispatch` | Incomplete |

Every packet has exactly one changed component. Each unresolved case has two
component blockers (one per side); there are zero global, outside-component,
outside-scope, binding-change or unmapped-file blockers. All native packages
type-check. Each unresolved site has two `interface_implementation` candidates,
including the immutable cases: candidates are possible implementations of the
contract, not evidence of which value reaches the call. These are controlled
examples, not an estimate of the production frequency of either receiver shape.

#### Confirmed causes

1. `go_review.go::localValueBindings` (line 438) tracks single initializing writes
   to local identifiers and rejects reassignment/address escape.
   `boundExpression` (line 509) follows those bindings and interface conversions.
   It stops at selectors and ordinary calls. The receiver resolver (line 787)
   consequently sees the declared interface type for both `h.value` and `New()`.
   Direct concrete receivers, explicit interface conversions, single local
   interface bindings, concrete-typed fields, and constructors declared to return
   a concrete type already resolve; they do not need another implementation.
2. There is no return-type summary for owned functions whose declared result is
   an interface. Even `func New() Value { return Concrete{} }` stays unresolved.
   Returning `&Concrete{}`, storing `New()` in a local, same-concrete-type branches,
   and a forwarding factory all reproduce the gap.
3. Field handling is absent from the exact receiver proof. Immediate literal
   projection, keyed and positional local literals, pointer literals, and an
   embedded interface's promoted method all remain unresolved.
4. `functionValueFlow` (line 1341), `candidates` (1481), and `storage` (1559) are
   existing bounded **possible-target** machinery. They pool assignments and
   parameter inputs, track storage escapes, and emit `may_call` edges while
   preserving uncertainty. They are not an exact-value proof. In particular,
   their conservative escape pass marks method receivers as unsafe storage, so
   the very `h.value.Get()` call can disqualify `h`. Reusing this path unchanged
   cannot establish an exact receiver; deleting its guards would be unsound.

#### Smallest useful implementation order

**First: exact return summaries for owned constructors.** Start with direct,
non-generic functions in the analyzed package, an unnamed single result, and a
body consisting of one explicit return whose expression has a compiler-known
concrete named or pointer type. Match compiler function identities, not `New`
spelling. Follow the existing single-write local binding when the result is
stored before calling the method. Retain the ordinary call edge to the factory
so its changes still invalidate callers. This targets three reproduced cases:
single return, local result, and pointer return. Do not inspect dependency
implementations across the declared API-only boundary. Named results, defer,
unknown results, function-value factories, recursion, branches and forwarding
calls stay outside this initial proof; add them only with separate evidence.

**Second: immediate literal field projection.** A receiver such as
`(Holder{value: Concrete{}}).value.Get()` has no intervening local storage to
mutate. Resolve the selected field by compiler identity/index, preserving Go's
pointer and promoted-method rules. Begin with direct fields; nested and promoted
fields need dedicated cases. This is smaller than general heap/alias analysis.

**Third: local struct field values.** Reuse the single-write binding and mutation
analysis where their invariants fit, but establish an explicit exact-storage
proof. At minimum, field writes, pointer aliases, taking the holder's address,
passing it to unknown code, implicit address-taking by pointer receiver methods,
and captured closure writes must invalidate an initializer-based proof. The
current candidate-flow result must never silently become an exact receiver.

The counterexamples make the required refusals concrete:

```go
// Reading the explicit return alone would incorrectly select Concrete.Get.
func New() (v Value) {
    defer func() { v = Other{} }()
    return Concrete{}
}

// There is only one write to the local h, but the interface field changes.
h := &Holder{value: Concrete{}}
alias := h
alias.value = input
return h.value.Get()
```

Additional cases cover mixed return types, passthrough interface parameters,
recursive factories, direct field writes, pointer-receiver mutation without
explicit `&h`, escaped holders, closure mutation, zero interface fields,
reassigned interface locals, and address escapes. Some can eventually support
more precise analysis; none may be resolved merely by choosing the initializer
or the first observed return.

Acceptance for each implementation: intended fixtures acquire the correct exact
method target and lose only their justified `interface_dispatch` limits; unknown
cases retain uncertainty; changing the factory or initializer changes its target;
base/head ownership and review-reuse proofs remain correct. Native helper source
changes must invalidate the affected cached facts using the existing analyzer
identity, verified in a retained-store test. Measure production receiver shapes
inside changed components before estimating benefit: the current generic
`interface_dispatch` histogram cannot distinguish these origins.

Validation completed: 27 native analyses and 27 pinned review calls, all expected
results, zero model calls, Python syntax check and `git diff --check` passed.
No production analyzer changes or deployment occurred.

```sh
python3 crates/graphicle/tools/investigate_interface_receivers.py \
  --binary /path/to/graphicle --output /tmp/new-interface-receiver-probe
```

### G4 receiver fixes and reassignment reassessment (2026-11-05)

Implemented the ten resolvable field/factory cases from the preceding probe.
The existing exact local bindings now feed a bounded receiver proof in
`go_review.go`. It resolves compiler-selected fields of fresh literals and
unexposed single-write local holders, including nested/pointer/promoted fields.
All uses of a holder are checked before trusting its initializer; reassignment,
field writes, address/alias escapes, captured writes and potentially mutating
concrete holder methods invalidate that storage proof.

Direct free functions in the same owned package receive exact return summaries
only when every return proves the same concrete type. This covers forwarding
functions and same-type branches as well as single returns. The original factory
call edge remains. Named results/defer, recursive or generic factories, unknown
or mixed return types, and cross-package interface-returning APIs remain
unresolved. Resolution is bounded to 256 expression steps and uses compiler
objects, not spelling or pooled implementation candidates. The existing candidate
analysis continues to provide `may_call` context for unresolved sites.

[Before/after evidence](g4-interface-receiver-fix-20261105.json) records the exact
baseline/candidate binaries and native source hashes on the same 27 fixtures:

| Measurement | Before | After |
| --- | ---: | ---: |
| Complete changed-component analysis | 5 | 15 |
| Component issue instances, base + head | 44 | 24 |
| Mutation/unknown-value cases retaining uncertainty | 12 | 12 |
| Code entries delivered across the matrix | 162 | 162 |
| Entries/diffs omitted for budget | 0 | 0 |

All ten intended cases now bind `Concrete.Get` directly. The twelve refusal
cases retain their original issue histograms. No analysis-scope, global verdict
definition, packing rule or review-skipping permission was weakened. The global
verdict becomes `potential_impact` on the ten cases because their actual analysis
gap is resolved, not because a warning is ignored.

Validation passed:

- Full Go helper test suite; 36 exact-receiver regression cases also verify
  field/whole-holder reassignment, implicit address-taking, pointer/field aliases,
  closure and range writes, nested escapes, nil handling, mixed returns, deferred
  named-result mutation and a 300-function chain exceeding the analysis bound.
- Three new service tests: the 27-case matrix; warm target and review-reuse
  invalidation for both factories and fields; and old-to-new binary cache upgrade.
- Three existing scoped-coverage service tests, including budget, delta and
  supplied-diff invariance, admission failures and changed unmodeled inputs.
- Separate 27-native / 27-packet before/after probe, Python syntax and
  `git diff --check`.

The reassignment check first warms an exact `Concrete.Get` result, changes the
initializer to `Other`, and verifies an exact `Other.Get` result with no stale
review reuse. Reassigning the interface from an unknown input then restores
`interface_dispatch` and incomplete scoped coverage. This implementation refuses
mutated storage across the function; it does not claim flow-sensitive knowledge
of the last assignment.

The retained-store upgrade test verifies that new helper identity rejects the
old incomplete native facts, recomputes complete analysis, and hits the cache on
the following identical request. The first review after an analyzer upgrade
therefore needs to rebuild affected facts. No structural parse-cache revision or
request/schema change is needed.

Reproduce with the new build (`--expect-resolved` selects the post-fix assertions):

```sh
GRAPHICLE_BIN=/path/to/new/graphicle \
GRAPHICLE_BEFORE_RECEIVERS_BIN=/path/to/old/graphicle \
  python3 -m unittest discover -s crates/graphicle/tests -p test_interface_receivers.py -v
python3 crates/graphicle/tools/investigate_interface_receivers.py \
  --expect-resolved --binary /path/to/new/graphicle --output /tmp/new-receiver-fix-probe
```

These are synthetic precision measurements, not production completion rates or
latency results. No deployment or production PR re-probe occurred. Sentinel can
use the updated binary without a protocol change; the same-PR production probe
is still needed to measure how often these receiver shapes occur in practice.

## Whole-side cache and soak diagnostics — 2026-11-05

The reported metadata-service and Nexus priming misses do not establish that an
`incomplete` impact verdict prevents caching. The eligibility policy on
`48e98163` and `9289e1d` is the same: unresolved interface calls and
inventory-backed deterministic type-check failures can be cached; failed
discovery and missing input inventories cannot. This change preserves that
policy and adds opt-in cache decision traces to `GRAPHICLE_PHASE_TIMINGS=1`.

The confirmed benchmark bug was that the second priming response was validated
before being saved. A failed cache assertion therefore discarded the most useful
response and stopped later repositories from running. The benchmark now saves
both responses and logs before validation, records the failure, runs the next
case, and exits nonzero with `complete: false`. Warm-cache, pin, packet-equality
and budget assertions remain enforced.

For a failed case, retain its `server/prime-request.json`, `prime.json`,
`prime-verify.json`, `prime.log`, `prime-verify.log`, `prime-failure.json` and
`server.log`, when available, together with `summary.json` and the captured
store. Match the base/head fact keys to lookup, eligibility and write events.
Pending writes only count as committed after `review_cache_commit`; that marker
does not imply every side was cacheable. See the cache diagnostics in
[pinned-review.md](../pinned-review.md#reuse-compiler-facts).

Validation uses `9289e1d` plus this working-tree change. The tested release
binary SHA-256 is
`cd83ea42fbd81cc9693bc6ee2fb1a8947fa4fc0a352f6523be7e87f0e9185eb4`.
The targeted Rust cache eligibility/persistence tests passed (3), as did the
service tests (2): same-store snapshot context and native-child deadline
cancellation followed by recovery and a warm hit.

All five soak regression tests passed (179.3 seconds locally):

- A 5,001-file capture uses multipart admission, primes a cold base, and
  completes ten CLI/server packet comparisons, including concurrent requests.
- A missing-dependency case deliberately fails priming, retains both responses
  and refusal diagnostics, and allows the next case to run. The following
  unresolved-interface case completes ten CLI/server comparisons with equal
  packets and `incomplete` impact throughout; its identical priming replay hits
  both whole-side caches. The overall benchmark correctly remains failed.
- Protocol checks preserve evidence on cache assertions, phase-log failures and
  transport errors, while retaining manifest, pin, cache-state and budget checks.

Reproduce the runtime checks with the candidate binary:

```sh
GRAPHICLE_BIN=/path/to/new/graphicle \
  python3 -m unittest discover -s crates/graphicle/tests -p test_review_soak.py -v
GRAPHICLE_BIN=/path/to/new/graphicle \
  python3 -m unittest discover -s crates/graphicle/tests -p test_review_service.py -v
```

The reported four-repository soak artifacts were unavailable locally. These
checks do not diagnose the actual metadata-service/Nexus misses or establish
their production memory and latency limits. Sentinel still needs to rerun those
cases and retain the diagnostics above; no production deployment was performed.

### Follow-up: four-repository soak on `16f44a2`

Sentinel supplied `cra/harness/runs/issue-fixes-20261025/indep-16f4.txt` after
the run completed. The summary was inspected locally: all four repositories
completed ten CLI/server packet comparisons each, with equal semantic packets,
pins and token caps preserved, no model calls, and unchanged queued jobs.
All four timeout checks stopped helpers, recovered on the same server and
returned equal recovered/warm packets. The one-second cooperative deadline
returned in 3.2–4.0 seconds; this is not a strict one-second response bound.

| Repository | Priming / replay seconds | Whole-side replay hits | Approx. sequential CLI / server seconds |
| --- | ---: | --- | ---: |
| argus | 32.0 / 5.0 | both | 17.4 / 10.3 |
| metadata-service | 37.9 / 22.8 | neither | 30.2 / 23.5 |
| nexus | 25.3 / 16.9 | neither | 23.0 / 16.5 |
| odin | 43.6 / 6.3 | both | 25.7 / 19.8 |

Sentinel reports stable fact keys and explicit cache refusal: eight
`incomplete_discovery` issues for metadata-service; 221 type-check failures
plus 470 `other` issues for Nexus. The local summary does not contain those raw
diagnostic lines or the affected package details. Inventory-backed type-check
failures alone are allowed by the cache policy, so the Nexus count is not enough
to establish its refusal cause. A missing `vtprotobuf` tag remains unconfirmed.

Nexus's priming replay hits all 3,092 dependency package exports on both sides;
the whole-side miss does not mean all dependency type-checking is repeated.
Metadata-service reuses 1,473 of 1,474 base packages but only 182 of 1,474 head
packages. Discovery and owned-code analysis still run on whole-side misses.

Inspection found a Graphicle diagnostics error: the histogram recognized
`external_package_unavailable`, but the native helper emits
`unavailable_external_package`. The latter was incorrectly grouped under
`other`. The follow-up corrects this spelling and also recognizes native
`package_discovery_failed`, `unsafe_or_invalid_module_configuration` and
`unsupported_architecture` issues. Unknown categories remain grouped as `other`;
package names and paths are not logged. This is an observability correction,
not a cache-policy, build-profile or analysis fix. Raw priming artifacts are
still needed before attributing Nexus's 470 issues or changing its profile.
The existing packet already exposes
`coverage.analysis_diagnostics.{base,head}.go.unavailable_external_package_instances`
and `package_failures` / diagnostic `examples`, independently of this stderr
histogram bug. Inspect those saved priming packets first; a full soak rerun is
not required merely to recover those counts.

The corrected release build passed the real-server missing-dependency regression
(74.2 seconds). Its refusal log names `unavailable_external_package` with no
`other` bucket; both repeated requests remain cache misses. The next,
cacheable-but-incomplete interface case still completes all ten equal CLI/server
packet comparisons. `git diff --check` also passed. Candidate binary SHA-256:
`98fd55587e9ccfb798219d75e9eb9df5aba2875f5c8fe33564dfc6e4a4c3c816`.

### Explicit template/build scope: first acceptance step

After reading the raw `graphicle-indep-16f4-20261105` packets, the
metadata-service refusal is confirmed as eight discovery errors per side, with
zero unavailable external packages or type-check failures. All five retained
package examples belong to the unrendered Cookiecutter tree; the two retained
messages report invalid `{` characters in import paths. Nexus's 470 `other`
issues per side are confirmed as `unavailable_external_package` instances. Its
saved packets omit all detailed diagnostic messages, so the missing imports
cannot be named from this capture. The `vtprotobuf` hypothesis remains unproven.

The template case can use existing explicit `modules` subdirectory selection;
no new runtime exclusion API or cache-eligibility exception is needed. A native
fixture admits a service, unchanged caller, imported owned helper, vendored
dependency, and unrendered template. Selecting the production subtree instead
of `.` removes its discovery error and permits whole-side replay reuse while
retaining all four modeled packages and unchanged caller context. The template
remains admitted, and outside-scope warnings keep the verdict incomplete.

[Captured fixture results](template-build-scope-20261105.json):

| Measurement | Whole-tree discovery | Explicit production subtree |
| --- | ---: | ---: |
| Discovery errors, base + head | 2 | 0 |
| Whole-side hits on identical replay | 0 / 2 | 2 / 2 |
| Unchanged caller context present | yes | yes |
| Semantic first/replay equality within that scope | yes | yes |
| Global impact / scoped coverage | incomplete / incomplete | incomplete / incomplete |
| Single local replay time | 0.738 s | 0.044 s |

These are single observations on a small synthetic fixture, not production
speedups. The scope itself changes graph identities, so packets from different
scopes are not expected to be byte-identical. The meaningful checks are retained
caller/dependency evidence, complete reporting of exclusions, and semantic
equality between cold/warm executions of the same scope.

The three real-server regressions in `test_review_build_scope.py` verify cache
reuse, a new owned-code edit reusing the base and dependency export, broadening
scope invalidating scoped facts, imported unresolved interfaces remaining
blocking, and edited templates remaining visible in the diff and file-change
summary with `uncertain_scope`. Template changes invalidate head facts rather
than reusing a stale analysis. They are not fabricated into compiler symbols.

Sentinel must choose all intended production roots from each pinned build
layout, including required consumers, and send them on both pins. Do not choose
only changed directories or automatically discard every directory named
`templates`. Recursive `.` still includes nested templates; root-level Go code
requires separate consideration. The existing `excluded_modules` field only
excludes real admitted module roots. This scoped path is an integration option
to validate on metadata-service, not a claim that its production run is fixed
or its excluded source is fully analyzed.

```sh
GRAPHICLE_BIN=/path/to/graphicle \
  python3 -m unittest discover -s crates/graphicle/tests -p test_review_build_scope.py -v
```

All three build-scope regression cases passed. The existing memory reliability
baseline also passed: 43 tests across `memory_handoffs`, `memory_symbols`,
`structured_memory`, and `retention`. These exercise exact original citations,
revision-checked transitive links, stale or denied source withholding,
symbol-level invalidation, repository and PR-scratch isolation, atomic dated
supersession, and incomplete evidence reporting when the context budget omits a
quote. Run from `crates/graphicle` with the repository build environment:

```sh
cargo test --release --test it --locked -- \
  memory_handoffs:: memory_symbols:: structured_memory:: retention::
```

This establishes existing local memory-graph invariants. It does not enable
reviewed memories in pinned `secondary_context`, certify production retrieval
recall, or implement a historical snapshot join. The remaining work is the
metadata-service integration probe, complete Nexus dependency admission,
package-cache miss/size attribution on a retained capture, and a revision-safe
reviewed-memory integration with fixed-budget context-value measurements.


### Package-cache attribution follow-up — 2026-11-05

The diagnostic build now connects `review_package_cache_load`, `_native`,
`_absorb` and `_save` to the base/head fact key. It reports stored/loaded export
counts and bytes, capacity and integrity refusals, native configuration hashes,
standard-library versus dependency hits, transport omissions, and committed-store
eviction counts. Save statistics remain provisional until `review_cache_commit`.
See [the cache contract](../pinned-review.md) for interpretation and bounds.

This closes the attribution gap; it does **not** establish which limit or
fingerprint caused metadata-service's production head misses. The original
stores were deleted. Whole-side eligibility, source admission, cache limits and
packet schema are unchanged. The helper identity changes on upgrade, so prime
the new build before evaluating warm reuse.

Local release binary SHA-256:
`c603d412513bdb97356d31896a211464be63e61127733f43d9300fd5e5a08747`.
This identifies the tested working-tree binary, not a published release.

Validation:

- Runtime package-cache unit tests: all four passed; the manual persistence
  benchmark remained intentionally ignored. Covers load/pool capacity counters,
  checksum/access refusals, repaired or evicted rows, retention and eviction.
- Full native Go suite: passed, including export equivalence, corrupt-export
  recovery, capacity and transport diagnostics.
- `test_review_cache_diagnostics.py`: passed. Verifies cold/warm/fresh semantic
  equality, base/head key correlation, persistent dependency reuse on a new head,
  build-profile and module-metadata invalidation, real missing-source refusal
  under `unavailable_external_package`, token limits and zero model calls.
- `test_review_build_scope.py`: all three passed on the diagnostic binary.
- `test_review_service.py`: both passed, including timeout cleanup/recovery and
  context retrieval bound to the admitted snapshot.
- Soak protocol unit checks: all three passed.
- Both stderr-filter tests passed in an isolated harness compiled directly from
  the current source with the release's serde dependency. The graph crate's
  standalone Cargo test command could not fetch its additional dev dependencies
  into the restricted Cargo cache; its full test suite was not run here.

To reproduce the server diagnostics check:

```sh
GRAPHICLE_BIN=/path/to/graphicle \
  python3 -m unittest discover -s crates/graphicle/tests -p test_review_cache_diagnostics.py -v
```

The production follow-up is a retained-store cold/replay/new-head probe with
`GRAPHICLE_PHASE_TIMINGS=1`. Preserve requests, packets and full stderr together
with the store. Compare load/refusal/eviction counts and native identities before
changing cache policy. Separately validate explicit production roots on
metadata-service and the complete pinned dependency closure on Nexus; neither
production completeness nor a speedup is established by these synthetic tests.


### Store-identity-dependent packing — 2026-11-05

The supplied `graphicle-prodprobe2-16f4-20261105` capture contains 19 packets,
but no saved `graph.impact` requests. In the five completed pass-2 pairs,
base/head pins match after mapping the repository IDs, and build profiles are
identical. All 25 common selected entries match after removing only entry ID,
revision, repository ID, the two package-binding fingerprints, and review-basis
scope. Source text, content hashes, symbols and reasons still compare exactly.

The differences are packing effects: Argus's CLI packet includes `startup.sh`
while its server packet does not. In the copied Odin packets both sides have
eight entries, with seven in common: CLI chooses `PodResolver`, while server
chooses `NullAdminAuditAction`. The recorded first-entry costs on Argus are
797 versus 820. Store-local repository identity enters package bindings and
entry/review revisions; cl100k assigns different costs to their hex spellings.
The entry ID's second charge reserves its occurrence in `context_order`, so it
must not simply be removed as accidental double counting.

The fix uses `fixed_hex_identifiers_v1` selection accounting throughout code,
metadata, diagnostic samples, history, flows and shared provenance. It keeps
real IDs, receipts and exact wire-budget checks. Entry/flow ties now use stable
source/symbol coordinates rather than store-bound identifiers. Direct pinned
content hashes retain their usual cost; conservative opaque-identifier reserves
can reduce what fits in a tight budget. Actual wire token totals and binding
hashes may still differ across stores.

[Machine-readable comparison and validation](store-identity-packing-20261105.json)
records the packet hashes and tested candidate binary. Re-costing the final
first selected entry (without its post-selection `update` field) gives equal
fixed costs in all five pairs. These re-costed values are not reconstructed
historical `first_entry_tokens`: the full preselection candidate inventory and
requests were not saved.

Validation: the new separate-store regression fails on the prior binary (a hunk
fits in only one store), then passes on the candidate at four budgets. Both
stores also produce canonically equal CLI/server packets when compared within
the same store. All 85 review unit tests pass; two manual benchmarks remain
ignored. The three build-scope server regressions and cache-diagnostics server
regression also pass. No deployment or live setting was changed.

For the next Sentinel probe, run the same candidate binary on both paths and
save the exact request arguments before dispatch, along with packets and store
identities. Compare selected source coordinates, content, verdicts and coverage
under verified logical pin mappings; do not erase source/content hashes to make
packets compare equal. Expect namespaced IDs and actual token totals to differ.
Recheck context yield at the production budgets, and handle Sentinel's timeout
allowance (#692) separately. The existing capture supports this diagnosis but
cannot substitute for that end-to-end candidate probe.
