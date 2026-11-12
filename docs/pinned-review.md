# Pinned change review

Core supports `graph.impact`, `impact.report`, the MCP tool `review_changes`,
and `graphicle review --request review.json`. All use the same request and
permission checks. For multi-repository MCP reviews, use a workspace connection
(`graphicle mcp --workspace`); project connections only admit their own repository
on both sides. Installed extensions retain ownership of operations they
explicitly override. Core also accepts revision-bound execution observations through `coverage.upload`.

The compiler-backed implementation analyzes **Go package calls in supplied immutable
snapshot sets**. It does not prove behavioral propagation or discover every
consumer in an organization. Explicit pinned HTTP/gRPC/event wiring is supported
through [service-boundary manifests](service-boundaries.md); it is not automatic
runtime service discovery. **Terraform, Helm, Bash, Python, Rust, TypeScript and
JavaScript also have basic file/module/chart-level impact** as described below. Other languages
remain available to ordinary graph retrieval, but their presence in a review
scope is reported as unsupported. An incomplete result must never be
interpreted as “no impact.”

With `risk_schema: "graphicle.review-risk.v2"`, `impact` describes the full
analysis of those supplied snapshots before response packing. A smaller token
budget does not change the verdict. Omissions are reported separately in
`coverage.budget_omitted`; `coverage.complete` and each entry's `review_basis`
remain conservative about missing context. Analysis and admission gaps still
produce `impact: "incomplete"`. See the [measurement contract](pilot-protocol.md#risk-measurements)
for v1/v2 comparison semantics.

Packets also include `analysis_coverage` with format
`graphicle.changed-go-coverage.v1`, computed before packing. It answers a narrower
question: is static analysis complete for the changed owned Go import components
and their resolved dependency APIs? It excludes dependency implementations and
callbacks, non-Go semantics, and runtime behavior. It can be `complete` while
global `impact` remains `incomplete` and delivered context is truncated.

Components include incoming and outgoing imports from both base and head. An
unresolved call in a changed component blocks completeness; a known disconnected
component is counted separately. Admission gaps, compiler failures, unknown
ownership and other unclassified limitations still block globally. Changed
unmapped inputs (including configuration, embeds and dependency implementation
files) block this assessment; profile-excluded/non-compiler inputs are excluded.
Dependency binding or module-layout changes are not certified by this v1 scope.
With no changed owned Go components and no blockers, status is `not_applicable`.

This field is always enabled and bounded; it does not grant review skipping or
alter the existing `review_basis` / `review_reuse` proof rules. Sentinel can record
it alongside v2 `impact` and delivery coverage using the
[scoped analysis contract](pilot-protocol.md#changed-go-analysis-coverage).

## Persistent review serving

Keep the HTTP process alive to reuse the compiled Go helper between reviews:

```sh
graphicle --data-dir /var/lib/graphicle-review serve --no-worker --port 8854
```

Call `graph.impact` or `impact.report` with the same pinned request as the CLI.
Source authorization, admission checks, analysis gaps and packet budgets are
unchanged. The first native Go review prepares the helper; later requests in
that process reuse it. Repository stores must also persist to reuse source and
compiler caches.

`--no-worker` disables the built-in workflow workers and their connector and
observation schedulers in this HTTP process. Existing jobs stay queued, and
`sync.commit` still queues projection work. Run `graphicle --data-dir
/var/lib/graphicle-review worker` separately when that work should execute, or
restart ordinary `serve` to resume it. Jobs that require a worker, including
model workflows, will wait until one is running. The flag does not cancel jobs,
disable authorization, restrict API operations, or suppress extension-owned
background tasks. Explicit graph-projection API calls still execute. Ordinary
`serve` and `open` retain their default worker behavior.

The Python SDK supports this through
`LocalServer(binary, store, config, serve_args=["--no-worker"])`. Keep that
context alive across requests. A process restart loses the helper cache but
retains persisted source/compiler caches. The stdio MCP startup drain is a
separate mechanism and is not controlled by this HTTP flag.

### Unix socket transport

On Unix, run the same HTTP API without opening a TCP listener:

```sh
mkdir -p /run/graphicle-review
chmod 700 /run/graphicle-review
graphicle --data-dir /var/lib/graphicle-review serve --no-worker \
  --socket /run/graphicle-review/review.sock
```

Create the directory as the service user. The socket path must be absolute and
its existing parent must be an owned, private directory, not a symlink. The
socket is mode 0600. Capabilities and repository authorization still apply.
`--socket` conflicts with explicit `--listen`, `--port` and `--open`. It does not
write the TCP discovery file `server.json`. A pre-existing path is refused;
normal shutdown removes only the socket created by this process. After SIGKILL,
confirm that the process is stopped before removing its stale socket.
`build.info.serve_unix_socket` reports platform support.

Use `Client("unix:///run/graphicle-review/review.sock", workspace, capability)`
or `LocalServer(binary, store, socket_path="/run/graphicle-review/review.sock",
serve_args=["--no-worker"])`. The SDK uses Unix HTTP directly, without a relay,
DNS lookup, proxy or TCP fallback. Non-SDK clients can use HTTP with Host
`localhost` (for example, curl's `--unix-socket`).

### Server-side review deadline

On Unix, set optional `timeout_ms` (integer 1–600000) on a pinned review request, or pass
it to `Project.impact`. `build.info.review_deadline` advertises
`cooperative-timeout-ms-v1` (null on other platforms, which reject the option).
The command and impact HTTP routes start the execution budget before entering
the blocking pool; CLI/MCP calls start it when the pinned review is dispatched.
HTTP parsing/admission and response delivery are outside this execution budget.
Waiting for Go helper initialization consumes the budget. Source loading, materialization and
analysis phases check it; native subprocesses and storage lock waits use the
remaining time. A helper compilation the deadline interrupted is retryable at once; one that
failed with budget left is retried after 10 minutes, and a helper compiled as the deadline
expired is kept for the next request.

On expiry the request returns HTTP 504 with `graphicle.error.v1`, code `timeout`,
after synchronous work has unwound, native subprocess groups have been
stopped, and direct children have been reaped. It does not turn cancellation into an incomplete packet.
The HTTP process and previously warmed caches remain usable. The deadline is
cooperative: an active CPU phase or filesystem operation may run past the limit
until its next check. It is not a hard real-time execution bound or a separate
cancel API. A timeout may occur at a transaction boundary, so callers should not
assume that every cache/receipt write was rolled back.

Keep the client transport timeout longer than the requested server deadline and
allow time for cleanup. A client disconnect or transport timeout alone still
does not prove that work stopped; wait for the structured server response, or
use the existing process-supervision recovery policy. With no `timeout_ms`, the
existing native operation limits apply.

### Context from the admitted review head

The existing `context` operation reads the project's current admitted files.
It does not need a second sync, graph projection, or a running worker. Reuse the
review project after admitting its head and restrict fallback retrieval to file
sources:

```python
from graphicle_sdk import Client, Project

client = Client("unix:///run/graphicle-review/review.sock", workspace, capability,
                timeout=150)
project = Project(client, repository_id)
packet = project.impact([base_pin], [head_pin], profile=profile,
                        max_tokens=8000, timeout_ms=120000)
fallback = project.secondary_context(primary_diff, question, enabled=True,
    expected_snapshot=head_pin["snapshot"], max_bytes=12000)
# Consume fallback["secondary"]["evidence"] only when status == "included".
```

`secondary_context` requests only `file:` evidence with memory retrieval disabled
and validates the returned repository, snapshot, hashes and byte spans. If another
admission advances the project before retrieval, it returns `stale_snapshot` with
no evidence. Direct `context` clients must compare the response's `snapshot`
against the review head themselves. This is current-head retrieval, not a query
against an arbitrary historical snapshot. Serialize admission/retrieval per
project when fallback context must be available for every head. Supplementary
context never grants review coverage or permission to skip review.

## Basic language support

Basic analysis is selected automatically for admitted sources in the existing
`graph.impact`, `impact.report`, `review_changes`, CLI and SDK review paths. No new
request flag is required. The existing `profile` object is still required by the
request contract; its Go settings apply only to Go analysis. Repositories with
only basic sources do not require a Go toolchain.

| Language | Basic impact scope | Explicit limitations |
| --- | --- | --- |
| Terraform | Changed `.tf`, `.tfvars`, JSON variants and lockfiles; conservative same-directory module scope; literal local module source dependencies | No plan/state, provider schemas, expression evaluation or remote modules |
| Helm | Changed chart files, values, templates and helpers; conservative nearest-chart scope and vendored subchart candidates | No rendering, release values, dependency-condition evaluation, cluster capabilities or remote charts |
| Bash | Changed scripts, declaration metadata, literal `source`, `.`, `bash` and `sh` path candidates | Working directory and PATH are not established; expansions, dynamic commands and runtime function binding remain unresolved |
| Python | Changed files, declaration metadata, local import candidates, literal env reads and nearest project-manifest scope | No runtime environment/package resolution, runtime dispatch, dynamic imports or rebinding resolution |
| Rust | Changed files, declaration metadata, local module/import candidates and nearest Cargo manifest scope | No Cargo dependency resolution, feature/cfg evaluation, macros, build scripts or trait dispatch |
| TypeScript / JavaScript | Changed `.ts`, `.tsx`, `.mts`, `.cts`, `.js`, `.jsx`, `.mjs`, `.cjs` files, declaration metadata, relative import candidates (extension and `index` probing, `.js` to `.ts`), `tsconfig.json`/`jsconfig.json` `paths`/`baseUrl` aliases, and nearest `package.json` (else tsconfig/jsconfig) scope, which also covers lockfiles | No `node_modules`, package `exports`, CommonJS `require`, dynamic `import()`, bundler aliases or runtime dispatch |

File entries carry `analysis_level: "basic_file"`, `language`, complete file
bytes and their existing hashes, ranges and snapshot provenance.
Python/Rust/Bash/TypeScript/JavaScript file entries also carry at most 128
declaration records with byte spans; omitted metadata is counted. A file's
signature/implementation change classification and test classification are
unknown (`null`), rather than being reported as unchanged or false.

For Python, Rust, TypeScript and JavaScript, each uniquely named declaration
(function, method, class, arrow-function constant) is also a node with
`analysis_level: "basic_symbol"`: its own byte range, and its header and body
digested apart, so a change reports `signature_changed` and
`implementation_changed`. A call the parser binds to one declaration, through a
visible declaration or an explicit import, is a `may_call` edge with evidence
`explicit_import_binding` or `unique_visible_declaration_candidate`. The callers
of a changed declaration are therefore required context and are counted in
`coverage.callers`; a call at module level is its file's. For TypeScript and
JavaScript, the oxc binder (as in `--precise`) also resolves modules over the
pinned snapshot's own files, with no `node_modules`, and adds `new` and method
calls on a receiver typed by an annotation (`oxc_type`) or made by `new`
(`oxc_binding`), inside functions and methods, in a repository with at most
64 MiB of TypeScript/JavaScript sources; a larger one is reported as
`precise_js_budget`. These are static bindings, not resolved runtime calls:
untyped receivers, JSX, callbacks and rebound names are not seen, so file
importers are still selected through `potential_import` and `configuration_scope` edges as optional context. A name
declared twice in one file is reported as `basic_duplicate_declaration` and has
no node; unbound calls are counted in `basic_unresolved_calls`. A
TypeScript/JavaScript file with a line over 10,000 bytes is generated output
(a minified bundle): it stays a file entry, so a change to it is still seen,
but has no declarations, imports or calls, and is reported as
`basic_minified_source`. Paths built from
these edges are `potential_static` candidates, never observed flows. Missing or ambiguous targets and parse/analysis limits are reported.
Basic analysis always reports `basic_analysis_limitations` and leaves
`coverage.complete=false`; it cannot certify no impact or permit skipped-review
coverage reuse. No scripts, project builds, Terraform plans or Helm rendering run.

Imports are matched only within an authorized pinned repository. Cross-repository
package resolution is not provided by this tier. Existing source admission,
permissions, packet budgets and delta invalidation continue to apply. Large files
may exceed a packet budget; an omission remains a coverage limitation, not a
successful complete review. Use `previous_packet_id` as usual for subsequent pushes.
The existing permission-safe receipt behavior is unchanged: if a previous packet
references an inaccessible repository, a delta returns a conflict requesting a
full packet; it never relaxes repository access to reconstruct the receipt.

`build.info.basic_change_impact` advertises this tier separately from the existing
Go compiler impact capability. These source changes require rebuilding the
executable before a running server can expose the new behavior.

## Admit source snapshots

Register a project per repository. Use the existing `sync.prepare`,
`sync.upload`, and `sync.commit` API with this `sync.prepare` metadata:

```json
{
  "commit": "<full lowercase Git commit ID>",
  "policy_sha256": "<SHA-256 of the source admission/exclusion policy>",
  "scope_complete": true
}
```

The collector must verify that the sources came from that commit and that all
relevant files were admitted. These fields are collector attestations, not
cryptographic proof of Git history. Graphicle verifies source bytes against the
immutable manifest and retains separate commit/policy bindings when different
commits have identical trees. Re-importing the same binding replaces its scope
attestation, including downgrades from complete to incomplete. An absent or
false `scope_complete` without an exhaustive omission list produces an explicit
coverage limitation. Upgrading it requires a new explicit collector attestation.

Collectors that withhold files can send an **exhaustive** list of exact
repository-relative paths in the initial `sync.prepare` metadata:

```json
{
  "commit": "<full lowercase Git commit ID>",
  "policy_sha256": "<SHA-256 of the source admission/exclusion policy>",
  "scope_complete": false,
  "withheld_paths": [".env.production", "tls/client.key", "private/credentials.json"]
}
```

The list includes every omitted file, not only credentials. Absent/null retains
the old blanket limitation; an empty array explicitly attests no omissions.
Paths must be unique and cannot overlap the admitted manifest. Traversal and
absolute paths are rejected. The list shares metadata's 64 KiB bound, with at
most 20,000 paths of at most 1,000 bytes each. A nonempty list conflicts with
`scope_complete:true`. Multipart sync retains the initial metadata and checks
overlap against every part.

For Go, known credential/documentation filenames can be ruled out as build inputs
only after checking all admitted packages' `go:embed` patterns, including directory
and wildcard matches. Character classes, escapes and invalid patterns retain
uncertainty rather than using a non-match as proof. `vendor/modules.txt` and compiler-source extensions are not
treated as documentation/credentials. Missing code, build metadata, unknown files
and potentially embedded assets remain incomplete. An apparently unrelated
withheld source file could add a consumer edge missing from the admitted graph;
Graphicle does not infer its irrelevance from that partial graph. Other languages
do not yet have this compiler-backed omission proof. This checks Go build inputs;
it does not prove runtime file reads or runtime configuration irrelevant.

`coverage.source_admission.base/head` reports per-binding `withheld_count`,
`outside_build_count`, `unresolved_count` and a digest of the sorted list, without
echoing the paths. The admission gap clears only when every listed omission can
be ruled out. Other analysis gaps remain. Lists are stored per exact
repository/snapshot/commit/policy binding, reassessed even on a compiler-cache hit,
and rechecked for concurrent updates before returning the packet. Old bindings
migrate with an unknown (null) list; an older writer replacing the scope
attestation invalidates any previously stored list. `build.info` advertises this contract as
`review_source_admission: "exhaustive-withheld-paths-go-build-v1"`.

Go discovery diagnostics distinguish `unavailable_external_package:<import path>`
from `incomplete_discovery:<local package>`. Dependency errors are attributed to
their owning package using Go's structured diagnostics, rather than marking every
importer as a discovery failure. Own-source discovery errors and actual type-check
failures remain incomplete. External code is still restricted to admitted
snapshots: merely placing it in a host module cache does not authorize Graphicle
to use it. This diagnostic fix alone cannot make an uncheckable build complete.

Error-only `go list` records with no package directory remain discovery failures;
they are not additionally counted as type-check failures or source outside the
snapshots. An importer that cannot type-check because of a missing dependency
still records its own type-check failure. Module configuration and symlink safety
checks cover the whole immutable pin set once, before any compiler world runs.

`coverage.analysis_diagnostics.<side>.go.package_failures` inventories failing
package instances by consumer build and import path. Each example has native
status codes and compiler diagnostic cause counts (`type_error`, `parse_error`,
`outside_snapshot`, `missing_package`, `import_cycle`, `discovery_error`, or
`unsupported_cgo`). Causes are collected before message sampling. The inventory
count, digest and `packages_by_cause` cover all failures; `examples_omitted`
explicitly counts packages whose details cannot fit the diagnostic allowance.
Increasing `max_tokens` allows more package details. Empty cause maps mean there
was no more specific native diagnostic; the status remains authoritative.
This inventory also includes discovery and unavailable-package failures that
never produced a compiler package instance. Its count can therefore exceed
`incomplete_package_instances`; a discovery failure is not proof that a package
body was type checked. Unrendered template paths can fail discovery even when
all application packages type check.

Graphicle consumes dependency source prepared by the collector; it does not run
`go mod download` or `go mod vendor`. Collector preparation deadlines and its
`vendor_failed`/`bound` statuses are distinct from Graphicle's compiler deadline.
`compiler_analysis_unavailable.detail` names the compiler stage and cause,
including `timeout` with `limit_seconds` (120), or `output_bound` with
`limit_bytes_per_stream` (64 MiB). It never forwards arbitrary process stderr.
Source admission errors name the corresponding `build.info.review_source_limits`
field, actual value and limit. Persistent compiler reuse requires a persistent
data directory and stable repository identities, as described below.

Existing source limits, permissions, upload validation, and compare-and-swap
sync checks apply. Retain both sides' source objects until review completes;
a missing or expired retained snapshot makes the review incomplete.

A verified [shared GCS bundle](shared-context.md) can supply these sources:

```sh
export GRAPHICLE_SHARED_CONTEXT_PREFIX=gs://code-review-transcripts/graphicle/
graphicle shared-context import --project tax --identity tax-base.identity.json \
  --snapshot '<artifact SHA-256>' --scope-complete
```

The prefix is a variable; use `--prefix gs://another-bucket/prefix/` to override
it. Import requires a short-lived `GRAPHICLE_GCS_ACCESS_TOKEN`, the exact
expected identity and object digest, and a compatible engine digest. It checks
expiry and source integrity before syncing. **Import changes the selected
project's current source snapshot** while retaining earlier immutable snapshots.
It imports source bytes only; review computes or reuses locally verified compiler facts. Omitting
`--scope-complete` imports the source with an incomplete admission attestation.
No bucket configuration, overwrite, public sharing, or lifecycle changes occur.
Local imported copies are governed by local repository permissions and retention;
GCS expiry/revocation does not retroactively delete an admitted local copy.

## Request a review

```json
{
  "repository_id": "<anchor project ID>",
  "base": [
    {
      "repository_id": "<project ID>",
      "directory": "tax",
      "commit": "<base commit>",
      "snapshot": "<base source manifest digest>",
      "policy_sha256": "<policy digest>",
      "modules": ["."]
    }
  ],
  "head": [
    {
      "repository_id": "<same project ID>",
      "directory": "tax",
      "commit": "<head commit>",
      "snapshot": "<head source manifest digest>",
      "policy_sha256": "<policy digest>",
      "modules": ["."]
    }
  ],
  "profile": {
    "goos": "linux",
    "goarch": "amd64",
    "tags": [],
    "tests": true,
    "workspace": ""
  },
  "max_depth": 8,
  "max_tokens": 16000
}
```

Include unchanged consumer/dependency repositories on both sides. `directory`
is a stable, single-component checkout name; local `replace` directives must
resolve within the admitted layout. `modules` lists module roots within each
checkout. A workspace file can be selected with, for example,
`"workspace":"checkout/go.work"`. The file, whatever its name, must be a regular
file inside a pinned checkout, and its `use` and `replace` paths must resolve
there too. Otherwise workspace discovery is off.

Optionally set `"business_context": {}` to attach conservative domain candidates,
or supply its `history` array from the local `review-history` collector. Historical
assertions are bound to exact pins and source hashes, share the packet budget,
and participate in delta invalidation. See [business context](business-context.md)
for collection, provenance limits and the review-value fixture.

Set `"compact_provenance": true` once in the review request when the consumer
supports service references. Graphicle automatically shares configured service
assertions when their repetition saves tokens across the candidate evidence.
Code-only reviews and small, unshared assertions keep the inline representation;
there is no per-service or per-language configuration. Omitting the option keeps
the existing inline response format. The option applies to `graph.impact`,
`impact.report`, MCP `review_changes`, CLI `review --request`, and both SDKs.
Consumers can discover support through
`build.info.review_compact_provenance == "graphicle.service-provenance.v1"`.
Omit the request field entirely when connecting to older servers that do not
advertise support.

Compact steps carry `provenance_ref` instead of `provenance`. Resolve that hash
in `service_provenance.definitions` (format `graphicle.service-provenance.v1`).
A cross-service definition can contain `source_ref` and `target_ref`, pointing to
leaf binding assertions in the same table. Keys are SHA-256 of compact UTF-8 JSON
with recursively sorted object keys. The full table needed by the selected view
is sent on every packet, including deltas with unchanged entries. Definitions
count toward `max_tokens`; changes to citations change references and revisions.
Pinned hashes, spans, authority and `runtime_observed=false` are preserved.
Boundary entries also use `service_boundary.provenance_ref` for their repeated
`citations` and `manifest`. Their identity, transport, authority and resolution
remain inline; SDK materialization restores the two citation fields and checks
the boundary identity against the referenced assertion.
When sharing is possible, the packer compares inline and compact selections:
it prefers more changed-head declarations, then callee contracts, then immediate
callers, then total selected entries/diffs, and finally lower canonical packing
cost. A tight budget can therefore automatically fall back to inline output.
This decision is independent of prior receipts and delta wire size.

Python `apply_review_delta` and TypeScript `applyReviewDelta` validate and expand
these references automatically. For full packets, `materialize_service_provenance`
and `materializeServiceProvenance` provide the same citation expansion. Expansion
retains reference IDs for later delta validation and removes the wire token count:
the expanded view is larger. Sentinel should retain the compact table in its
model prompt to keep the savings, and expand only for citation consumers that
need the inline view. An unresolved or corrupt reference is an error, never
evidence of low risk.

Packing reserves immediate current-head callers alongside changed declarations
and discarded-error callee contracts before optional history. `coverage.callers`
reports required/selected counts and completeness when such callers exist;
oversized callers still produce explicit omissions.

A local Go toolchain must be installed. Each consumer gets its own native
`go list`/`go/types` resolution world. Build tags, platform, module versions,
workspace/local replacements, and test variants determine the selected source
and target declaration. Modules and toolchains are **never downloaded during
review**; CGO is disabled. A pinned package whose file selection depends on cgo
(an `import "C"` file or a `cgo` build constraint) fails closed with
`unsupported_cgo`, because its cgo files are never analyzed. Provide pinned local replacements, a workspace, or
a vendor tree for the dependency source. An arbitrary compatible checkout is never substituted
for an installed version. Missing dependencies remain unresolved.

Review selects Go working directories from admitted `go.mod` files. The default
`modules:["."]` discovers nested modules, including repositories without a
top-level module; each nested module gets its own compiler invocation. Explicit
subdirectory scopes can use an admitted ancestor module and its vendor tree.
Discovery skips nested `vendor`, `testdata`, dot-prefixed and underscore-prefixed
directories unless explicitly selected, and respects the existing Helm-source
classification. Discovery is bounded to 64 roots per pinned repository.

A repository with no selected module does not run Go analysis. Ordinary Go files
without an admitted module retain a `go_source_without_module` limitation instead
of fabricated compiler failures; files outside explicitly selected module scopes
retain `go_source_outside_module_scope`. A `go.work` file alone is not a module.
`build.info.review_go_roots` advertises `admitted-module-discovery-v1`.

Collectors can set `excluded_modules:["feature-agent/evals/context/repos/odin"]`
on each pin to exclude admitted module roots (and their descendants) from root
discovery. Paths are relative to that pin, at most 64, and must name directories
with an admitted `go.mod` to take effect. These are explicit collector decisions;
Graphicle does not infer that every `eval` or `fixture` directory is irrelevant.
Sources remain admitted and available if an included package imports them.
`go_module_excluded` records the omitted analysis scope and retains incomplete
impact; it does not turn an intentionally partial analysis into a complete one.
Exclusions are part of the compiler-cache and review identity.

For an admitted repository containing unrendered code templates, collectors can
use the existing `modules` field to select the actual production package
subtrees. For example, a service contained in `service/` can use
`"modules":["service"]` on both pins. Go still uses the admitted ancestor
`go.mod`, vendor tree and imported owned packages outside that subtree. This
does not require a second upload or deleting templates from the snapshot.

Choose the full intended build scope, including consumers whose context is
required, rather than just the directories changed by the PR. Multiple
subdirectories can be selected. A directory selection is recursive: `"."`
still includes templates beneath the repository root. This is not an arbitrary
package-directory exclusion API; `excluded_modules` requires admitted module
roots as described above. A repository with root-level Go code may therefore
need a different explicit build layout before this approach applies.

This can eliminate template discovery failures and allow compiler-fact reuse,
but does not certify excluded source. `go_source_outside_module_scope` remains
visible, and impact/scoped coverage may remain incomplete. An edited template
remains in `file_changes` and eligible diffs with `uncertain_scope`; packing
limits still apply. Changing the selected roots changes cache identity, and
changed Go template bytes still invalidate the corresponding facts. Imported
packages retain their unresolved-call diagnostics. The regression in
`tests/test_review_build_scope.py` checks all of these boundaries, unchanged
caller context, and dependency-export reuse on a subsequent owned-code edit.

For a non-vendored repository, prepare each pinned revision separately with Go's
native vendoring in a disposable source copy before admission. An offline
preparation can use an explicit module cache with `GOPROXY=off`, `GOVCS=*:off`,
`GOTOOLCHAIN=local` and read-only module metadata. Verify cached modules, preserve
the original source and `go.mod`/`go.sum` hashes, and include `vendor/modules.txt`
and the dependency files in the admitted snapshot. Retain the source commit,
policy, build profile, toolchain and generated-file hashes as preparation evidence.
Populating the review server's ambient module cache does not admit dependencies.
Missing or policy-withheld source still requires an incomplete scope attestation.

Admitted vendored packages are compiler dependencies, not owned implementations.
Graphicle still discovers and type-checks their selected source and retains their
package/file inventory, but collects symbols, implementation calls and dynamic
candidate searches only from owned packages. First-party internal packages and
local replacements inside the pinned checkout set retain full analysis.
Compiler-resolved references to vendored and standard-library APIs carry a
boundary identity (package, module/version when available, symbol and signature)
instead of an `outside_pinned_scope` issue. Changes to dependency inputs still
invalidate compiler facts and conservatively select affected owned consumers.

This is API resolution, not proof of library effects or callback dispatch.
`dependency_effects_unmodeled` qualifies each consumer build using these
boundaries, including blank-imported vendored packages; coverage remains
incomplete. Owned dynamic calls, type failures and withheld input gaps remain
visible. `analysis_diagnostics.go` reports owned/dependency package instance
counts, resolved dependency references and bounded boundary examples. Source
admission checks and the per-side byte/file limits below still apply; this does
not introduce a dependency object store or compiler export-data cache.

Required binary compiler assets can be uploaded with `content_base64` instead of
`content` in `sync.upload`. Supply exactly one field. Manifest hashes and sizes
always describe decoded original bytes. The store retains binary objects as bytes;
compiler materialization and shared-bundle export/import preserve them exactly.
Text-only retrieval and excerpt generation exclude binary payloads. Existing text
snapshots and hashes remain compatible, including stores created before binary
input support. The usual upload, retained-source, authorization and deletion
limits apply; base64 does not bypass them.

The collector must attest each binary path and hash as a native selected embed.
Credential scanning still applies to the bytes. Arbitrary binaries are not admitted
by the evaluation harness. Changed binary inputs are listed in `file_changes`
with before/after hashes and `binary: true`, without a fabricated text diff;
unmodeled binary inputs and changed embeds keep impact coverage incomplete.
The shared-bundle wire-size limit includes base64 expansion.

Keep original test and platform-specific files in the source inventory; the build
profile selects them. A collector may trim newly generated vendor files to the
native selected Go, assembly, header and embedded inputs for one profile. Record
omitted paths without reading their contents, preserve every original file, and
re-run native discovery to verify the selected input set did not change. Required
files cannot be omitted to satisfy a source-policy check. Such preparation must
include `metadata.dependency_preparation.profile`; review reports
`dependency_preparation_profile_mismatch` and incomplete coverage for another
profile. Prepare a new dependency closure before changing platforms, tags or tests.

Source-aware admission may distinguish Go symbolic references from credential
values using parsed declaration and literal spans. Preserve the original bytes
and record the classifier identity, file hashes and classification counts. Keep
provider-token, private-key and decoded-literal checks; failure to prove a
classification must still refuse admission. An empty package selection has an explicit empty file list, while
discovery and type-check failures continue to qualify coverage.

The compiler helper refuses symlinks and replacements outside the pinned
checkout set. Pinned review does not apply folder-discovery build-output hints
(`target`, `build`, `dist`, etc.) to admitted manifest paths. It materializes Go,
native and embedded inputs under those names before Go selects the build files.
Credential filenames, secret directories, VCS and private-tool directories remain
excluded. Local folder discovery and shared-bundle admission retain their existing
skip policies.

Each side admits up to 64 MiB of repository source plus 256 MiB
of Go vendor inputs, with 20,000 files combined and at most 64 MiB per file.
The vendor allowance applies only below a `vendor/` directory whose parent has
an admitted `go.mod` and whose `vendor/modules.txt` is also admitted, including
nested modules. It covers selected embedded and native assets as well as Go
source. Merely naming a directory `vendor` does not grant the allowance. The
counters span every pin on a side; base and head are bounded independently.
`build.info.review_source_limits` advertises the exact byte and file limits
under scope `owned-and-go-vendor-v1`. This does not exempt any input from
source-policy, authorization, hash, profile, symlink or replacement checks.
Store upload and per-file admission limits continue to apply independently.
Go discovery disables VCS stamping (`-buildvcs=false`): the supplied pins, not
ambient Git metadata, provide provenance. Helper execution and output have explicit bounds. Exceeding a hard bound
fails the request; unavailable compiler analysis makes the report incomplete.

## Read the packet

The packet includes both commit sets, build profile, analyzer/toolchain identity,
source diffs, symbol changes, conservative move/rename correspondence, exact
symbol text and fingerprints, and dependency paths explaining inclusion.

Entries marked `change_seed` are changed declarations. Conservative seeds from
configuration/unmapped source changes use `conservative_scope` and
`uncertain_scope: true`. The `changes` list identifies declaration changes.
Package constants and variables have exact declaration spans; grouped declarations
stay together to preserve implicit expressions and `iota`. `uses_value` links
references to those declarations. `references_callable` links stored or passed
functions without asserting a runtime call. Changed variable initializers and
`init` functions also include the owning packages and their consumers because
initialization may have side effects. An unmapped edit keeps conservative scope.

Local function aliases, closures and interface receivers resolve when one
initializing write proves the target and the variable's address does not escape.
Interface receivers also resolve through fresh struct literals and single-write
local holders whose storage is neither mutated nor exposed. Keyed, positional,
pointer, nested and promoted interface fields use compiler field selections.
Whole-holder or field reassignment, address escape, pointer aliases, captured
writes and concrete holder methods that can implicitly take its address reject
the initializer proof. This is conservative across the whole function, rather
than a flow-sensitive simulation of successive assignments.

For direct owned free functions in the same package, interface-return summaries can
prove a concrete receiver when every return has the same exact concrete type.
This includes forwarding factories and same-type branches. Named results,
defer, generics, recursion, mixed/unknown results and function-variable factories
remain outside this proof. Cross-package interface-returning APIs remain opaque;
dependency implementation bodies are not imported into this analysis. The
original factory call remains a dependency edge. A 256-step per-receiver limit
falls back to explicit uncertainty, and the helper source identity invalidates
old analysis caches automatically. No opt-in is needed.

Unknown interface parameters and mutable global callbacks retain uncertainty.
Exact method identity does not prove runtime success (a typed nil pointer can
still panic) or defect absence. For unresolved interface dispatch, compiler-checked concrete
implementations in the same admitted consumer build may supply `may_call`
candidates with `interface_implementation` evidence. Each must satisfy the entire
interface; none is asserted to be the runtime target. The original unresolved
call remains visible. Candidate search is bounded, prioritizes application calls
with discarded error results, and reports `interface_candidate_budget` when
truncated.

Deleted symbols seed the base graph; additions seed the head graph. Modified
symbols seed both. Configuration and dependency-binding changes conservatively
seed the affected scope. Incoming paths include potential consumers; outgoing
paths provide dependencies and named types. Test callers participate when tests
are enabled. The traversal reuses the graph engine's reverse traversal.
Changed declarations retain their own change reason. Other entries retain one
shortest example path per direction, with deterministic tie-breaking, rather
than repeating every route to the same symbol. Coverage records this policy as
`path_selection: "shortest_example_per_direction"`; these examples are not an
enumeration of all dependency paths.

Exact AST declarations include an enclosing function's entire body. Signature
and implementation revisions are separate. Move/rename matches require unique
compatible candidates; ambiguity remains visible. Existing memory-anchor stable
IDs are unchanged. Legacy graph annotation retains `legacy_revision`, emits
exact declaration `revision` and byte spans where uniquely matched, and uses a
conservative whole-file revision when exact parsing/matching is unavailable.
Existing persisted episodes retain their recorded identities until rebuilt.

`coverage.complete` applies only to the supplied Go/static scope and selected
build profile. Unresolved dispatch, outside-scope calls, unavailable snapshots,
unsupported files, ambiguous correspondence, depth limits, traversal limits and
packing omissions make it false. `no_changes_in_scope` is returned only when the
scope is complete and no changes were found. It is not an organization-wide
“no impact” claim. The packet bounds change/issue examples while retaining
counts and digests; package metadata is represented by its digest and counts.

Native input inventories distinguish selected source, excluded tests/build files,
and embedded assets. README/documentation edits and excluded tests do not select
all declarations. Package-local unknowns and changed import bindings conservatively
include the owning packages and their transitive consumers, using the native
build inventory. They do not seed unrelated suppliers. Unknown build inputs still
retain broader scope and explicit limitations. `file_changes` reports classifications with bounded
examples, a total count and a digest; examples are not the complete inventory.

Diffs use separate exact-byte hunks with three context lines. Packing gives
current-head changed declarations priority. Next come complete immediate callee
bodies when a changed caller discards a compiler-typed `error` result, including
potential interface implementations. This reserves failure-contract evidence
before optional history, flows and diff text; it does not infer that an error
occurs. `coverage.call_contracts` reports selected/required callee entries within
the discovered pinned targets, not completeness of dynamic target resolution.
Diff hunks receive up to a quarter of the total budget from the capacity left
after those source reservations. Deleted/base symbols and compiler-linked consumers and
dependencies follow; uncertain fallback text comes last. Unused reservations
are reclaimed without re-tokenizing candidates. `context_order` defines the
selected view. Under budget pressure, diagnostic change/file examples and
additional dependency-path examples on changed declarations yield to source and
requested history. Counts/digests and uncertainty flags remain, as does the
changed-declaration inclusion reason. Non-seed dependency citations are retained.
Scarce optional capacity is shared between whole history blocks and diff hunks.
Full and delta packets select the same bounded context at the same
budget, independent of previously supplied text. `coverage.current_head` reports
required/selected changed head declarations, `complete`, and whether any useful
head context survived. `minimum_tokens_with_hunk_reserve` estimates the budget
needed for all changed head declarations plus the current hunk allowance; it does
not promise complete dependency coverage. A fixed budget cannot hold arbitrary
source size: increase `max_tokens` (up to 64,000) when that lower-priority context
would otherwise crowd out required evidence. Consumers must require complete
coverage as well as useful head context
before treating the packet as a complete dependency slice. Selective reuse has a
separate per-file proof described below; it never changes this global flag. Deletion-only and non-compiler-input
changes may legitimately require no head declaration.

When tokens are scarce, repeated issue examples are removed before source code;
the issue histogram and completeness flags remain unchanged, and
`issue_examples_omitted` reports the missing examples. Code declarations and
diff hunks remain whole, preserving exact-span and reuse proofs.
If even the smallest measured code candidate cannot fit, optional compiler,
package-failure, dependency-boundary, unsupported-file and Python diagnostic
samples also yield to source. Their full counts, causes and explicit omission
counts remain. This does not guarantee an entry at every budget: required
identities, controls and an exact declaration must still fit together.
`coverage.packing.metadata_tokens` charges the preselection envelope with empty
entries/diffs (and a zero placeholder for the count itself).

`coverage.packing.cost_model: "fixed_hex_identifiers_v1"` makes selection costs
independent of random store-local identifiers. Opaque 32-, 40- and 64-character
hex JSON strings, including identifier references and fingerprint keys, receive
fixed conservative charges by width. Direct pinned content/symbol hashes,
commits, snapshots and policies keep their ordinary token cost; source `text`
and diff `hunk` strings are never normalized. The same accounting applies to
metadata, optional diagnostic samples, history, flows and shared provenance.
Entry ties use source/symbol coordinates instead of the store-local entry ID.

These are selection charges, including `first_entry_tokens` and
`minimum_tokens_with_hunk_reserve`, not claims about the exact wire token count.
The real identifiers, repository bindings, revisions and citations remain in
the packet. Its `tokens` and final budget validation still use the exact cl100k
count of the serialized output. Raw packets and actual token counts can therefore
differ across stores while the chosen source context stays the same. Fixed
charges reserve room for expensive identifier spellings, so some tight budgets
can contain less context than a store that happened to receive cheap IDs. No
analysis gap or omission is promoted to complete to recover that space.
`control_reserve_tokens` is the additional packing allowance for later selection
and delta controls. `first_entry_tokens` reports the first candidate's packing
cost before optional history. These explain allocation; they are not an additive
decomposition of final wire tokens. `max_tokens` still bounds the whole packet.

Packing prioritizes current-head changes, immediate discarded-error call
contracts, immediate callers and diff hunks, then shares space between deletion,
consumer, base-change and dependency classes. Within each code class, shorter
resolved graph paths precede longer ones; file, line and ID break ties.
Conservative scope and history-only candidates have no measured graph distance.
This is dependency distance, not lexical distance from an edited line.
`first_omitted_rank` and `last_selected_rank` are one-based positions in this
combined candidate order (null when absent). There is no single rank cutoff:
an oversized earlier candidate can be skipped while a later candidate fits.

If required content does not fit, `budget_omitted` is nonzero and packet coverage
is incomplete. The full-analysis `impact` verdict does not change with packing.
`budget_omitted_entries` and `budget_omitted_diffs` sum to `budget_omitted` and
include omissions from the canonical selection, not unchanged delta entries.
An unsupported-language packet can have zero code candidates and omitted raw
diffs; `empty_reason` explains only code entry delivery. Increase the budget or
narrow the explicitly declared scope; fewer tokens alone is not a successful review.

## Subsequent pushes

Keep the base pins/profile fixed and send the previous `packet_id` as
`previous_packet_id`. Receipts are issued by this store and bound to the
principal and anchor repository. Up to 20 receipts are retained per pair.
A foreign, evicted, revoked, or incompatible receipt requires a full packet.
If a receipt includes a repository the caller cannot access (even one already
reported as `unavailable_snapshot`), the delta returns HTTP 409 and asks for a
full packet. Retry without `previous_packet_id`; unavailable pins remain explicit
coverage limitations in the full packet.
Receipts have count-based retention (20 per principal/repository), not a time-based expiry.
Caller-provided hashless “already shown” spans cannot suppress changed source.

Apply a delta atomically across both `diffs` and `entries`: remove `invalidated`
IDs from their named `collection`, replace objects marked `updated`, add objects
marked `added`, and retain only the listed `unchanged` IDs from the preceding
packet. Repeated reviews do not resend unchanged diff text. Never merge a delta into an unrelated packet.
Moved/deleted/omitted excerpts are invalidated. A smaller budget cannot silently
drop invalidation metadata: if that metadata cannot fit, the request fails.
Receipts contain revisions/identities, never excerpt bodies.

### Reuse source bytes

Set `reuse_text: true` to omit an updated entry's identical text. The entry still
contains its current binding, `text_sha256`, and `text_ref: true`; its bytes must
come from the same entry ID in `previous_packet_id`. The default remains false.
Python's `apply_review_delta(previous, packet)` and TypeScript's asynchronous
`applyReviewDelta(previous, packet)` validate references, hashes, invalidations,
and `context_order`, then return the current materialized view. Pass `None`/`null`
for the first full packet. Keep that view locally; there is no need to request a
second full packet on every push. A materialized view omits the wire token count;
measure the actual reviewer prompt separately. Equal bytes never preserve old
commit bindings or certify equivalent review inputs.

### Reuse completed review coverage

Send `reviewed_entries` with the head entry IDs from a previous **completed**
review. Each head entry carries `review_basis`: a component fingerprint,
`complete`, and a reason. Components connect native packages through both
incoming and outgoing imports across the base/head graphs. Their fingerprint
covers every member's exact source, transitive package inputs, configuration,
policy, analyzer and toolchain. A changed consumer invalidates a supplier too.

Unresolved calls block their whole component. Analysis failures, unavailable
snapshots, unsupported inputs, traversal limits and issues without a safe owner
block all proofs. Every declaration and consumer-build view of the reviewed file
must be present, and all selected component context and diff hunks must fit the
packet. Packing omissions revoke the proof and update its delta revision.
An unrelated component's uncertainty or omitted text does not revoke a complete
local proof. Global `coverage.complete` and omissions remain unchanged.

`review_reuse.eligible` contains only requested entries whose prior stored proof
and current packed proof are complete, whose file and component fingerprints
match, and whose attached business/execution evidence is unchanged. Older
receipts without this proof require full review. Eligibility proves input
equivalence, **not a review verdict or that a review took place**. The consumer
must account for every required file, require every view of a file to qualify,
retain validated completion evidence and freeze the review schema, invariants,
instructions, author context, source policy and reviewer contract. Any missing
proof, changed contract or omitted view requires explicit full-review fallback.
Never infer completion from carried-forward findings or excerpt presence.

### Reuse compiler facts

The store retains up to four compiler fact sets (each at most 64 MiB, stored as
gzip/base64) per anchor repository. Keys include the complete file inventory,
compiler input hashes, repository/policy identity, profile, toolchain, analyzer
and helper implementation. Existing `.md`, `.rst`, `.txt`, `.json`, `.yaml` and
`.yml` content can change without rerunning Go analysis, except `modules.txt`,
the profile's selected workspace file (regardless of its name), and files beneath
any package containing a `//go:embed` directive. All content
beneath such a package remains hashed, including dependencies and files excluded
by the build profile; this conservatively avoids reimplementing Go embed rules.
Unknown file formats, native inputs and module/workspace metadata remain hashed.
Commits and unrelated content edits do not change compiler inputs; current binding envelopes are always
recomputed. Every read rechecks authorization, retained source and exact spans.
Deterministic type-check failures can also be cached when discovery succeeded and
every selected file has a collected package record. Such cache hits retain all
compiler diagnostics and unresolved calls; they never imply complete coverage or
eligible review reuse. Discovery, parsing, missing-inventory and helper failures
remain uncached. Changing the admitted source set invalidates the fact key.
Deleting any input snapshot invalidates dependent cache records. Native transport
remains bounded to 64 MiB; decoding directly to typed facts caps expansion at
512 MiB. Corrupt or truncated encodings fail. The graph is bounded to 200,000
symbols (including package values) and 500,000 distinct traversal links
(including service links). Repeated call sites with the same caller, target and
relation share a link; all native call-site records remain available for evidence
and unresolved-call accounting within the transport bounds. Native
binding uses a per-package/file declaration index. Traversal reuses declaration
identities and materializes call-site evidence only for selected paths. `GRAPHICLE_PHASE_TIMINGS=1` reports loading,
native analysis, decoding, comparison and packing times to local stderr.

`analysis_cache` reports base/head hits and `granularity: "go_inputs_with_file_inventory"`.
These execution diagnostics are outside semantic packet identity. An unchanged
base, repeated head, or head with only the unrelated content edits described above
can avoid compiler execution. Identical compiler inputs on base and head also
share one analysis within a cold review. Other source languages, configuration
impact and all current snapshot bindings are recomputed. File additions/deletions
and changed compiler inputs require fresh owned-package analysis. Reuse across
requests requires a retained store.
Discovery/parser/helper failures remain uncached. This cache does not admit
missing dependency source.

With `GRAPHICLE_PHASE_TIMINGS=1`, stderr also explains whole-side cache decisions:

- `review_cache_identity` reports whether the native identity is available.
- `review_cache_side` associates base/head with a fact key; `shared_base` marks
  head facts reused directly from the base analysis within this request.
- `review_cache_lookup` reports `hit`, `not_found`, `input_unavailable`,
  `checksum_mismatch`, or `invalid_encoding` (which fails the request).
- `review_cache_eligibility` reports `cacheable`, `empty_worlds`,
  `missing_packages`, `missing_selected_inputs`, `non_cacheable_world_issue`,
  `selected_input_not_in_package`, `native_failure`, or `no_go_roots`.
  `native_issues` counts bounded native issue categories without package names
  or source paths. Unresolved interface calls and the packet's `incomplete`
  verdict do not themselves prevent caching.
- `review_cache_write` distinguishes `oversized` and `unbound_inputs` from
  `write_pending_commit` / `refresh_pending_commit`; successful writes also
  report the number of evicted fact sets. `review_cache_commit` is emitted only
  after the final authorized transaction succeeds. A commit marker alone does
  not mean facts were eligible or written.

Compare keys across identical requests. Repeated `not_found` with a refused
eligibility status identifies a deliberate cache refusal. Different keys mean
the compiler identity or keyed inputs changed. If a write committed and a later
lookup of the same key misses, inspect eviction, snapshot retention and store
identity; the miss alone does not distinguish them. These opt-in diagnostics
change neither the packet schema nor cache eligibility.

The soak benchmark preserves `prime-request.json`, `prime.json`,
`prime-verify.json`, `prime.log`, `prime-verify.log`, and `prime-failure.json`
when available, plus the full `server.log`. It saves responses before validating
priming. A priming failure is recorded in `summary.json` and the next case runs;
the overall run remains incomplete and exits nonzero. Keep these artifacts with
the captured store to diagnose misses without relaxing the warm-cache checks.

Successful dependency packages, including standard-library packages, additionally
retain serialized Go types in the same store. This package pool is scoped to the
anchor repository and partitioned by build configuration: at most 4,096 compressed
exports / 64 MiB per configuration, with at most 4 MiB per encoded entry and 3 MiB
of expanded export data per entry. Base and head share a pool when their discovery
configuration matches; otherwise each loads its own pool, up to the full 64 MiB.
The final authorized transaction retains only the current base/head configurations
(at most two pools: 8,192 exports / 128 MiB per repository). Configuration includes
the helper/toolchain, profile, repository/policy/scope and module/workspace metadata;
ordinary source edits reuse the same pool, subject to native package fingerprints.
A full native pool replaces supplied exports not yet used by that analysis before
refusing new exports. Already-used exports are protected. Persisted pools evict
least-recently-used exports at their individual bounds. Deleting any contributing
snapshot invalidates its exports. Reads recheck access to contributing repositories.
The additive `pool_key` migration preserves sources and existing artifacts; legacy
unpartitioned exports are not loaded and are removed on the next successful review.
An upgrade therefore needs fresh priming. Concurrent reviews commit serially; the
last successful review selects the retained configurations, without mixing facts.
This requires the caller to preserve `--data-dir` and repository identities across
reviews; deleting a scratch store discards reuse. It is not a global module
download service.

The [metadata-service capacity replay](evidence/package-cache-capacity-20261105.json)
retained two 1,493-export working sets (35.3 MiB each) with zero capacity refusals.
Cold, replay and new-head packets matched an exact-commit control; the final
replay hit all 1,493 exports on each side. The eight incomplete-discovery
packages still refused whole-side caching. Timing varied substantially between
control runs, so this establishes bounded reuse and packet equality, not a
production latency or memory-reduction guarantee.

Package fingerprints cover selected and ignored/test Go inputs, native and embedded
files, transitive imports, module/replacement and workspace metadata, build profile,
repository/policy identity, toolchain and helper implementation. Discovery still
runs on the admitted source on every compiler-cache miss. Only successfully checked
packages are exported; cgo packages, incomplete packages and dependencies reaching
owned code remain on the source path. Missing or refused files cannot be restored
from this cache. Owned code always receives full source analysis. Invalid exports
fall back to source checking without hiding diagnostics.

With `GRAPHICLE_PHASE_TIMINGS=1`, package-cache stages also emit bounded JSON
`stats` on stderr, correlated by `key` with `review_cache_side`:

- `review_package_cache_load`: stored and loaded entries/bytes, plus entries
  skipped for size/count limits, unavailable inputs, invalid keys or checksums.
  `namespace` identifies the configuration; its load and retention limits are 64 MiB.
- `review_package_cache_native`: supplied/loaded exports, dependency versus
  standard-library hits, absent/import-failed/invalid-export misses, export
  creation limits, unused-export evictions, and optional transport omissions. `identity` hashes the
  complete native cache configuration; `configuration_sha256` separately hashes
  discovered module/workspace metadata. Equal configuration hashes with different
  identities point to other inputs such as profile, helper or repository policy;
  neither hash alone proves that individual package fingerprints match.
- `review_package_cache_absorb`: offered and accepted entries, refusals and
  entries marked used but unavailable in the configuration’s runtime pool.
- `review_package_cache_partitions`: retained base/head configuration hashes and
  the count of exports removed from other configurations.
- `review_package_cache_save`: written/refreshed entries, skips, evictions and
  retained size. `pending_commit` is provisional until `review_cache_commit`;
  `unused` means there were no used exports to persist.

Byte counters measure encoded export payloads, excluding JSON/SQLite overhead.
Refusal counters count the first applicable reason at each stage. Native hit/miss
and save counters count operations across build worlds, not unique packages.
Native diagnostics survive optional-cache transport omissions and contain only
validated hashes and fixed numeric counters, without package paths or source.
They do not consume review packet space or relax analysis/cache eligibility.
Changing the helper implementation invalidates its cache identity, so upgrading
to this diagnostic build requires fresh priming before comparing warm runs.

The helper vendors `golang.org/x/tools v0.36.0` (`gcexportdata`, Go 1.23 minimum)
for Go type serialization and remains offline. It executes no dependency code.
API boundary targets carry symbol/module identity rather than dependency source
spans. Compressed cache entries are optional within the existing 64 MiB
native-output bound. If all entries cannot fit alongside the facts, the helper
retains a bounded subset (preferring smaller entries) and the hit/miss counters. `analysis_cache.packages` reports base/head hits, misses and
rejected exports; these execution counters do not affect semantic packet identity.
Compiler scratch files remain private and are removed after use. Their writes
do not request crash durability; the authoritative admitted bytes remain in SQLite.
`review_materialize` phase timing separates that preparation from native checking.

## Verification

`tests/it/pinned_review.rs` exercises a three-repository chain, unchanged callers,
base-side deletions, nested functions, file/directory moves, later-push
invalidations, repeated packets, identical-tree commits, unavailable snapshots,
authorization, and token/depth omissions. `tests/go_review.rs` in the engine
checks exact receiver targets, aliases, build tags, simultaneous dependency
versions, local replacements, workspace/test variants, and unsafe paths.
Run `go test -mod=vendor .` in `crates/graphicle-graph/src/extractors` for native
export-cache equivalence, invalidation, corrupt-data recovery and multi-world
type-identity checks. Runtime compiler-cache tests also cover persistence,
authorization, quota enforcement and snapshot deletion.
Memory tests cover exact nested revisions and hashless shown-context reuse.
The chain fixture computes call-edge precision/recall, affected-symbol recall,
and required-context recall at 16,000 tokens. It also checks that applying a
subsequent-push delta produces the same entry revisions as a fresh packet.
`tools/validate_review_rca.py --binary <built graphicle> --output <new directory>`
runs unpaid selection, budget, identical-tree byte reuse and warm-cache checks.
The matching evaluation harness validates pinned dependency closure and useful
current-head context before dispatch. Its opt-in replay adapter requests one
packet per push; it is not enabled in the live watcher.

These are bounded fixtures, not evidence of complete production-language recall.
Policy-withheld dependencies/assets still block the real historical replay. No
paid quality/cost benchmark or production review pruning is established by these
checks.

## Upload execution observations

Core serves these routes without an extension:

- `POST /api/workspaces/{workspace}/repositories/{repository}/impact` (`graph.impact`).
- `POST /v1/workspaces/{workspace}/repos/{repo}/impact/report` (`impact.report`).
- `POST /v1/workspaces/{workspace}/repos/{repo}/coverage` (`coverage.upload`).

Coverage upload requires write access and the following normalized format:

```json
{
  "repository_id": "<project ID>",
  "format": "graphicle.coverage.v1",
  "commit": "<full Git commit ID>",
  "snapshot": "<admitted manifest digest>",
  "policy_sha256": "<admission policy digest>",
  "profile": {"goos": "linux", "goarch": "amd64", "tests": true},
  "environment": "ci-linux",
  "ranges": [
    {"path": "tax.go", "line_start": 10, "line_end": 15, "test_id": "TestCalculate"}
  ]
}
```

The collector translates its coverage tool's output to repository-relative,
1-based inclusive source ranges. Omit `test_id` for suite-level data; Go's normal
suite coverprofile does not identify individual tests. This endpoint does not
execute tests or parse arbitrary native coverage formats. It validates the
commit/snapshot/policy binding, source hashes and line bounds. Observations are
caller-attested evidence, not proof of full test execution or inventory.

Repeated identical uploads are idempotent. The latest 20 distinct runs per
repository are retained; deleting a retained source snapshot cascades to its
coverage. Reports join observations only on the exact repository, commit,
snapshot, policy and build profile. A new commit with identical source still
needs its own run. Absence means unknown, never untested or unaffected.

Matching observations supplement selected symbol entries. Changed or removed
observations change entry revisions, so the next packet explicitly replaces the
previous evidence. Per-run matched ranges are capped at 20 examples with an
omission count; packet token omissions remain explicit. Execution observations
never upgrade unresolved static edges to resolved calls.

Ordinary graph extraction now asks Go for test variants when its input includes
`_test.go` paths. Pinned review includes internal and external test packages when
`profile.tests` is true, and excludes them when false; build tags still apply.

### Environment configuration context

Pinned reviews analyze literal Kubernetes env declarations and Go
`os.Getenv` / `os.LookupEnv` or Python `os.getenv` / `os.environ` consumers by
default when a repository has one admitted application root and one
workload/container. Python links use file-level import dependencies, not a
compiler call graph. Automatic associations are
labelled source candidates, not verified service/environment ownership.
Ambiguous or cross-repository mappings require pinned service manifest v2;
ordinary context retrieval does not infer these deployment scopes. See [environment inputs](service-boundaries.md#literal-environment-inputs-v2)
for the manifest, supported syntax, fallback rules and measurement counters.

Direct calls to established `os.path` bindings for lexical operations (`join`,
`basename`, `dirname`, `split`, `splitext`, `splitdrive`, `splitroot`, `normpath`,
`normcase`, `isabs`, `commonpath`, `commonprefix`) do not create environment-binding
escape warnings. Direct `os.sep`/`os.pathsep` reads and direct
`os.fsdecode`/`os.fsencode` calls follow the same rule; values and filesystem
encodings are not evaluated. `import os as alias` is supported. Arguments are still checked:
passing `os` or `os.environ` remains an escape, and nested `os.getenv` calls still
produce environment reads. `abspath`, `realpath`, `relpath`, `expandvars`,
`expanduser`, stored function/module
aliases, dynamic attributes, conditional imports and monkey-patching remain
conservative. This does not change Python package or re-export resolution, nor
remove `basic_analysis_limitations`. Structural revision 10 invalidates older
cached parses so the warning fix takes effect on the next sync.
