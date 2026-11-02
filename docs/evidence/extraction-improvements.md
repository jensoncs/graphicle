# Extraction improvements: consolidated results

The extraction gaps identified in the Floorplan comparison now have bounded
implementations in Graphicle's existing AST route pass. Visualization is outside
this change. No Floorplan implementation or tests were copied.

| Improvement | Before | After | What was measured |
| --- | ---: | ---: | --- |
| Fetch/Axios methods | 1 / 12 | 12 / 12 | Correct cross-repository links |
| Immutable URLs and local request helpers | 1 / 11 | 11 / 11 | Correct cross-repository links |
| Next.js App/Pages endpoints | 0 / 5 | 5 / 5 | Endpoint facts |
| Spring mappings | 0 / 6 | 6 / 6 | Endpoint facts |
| Actix/Axum endpoints | 0 / 5 | 5 / 5 | Endpoint facts |
| Firebase functions, triggers and callable clients | 0 / 7 | 7 / 7 | Boundary facts |
| Incorrect local HTTP links in method controls | 8 | 0 | False links |

No unexpected cross-repository links occurred in either HTTP benchmark, and no
unexpected facts occurred in the framework inventory benchmark. The three fixed
acceptance sets contain **90 synthetic cases**: 22 method cases, 29 URL/helper
cases, and 39 framework cases. These are development fixtures, not held-out or
production recall measurements. Runtime speed, memory use and deployed service
wiring were not benchmarked. The rows use successive baselines: each improvement
starts from the preceding working tree, not from one shared baseline commit.

Receipts and earlier scope details:

- [HTTP method measurement](http-method-extraction.md) and [per-case JSON](http-method-extraction.json).
- [URL/helper measurement](http-url-extraction.md) and [per-case JSON](http-url-extraction.json).
- [Framework per-case JSON](framework-extraction.json), including the baseline
  source hashes and final source/test hashes. The framework baseline includes
  both earlier uncommitted improvements, at AST cache schema 19.

## What the remaining changes do

**Next.js:** discovers exported App Router HTTP handlers and Pages API default
handlers from an admitted `package.json` declaring Next. Supports named export
aliases, anonymous Pages default handlers, route groups, dynamic segments,
required and optional terminal catchalls, and `src/` layouts. Private directories,
intercepting/parallel paths, pages/layouts, re-exports from other files, type-only
exports and mutated handlers do not produce endpoints. A nearest non-Next package
blocks inheriting the parent's identity. Manifests outside the admitted input are
not read. Manifest changes trigger a full route rebuild, even when the previous
graph contained no routes; enabling, disabling and deleting the dependency are
covered by incremental tests.

**Spring:** recognizes actual Spring controller and mapping annotations through
explicit imports, a single unambiguous wildcard import, or qualified names.
Combines class paths with method paths, expands literal path/method arrays, and
links the actual method declaration. Dynamic annotation values, custom
lookalikes, multiple mapping annotations and unsupported request conditions
(headers, parameters, consumes/produces) abstain.

**Actix/Axum:** recognizes qualified/imported Actix verb attributes and Axum
`Router::new().route(...)` chains with imported/qualified routing functions.
Method chains retain the corresponding handler per verb. Supports Rust import
aliases/groups and Axum catchall normalization. Local shadows and lookalike APIs
abstain. Known prefix-changing/unknown enclosing builders and same-file Actix
scopes are conservatively omitted rather than exposing their inner path as a
complete served path.

**Firebase:** inventories exported ESM functions and triggers from proven SDK
imports, including aliases and namespace imports. Supports v1/v2 callable and
HTTP functions, v2 schedule/Firestore/PubSub/database/storage triggers, and v1
schedule/topic/document/ref/object/auth builders. Static resources are retained
from supported literals/options. Imported `httpsCallable` calls with literal
names produce separate client facts. A function needs a supported callback and
an export; shadowed, reassigned and type-only bindings abstain. Trigger kind edits
and removing an export replace/remove stale facts in incremental extraction.

The new adapters reuse parsed trees and the existing route nodes, handler
resolution and HTTP linker. Cache schema **20** invalidates older projections.
Firebase uses separate `cloud_function`, `cloud_trigger` and `cloud_call` node
kinds, with `deployment_identity: unresolved`. These nodes cannot enter HTTP
matching accidentally.

## Multi-repository behavior and limits

Full native extraction, with no seeded symbols, finds and links actual handlers
for all four HTTP framework families. A separate memory-crate test feeds these
graphs through the real workspace exporter/linker and verifies one route and one
handler edge per family. Duplicate destinations and absolute URLs without a
proven matching origin produce no cross-repository link. All heuristic HTTP
links remain `INFERRED`.

Endpoint declarations do not prove that an application is deployed, mounted,
enabled by runtime configuration or reachable at a particular host. This change
does not evaluate Next configuration/rewrites, custom framework conventions,
Spring meta-annotations/inheritance, arbitrary Rust builders/macros/cross-file
mounts, or Firebase CommonJS exports and arbitrary fluent wrappers. Firebase
project/region identity and event/callable wiring remain unresolved; matching
function or topic names alone never creates service edges. Use the existing
[configured service-boundary evidence](../service-boundaries.md) for pinned
HTTP/gRPC/event relationships. Existing package/version-aware import linking is
unchanged.

## Verification

Run the framework and incremental checks with:

```sh
cd crates/graphicle-graph
cargo test --locked --no-default-features --lib extractors::routes::
cargo test --locked --no-default-features --test framework_routes --test routes_incremental -- --nocapture
cd ../graphicle-memory
cargo test --locked --no-default-features --test http_method_extraction --test http_url_extraction --test framework_http_extraction -- --nocapture
```

These focused checks comprise 34 passing tests: 23 route unit tests, three
framework integration tests, five incremental tests, and three memory acceptance
tests. Additional regression fixtures cover optional catchalls, anonymous
handlers, long annotation spans, malformed/shadowed/unsupported declarations,
Firebase v1 triggers, resource options, and package admission boundaries; these
are not added retrospectively to the fixed before/after inventory score.

The broader engine library suite also passes: **340 passed, three ignored**, with
`--no-default-features --features grammar-objc,grammar-elixir,grammar-commonlisp`.
Those grammar features are necessary for the existing multilingual library
tests; disabling them leaves the Objective-C/member and Common Lisp tests unable
to extract their fixtures. The final native run includes the three framework and
five incremental tests as well. Together with the three memory tests, this is
**351 passing tests and three ignored**; the 23 route unit tests are already part
of the library total, not additional tests.

The recorded environment is macOS ARM64, Rust 1.94.1, locked dependencies,
`--no-default-features`, and macOS SDK 15.4. Memory tests use the prepared
JavaScript/TypeScript/TSX parser bundle. Both crates' locked tree-sitter versions
are exercised.

Changed Rust files pass rustfmt and `git diff --check`. Full repository CI is
not claimed: strict engine Clippy still reports the four pre-existing library
findings in `helm/expressions.rs`, `helm.rs` and `routes/go.rs`; no new library
findings remain. Whole-repository formatting, the all-feature test gate and
`cargo-deny` retain the limitations recorded in the earlier measurements.
