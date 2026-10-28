# Pinned Go change review

Core supports `graph.impact`, `impact.report`, the MCP tool `review_changes`,
and `graphicle review --request review.json`. All use the same request and
permission checks. For multi-repository MCP reviews, use a workspace connection
(`graphicle mcp --workspace`); project connections only admit their own repository
on both sides. Installed extensions retain ownership of operations they
explicitly override. Core also accepts revision-bound execution observations through `coverage.upload`.

This first implementation analyzes **Go package calls in supplied immutable
snapshot sets**. It does not prove behavioral propagation, discover every
consumer in an organization, or link RPC/HTTP/event contracts. Python and other
languages remain available to ordinary graph retrieval, but their presence in
a review scope is reported as unsupported. An incomplete result must never be
interpreted as “no impact.”

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
false `scope_complete` produces an explicit coverage limitation. Upgrading it
requires a new explicit collector attestation.

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
It imports source bytes only and rebuilds compiler facts for review. Omitting
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
`"workspace":"checkout/go.work"`. Otherwise workspace discovery is off.

A local Go toolchain must be installed. Each consumer gets its own native
`go list`/`go/types` resolution world. Build tags, platform, module versions,
workspace/local replacements, and test variants determine the selected source
and target declaration. Modules and toolchains are **never downloaded during
review**; CGO is disabled. Provide pinned local replacements, a workspace, or
a vendor tree for the dependency source. An arbitrary compatible checkout is never substituted
for an installed version. Missing dependencies remain unresolved.

The compiler helper refuses symlinks and replacements outside the pinned
checkout set. Source materialization is bounded to 64 MiB and 20,000 files per
side; helper execution and output have explicit bounds. Exceeding a hard bound
fails the request; unavailable compiler analysis makes the report incomplete.

## Read the packet

The packet includes both commit sets, build profile, analyzer/toolchain identity,
source diffs, symbol changes, conservative move/rename correspondence, exact
symbol text and fingerprints, and dependency paths explaining inclusion.

Entries marked `change_seed` can be directly changed symbols or conservative
seeds from configuration/unmapped source changes. The `changes` list separately
identifies declaration changes. Package-global value references currently produce
explicit unresolved data-flow edges; an unmapped source edit also seeds the
consumer builds selecting that file.

Deleted symbols seed the base graph; additions seed the head graph. Modified
symbols seed both. Configuration and dependency-binding changes conservatively
seed the affected scope. Incoming paths include potential consumers; outgoing
paths provide dependencies and named types. Test callers participate when tests
are enabled. The traversal reuses the graph engine's reverse traversal.

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

Packing prioritizes diffs and changed symbols, then selected dependency context.
If required content does not fit, `budget_omitted` is nonzero and impact is
`incomplete`. Increase the budget or narrow the explicitly declared scope;
fewer tokens alone is not a successful review.

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

## Verification

`tests/it/pinned_review.rs` exercises a three-repository chain, unchanged callers,
base-side deletions, nested functions, file/directory moves, later-push
invalidations, repeated packets, identical-tree commits, unavailable snapshots,
authorization, and token/depth omissions. `tests/go_review.rs` in the engine
checks exact receiver targets, aliases, build tags, simultaneous dependency
versions, local replacements, workspace/test variants, and unsafe paths.
Memory tests cover exact nested revisions and hashless shown-context reuse.
The chain fixture computes call-edge precision/recall, affected-symbol recall,
and required-context recall at 16,000 tokens. It also checks that applying a
subsequent-push delta produces the same entry revisions as a fresh packet.
These are bounded fixtures, not evidence of complete production-language recall.

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
