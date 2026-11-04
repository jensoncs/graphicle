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
