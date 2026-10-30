# Structured symbols, function values and static execution paths

`search_symbols` and `execution_flows` are read-only navigation tools on the
existing MCP server. HTTP callers use `graph.navigate`; they inherit project
permissions and the `as_of` episode selector. Python exposes `search_symbols()`
and `execution_flows()`; TypeScript exposes `searchSymbols()` and
`executionFlows()`. No model or network calls are involved in these operations.

## Structured declaration search

```python
matches = project.search_symbols(parameter_name="tenant", parameter_type="Tenant")
decorated = project.search_symbols(decorator="router.post", language="python")
annotated = project.search_symbols(annotation="Override", language="java")
flows = project.execution_flows(matches["matches"][0]["node_id"], max_depth=8)
```

Filters are exact, case-sensitive source spellings, ANDed together. Parameter
name and type must match the same parameter. Types are not resolved through
aliases or assignability. `router.post(...)` is indexed as `router.post`.
Body mentions do not become declaration metadata. Overloads retain separate
metadata records even when an older extractor shares their graph node.

The existing extraction tree supplies the metadata; queries do not reparse code.
Supported fields:

| Language | Parameter names | Parameter types | Decorators | Annotations |
| --- | --- | --- | --- | --- |
| Python | Yes | Written annotations | Yes | Use parameter type |
| JavaScript | Yes | Not available | Syntax only | Not available |
| TypeScript/TSX | Yes | Written annotations | Yes | Use parameter type |
| Go | Yes, including grouped names | Written types | Not applicable | Not applicable |
| Java | Yes | Written types | Not applicable | Method annotations |

Other languages and declarations missing metadata are reported as uncovered.
Destructured parameters, syntax failures and metadata limits remain partial.
Arrows/closures without existing declaration records are not synthesized by
search. Coverage is per field and language; empty results do not establish
absence in uncovered code. Limits: 128 parameters per declaration, 256 bytes per
searchable spelling, 1,024 header nodes, 100 search results. Omitted matches are
counted. Old episodes without metadata report the gap. AST/resolution cache
versions changed so subsequent projection re-extracts metadata; stable symbol
identities and memory-link revisions retain their existing source rules.

Matches identify their graph node and exact declaration/parameter byte ranges.
`nodes` contains the existing checked source citations, revision and stable ID.
Historical results mark changed/deleted current sources unchecked and retain the
episode they describe. Search never crosses into an unauthorized repository.

## Function values and callbacks

The Go compiler helper now shares function-value analysis between extraction and
pinned review. Existing single-initializer local aliases continue to resolve as
static calls. Additional constraints track local assignments/reassignments,
arguments to directly identified same-package functions (including forwarding),
and function values in local keyed struct/map literals and constant array slots.
The analysis uses compiler object identities, not same-named variables or fields.

These additional targets are **candidates**, expressed as `may_call`. The original
unresolved invocation remains present, including when only one candidate is
known. Unknown callers, mutable storage and escaping values prevent claiming a
closed target set. Branches and different invocations are not correlated; multiple
writes can retain candidates that are not executed at a particular call. Unknown
map keys, return-value factories, arbitrary heap aliasing, variadic callback
forwarding and interface dispatch are not newly resolved. Dynamic targets and
incomplete packages remain unresolved. Limits bound recursion, visited expressions
and candidate count; exhaustion never turns into a resolved call.

`may_call` participates in reverse impact traversal and potential flows, and carries
function-value evidence. Function references (`references_callable`) remain a
separate dependency relationship, not an invocation. The Go analyzer identity
changes and its source fingerprint invalidates old compiler facts. Python and
JS/TS retain their existing opt-in precise analyzers; this change does not claim
new callback coverage for other languages.

## Ordered flow results

Navigation's `execution_flows` starts from an exact `node_id`; duplicate labels
never choose a first match. It returns ordered `node_ids`, step relationships and
source evidence, plus checked episode citations. Only invocation relations are
followed, not imports, type uses or mere references to function values. Limits
are 16 edges per path, 32 returned paths and 2,048 expansions. Cycles, missing
nodes, depth truncation and path-budget omissions remain explicit. An empty flow
set never certifies that no execution is possible.

Pinned review marks invocation-only reasons with `execution_flow`; their `path`
and `steps` are in caller-to-callee order, including incoming impact paths.
Compiler steps cite repository, commit, snapshot, admission policy, source hash
and byte range. Configured service steps keep their original contract and
configuration provenance. Mixed dependency paths involving type/value references
remain dependency reasons without the execution-flow label.

The packet's `execution_flows.paths` composes selected incoming/outgoing invocation
segments into paths across HTTP, gRPC and events. Its steps reference selected
entries' reason/step indexes, avoiding a second copy of source evidence. Compose
only after materializing a delta with the SDK helper. Composition uses remaining
packet space reserved after required head code, hunks and requested business
history (at most 1/16 of the budget, capped at 2,048 tokens). The allowance covers
both path references and their citation upgrades; only complete paths are
admitted. Unused reservations are released for history on selected code.
Composition honors review depth, bounds candidates and reports omissions,
including `reserved_tokens` and `used_tokens`. No hidden source or inaccessible pin is consulted. Normal
entry revisions, receipt invalidations and full/delta selection apply to flow
changes. This is static configured service wiring; automatic production discovery
and observed telemetry remain separate work.

Every flow is `kind: potential_static`, `observed: false`. Paths do not establish
branch feasibility, temporal ordering, delivery guarantees or a runtime trace.
Global graph incompleteness, unresolved calls and selection limits remain visible.

## Reference and evaluation policy

The structured-search exclusion fixture adapts CodeGraphContext's
`tests/unit/tools/test_find_functions_by_argument.py`,
`test_argument_search_filters_functions_on_a_real_graph`, at commit
`93c9685bff9151f50a3121e9913e5ba2d27c1191`, under MIT; its license
is retained in `crates/graphicle/licenses/CodeGraphContext-MIT.txt`. Graphicle uses
its own stored extraction metadata and authorization model, not CGC's database
query implementation. The flow and callback code are independently implemented;
no GitNexus implementation was copied (its checkout is PolyForm Noncommercial).

Regression checks measure expected target sets, false-positive exclusions,
ordered paths, source freshness, authorization, and packet invalidation—not just
edge counts. Synthetic fixture success alone does not establish production
accuracy or reviewer cost savings. Representative service labels and actual
review comparisons must be reported separately.
