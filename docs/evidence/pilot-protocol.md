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
