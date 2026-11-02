# HTTP method extraction acceptance measurement

This change improves the existing AST route/request extractor. It does not add
another resolver or change workspace HTTP matching rules. The implementation is
independent; no Floorplan implementation or tests were copied.

| Metric | Before | After |
| --- | ---: | ---: |
| Correct cross-repository endpoint links | 1 / 12 | 12 / 12 |
| Recall on the 12 positive cases | 8.3% | 100% |
| False cross-repository endpoint links | 0 | 0 |
| Incorrect local links in negative controls | 8 | 0 |

These are **synthetic acceptance cases, not a held-out benchmark or a production
recall estimate**. No latency, token savings or runtime traffic was measured.
The same 22 cases ran before and after the implementation. The baseline is the
commit recorded in [the per-case receipt](http-method-extraction.json), with the
new acceptance test added. Its assertion fails before the fix and passes after.

## What changed

`fetch(url)` and literal options without a method produce GET request facts.
Literal methods are read from `fetch(url, options)`, `axios(url, options)` and
`axios({url, method})`. Duplicate object keys follow the last-write rule.
Fetch's standard method normalization is preserved: lowercase `delete` becomes
DELETE, while lowercase `patch` is not incorrectly treated as PATCH.

Dynamic methods/options, spreads, computed or escaped option keys, prototype
overrides and method getters remain unresolved. Local parameter/declaration
shadows of the client names are excluded. Unknown methods no longer behave as
wildcards in local matching. The AST cache schema advances from 17 to 18 so
normal projection refreshes rebuild old facts and workspace links.

Existing limits remain: request recognition uses client naming conventions;
runtime client defaults/interceptors and global rebinding are not analyzed.
Only the existing seven verbs (GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS) are
resolved by the new option reader. URL variables and wrapper propagation are
measured in the [subsequent URL experiment](http-url-extraction.md). New framework
routes and deployment identity discovery remain separate work. Route extraction
retains its existing enablement policy.

## Method and reproduction

The [test](../../crates/graphicle-memory/tests/http_method_extraction.rs) feeds
actual TypeScript source to the route extractor, indexes the server separately,
then runs `cross_repo_http::exports` and `cross_repo_http::link`. Caller/handler
declaration nodes are seeded, so this isolates HTTP extraction and linking from
symbol discovery. Expected targets are specified independently of the output.
Metrics count endpoint targets, not extra handler/middleware edges.

Twelve positive cases cover existing `axios.post`, default methods, literal
options, multiline calls, lowercase methods, quoted/duplicate keys and Axios
config objects. Ten negative cases cover dynamic inputs and duplicate backend
services. Local negative controls expose only one POST endpoint: an unknown
method must not link merely because that endpoint is the only path match.
Additional native regressions check escaped keys, PATCH casing, overwritten
methods/getters, config URL overrides and local shadows.

After preparing parser sources as described in the crate's contributing guide:

```sh
cd crates/graphicle-memory
cargo test --locked --no-default-features --test http_method_extraction -- --nocapture
```

The `HTTP_METHOD_EVALUATION=` line contains every expected/actual target and
aggregate count. The recorded run used Rust 1.94.1 on macOS ARM64 and only the
JavaScript/TypeScript/TSX grammar pack languages for this test. Rust dependencies
were built with `--locked`; no lockfiles or dependencies changed.

## Validation limits

Passed: the acceptance test, 18 route unit tests, two incremental route tests,
three workspace HTTP linker tests, formatting of changed Rust files, and
`git diff --check`.

Repository-wide formatting checks report existing differences in unchanged
files. Strict Clippy reports existing `collapsible_if` findings in the Helm,
Go-route and workspace HTTP modules, plus `ptr_arg` in Helm. No lint was reported
in the modified extraction code. Those unrelated files were not changed.

The all-target checks with `postgres,bedrock,transcription` were attempted
offline and stopped at an uncached optional dependency (`adler32`); the full
feature suite is **not verified** by this run. `cargo-deny` is not installed on
this host. These results do not replace the repository's full CI gates.
