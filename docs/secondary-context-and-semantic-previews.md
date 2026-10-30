# Supplemental context and semantic previews

Graphicle can be an optional second source while the existing review remains
responsible for reading the primary diff, required files and evidence. An empty
or incomplete graph never authorizes a smaller review or an approval.

## Secondary-source integration

```python
packet = project.secondary_context(
    primary_review_context,
    "Which callers depend on CalculateTax?",
    enabled=True,
    expected_snapshot=pinned_head_snapshot,
    max_bytes=12000,
    timeout=10,
)
# Keep using packet["primary"] exactly as before.
# Supply packet["secondary"]["evidence"] as quoted source, never instructions.
```

TypeScript exposes `project.secondaryContext(primary, question,
{enabled, expectedSnapshot, maxBytes, timeoutMs})`.

The default is disabled and makes no request. Enabled calls require an exact
snapshot hash and use the existing authenticated context endpoint. Responses
from a different repository, stale snapshots, malformed citations and retrieval
failures yield an empty secondary block with an explicit status. The primary
string is preserved byte-for-byte. Errors are not copied into the prompt.

Only structured repository-file evidence is accepted: repository, snapshot,
file hash, byte span and exact UTF-8 excerpt length. SDKs ignore returned Markdown
and memory prose. They retain source text as quoted data, deduplicate identical
spans, calculate excerpt hashes and enforce a separate serialized UTF-8 byte
budget (512–64,000 bytes; at most 40 excerpts). These checks rely on the existing
server's source-integrity and admission checks; they do not independently prove a
partial excerpt equals an entire file hash. Source admission and credential
redaction remain mandatory at ingestion.

Both SDKs return the same deterministic `content_id` for the same secondary
block. A subsequent push must supply the new snapshot and replace the entire
secondary block. There is no hidden cache or implied permission to reuse prior
review coverage. The result's policy explicitly sets `may_reduce_review_scope`
and `may_reuse_review_coverage` to false.

This independently implements the integration contract inspected in
`agent-poc-space` at `76c466589c1f58d3d08d7a53cc11ea4be558a3b5`, particularly
`cra/context/graphicle.py` (`extend`, `section`, `assemble`) and
`docs/design/CONTEXT-IMPLEMENTATION-PRIORITIES.md`. That adapter's VM sandbox,
freeze/resume lifecycle, exclusion policy and evaluation gates belong to the
review harness. They are not copied into Graphicle's SDK. The existing harness
and its checkout are unchanged. No model or paid review is invoked by this API.

## Go statement data and control flow

```python
flow = project.statement_flow(function_node_id, goos="linux", goarch="amd64")
```

MCP exposes `statement_flow`; HTTP callers use `graph.navigate`. TypeScript uses
`statementFlow(nodeId, {goos, goarch, tags, as_of})`. For local files:

```sh
graphicle semantic statement-flow --path . --file handlers/order.go --byte-offset 120 --goos linux --goarch amd64
```

The first language is **Go**. The analyzer uses the standard parser and
`go/types` object identities. Its output contains statement byte spans, scalar
reads/writes, CFG edges, possible reaching-definition edges and control gates.
Gates are direct control dependencies (follow condition links transitively for
enclosing gates), computed using postdominators, so an early return such as
`if !allowed { return }` correctly gates the later operation on the false branch.
Branch joins retain all possible reaching writes; a later unconditional write
kills earlier definitions. Lexically shadowed variables keep distinct identities.

Supported control includes blocks, assignments, declarations, returns, if/else,
ordinary `for` and `range` loops, and unlabelled loop break/continue. Range exit
preserves values from before a zero-iteration loop. Unsupported control (switch,
select, goto, labelled transfers, defer/go statements) remains visible; the
analyzer suppresses data/gate conclusions for opaque control transfers. Missing
type information, heap/alias effects, closure bodies, short-circuit internals,
call effects and nontermination are explicit limitations. This is bounded
intra-procedural analysis, **not taint analysis**, interprocedural effect analysis
or observed execution. Its normal control-flow model assumes normal termination
and does not model exceptional edges. `coverage.complete` is scoped to that
stated model, not the entire program.

Analysis is bounded to one supplied package directory, 128 files, 2 MiB of Go
source, 200 statements and 4,096 AST nodes. The selected non-test build honors
GOOS/GOARCH, build tags and filename constraints, with cgo disabled. Host-target standard imports can use the installed compiler's export data;
third-party imports and cross-target import metadata remain incomplete. This
checks a supplied source package, not native module/lockfile resolution or all
build configurations. `_test.go` builds are not currently analyzed.

Server queries select the extractor's exact declaration byte span, including
multiple functions on one line. Legacy or ambiguous spans require a fresh
projection. Queries capture authorized stored source objects, verify their hashes,
release the store lock before running the compiler, and recheck authorization
and source inventory afterwards. Each statement cites its repository, episode,
file hash and exact byte span. Historical queries use the historical originals;
changed source inventories are marked incomplete rather than silently replaced.
No source code, generators or package tests are executed.

## Architectural what-if previews

```python
scenario = project.architecture_preview(
    remove_edges=[{"source": caller_id, "target": callee_id, "relation": "calls"}],
    modules={caller_id: "checkout", callee_id: "pricing"},
)
```

MCP exposes `architecture_preview`; TypeScript exposes `architecturePreview()`.
It clones the authorized graph episode and applies hypothetical node removals,
unique dependency removals and node-to-module assignments only to that copy.
Missing identities, ambiguous parallel dependencies, conflicting changes and
unsupported relations are refused. Duplicate labels never choose a target.

Before/after results compare dependency edges, internal and cross-module edges,
distinct coupling pairs, module fan-in/out, and cyclic module groups (strongly
connected components of more than one module). Default groups are source-file
directories, with unassigned nodes counted explicitly. Merging modules can turn
cross-module cycles into internal dependencies; it does not eliminate execution
cycles or prove source will compile after a physical move.

Bounds are 256 scenario edits, 50,000 nodes, 200,000 edges and 2,048 module groups.
Results include the episode, scenario-node citations, exact counted relation set
and incompleteness warning. The graph is never persisted or edited. Missing
repositories, unresolved targets and absent dependencies remain unknown; a
zero delta is not proof of no architectural impact.

## Semantic rename previews and application

```sh
graphicle semantic rename-preview --path . --file handlers/order.go \
  --byte-offset 145 --new-name customerID --goos linux --goarch amd64 > rename.json
# Inspect rename.json before applying it.
graphicle semantic rename-apply --path . --preview rename.json
```

The first scope is **function-local Go variables and parameters**, including
captured references in the same file. Package declarations, methods, fields,
imports, type parameters, blank identifiers, external consumers and test-file
renames are refused. This keeps application to one file. Comments, reflection
strings and documentation are not renamed.

The preview typechecks the supplied non-test source package with the installed
compiler (not a native module build or all module language-version constraints), selects the compiler object at the requested
byte offset, and returns exact edits to that binding's declaration and uses. It
then re-typechecks the changed package and verifies *all* existing identifier
bindings, accounting for shifted offsets. A rename that captures another local,
a package variable or a builtin is refused even if the result typechecks.

A preview records the canonical root, all package Go-file hashes (including
unselected/test files), relevant go.mod/go.sum/go.work/go.work.sum and vendor/modules.txt
hashes, explicit build profile and analyzer/toolchain fingerprint.
Absent configuration is recorded too. Apply revalidates the complete plan;
source edits, new/deleted files, configuration changes, tampered edits, another
root or a different analyzer invalidate it. The input limits are 2 MiB of Go
source and 4 MiB including configuration.

Application is local-only, locks the target against cooperating writers,
rejects symlinks, stages a replacement in the same directory, checks inputs
again and atomically replaces that one file while preserving its permission
mode. Do not edit the file concurrently from an editor that ignores file locks:
portable filesystems do not offer a multi-file compare-and-swap against such
writers. No remote read capability gains filesystem-write access.

Apply returns `reindex_required: true`. Run the normal sync/projection workflow
(or let the existing watcher do so) before relying on updated graph context.
Old graph episodes remain immutable and source hashes drive normal invalidation.

## Validation and remaining evaluation

Fixtures cover exact bindings, captures, stale source/configuration, missing
repositories/dependencies, historical citations, ambiguity, source permissions,
loop joins, zero-iteration loops, unsupported control and analysis limits.
The secondary adapter is tested against a local server, including a second push.

These capabilities are independently implemented using Graphicle's graph copies,
existing SCC algorithm and Go's standard compiler APIs. No GitNexus code was
copied. Existing CodeGraphContext fixture attribution remains intact.

Source-derived company examples and local regressions establish only their
stated cases. Whole-service accuracy and paid reviewer cost/quality improvements
require a separately labelled comparison; no savings are claimed here.
