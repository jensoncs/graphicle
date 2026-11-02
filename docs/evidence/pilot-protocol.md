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
