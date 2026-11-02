# Sentinel review transport and measurements

These additive fields ship in Graphicle 0.2.0. Review callers should use compact
status, follow missing-list cursors, and request compact commit receipts. Existing
full status and full commit receipts remain available. Missing-list pagination is
a client migration: callers must follow `next_cursor` before committing.

## Compact status

Call `status` with `{"repository_id":"…","compact":true}`, or:

```sh
graphicle --json-errors status --compact --project PROJECT
```

The `graphicle.status.v1` reply contains `repository` (id, name, snapshot,
generation, updated_at), `files`, `bytes`, `structural_revision`, and
`service_version`. Counts describe admitted paths, including binary files; bytes
count each path. It does not load source text or construct a graph. The CLI compact
form reports admitted state without first syncing the local folder. Python uses
`project.status(compact=True)`; TypeScript uses `project.status({compact:true})`.

## Bounded admission

Measured fixture results and verification are recorded in the
[pilot measurement receipt](evidence/pilot-protocol.md).

1. Send the manifest to `sync.prepare`, using the existing multipart protocol
   (`manifest_complete:false`, then the returned `sync_id`) for large manifests.
2. Upload the returned `missing` paths using existing upload size limits.
3. While `next_cursor` is non-null, call `sync.missing` with `repository_id`,
   `sync_id`, and `cursor`, and upload that page.
4. Call `sync.commit` with `compact:true`. Its receipt uses `changed_count`,
   `removed_count`, `limited_count`, and `skipped_count` instead of path arrays.
   Replay receipts are already small.

The final prepare response and each missing page use `graphicle.sync-missing.v1`:

```json
{"format":"graphicle.sync-missing.v1","sync_id":"…","snapshot":"…",
 "missing":["src/example.ts"],"missing_total":42,"next_cursor":"…","replayed":false}
```

`missing_limit` defaults to 256 and is capped at 1,024. The encoded path list is
also capped at 128 KiB; envelope and cursor add bounded overhead. A cursor belongs
to one immutable pending manifest and advances by path, so uploading earlier
pages does not skip later files. It is opaque pagination state, not authority.
`missing_total` counts currently absent unique objects, so it shrinks as uploads
arrive. Identical SHA-256/size pairs need one representative path upload, even if
many manifest paths share it. Commit checks all required objects again. Replay
returns `replayed:true`, empty `missing`, null cursor, and no pending sync.

## Reuse already admitted sources

Prefer existing context-project base/head snapshot pins directly when their commit
bindings and access meet the review request. A separate scratch project is not
required by the review API.

If scratch isolation is needed, supply up to eight explicit donors on the final
`sync.prepare` manifest part:

```json
{"repository_id":"scratch","expected_snapshot":"…","manifest":[],
 "reuse_sources":[{"repository_id":"context","snapshot":"…"}]}
```

Replace the example's empty manifest with the desired complete tree. Reuse copies
only requested SHA-256/size pairs present in those immutable, readable snapshots
in the same workspace. It verifies content hashes, preserves binary bytes, and
enforces the target's retention limit in the same transaction. An inaccessible
donor fails the call; an available donor without an object leaves that object
missing. Uncommitted donor uploads are not eligible. Prepare reports
`reused:{objects,bytes}`; unchanged-tree replay reports zeros.

Copies are independently retained by the target; later donor changes or deletion
do not revoke them. No global digest lookup grants cross-project access. Target
manifest validation, commit identity binding, and admission policy still apply.
This reuses source bytes, not parse caches, memories, or policy bindings. Base and
head in one target automatically share retained content by digest.

## Risk measurements

Packets carry `risk_schema:"graphicle.review-risk.v1"`. The machine contract is
[review-risk-v1.json](schemas/review-risk-v1.json). It specifies the measurement
subset and permits other packet fields. Consumers must record this marker and the
service version. Changes to enum values or their meaning require a new schema
version; additive unrelated packet fields do not.

`impact` is the existing **string enum**, not an object:

| Value | Meaning |
| --- | --- |
| `potential_impact` | Selected changes may affect supplied context; not a defect verdict. |
| `no_changes_in_scope` | No selected changes in the analyzed scope. |
| `incomplete` | Analysis cannot establish the complete requested impact. |

Every delivered entry has:

| Field | Values and meaning |
| --- | --- |
| `analysis_level` | `compiler_symbol`: compiler-backed Go symbol; `basic_file`: conservative file-level analysis. |
| `update` | `added`: new to this packet lineage; `updated`: changed since its prior entry revision. This is not Git file status. |
| `test_classification` | `test_file` / `non_test_file`: Go filename classification; `not_analyzed`: no supported classification. None proves test execution. |
| `review_basis.scope` | Opaque static component scope, or null when unscoped. |
| `review_basis.complete` | Whether the delivered context satisfies that static component's reuse proof. Not a global completeness claim. |
| `reasons` | Selection explanations; may be empty when compact provenance has summarized examples. |

`review_basis.reason` values are `base_snapshot` (historical side),
`unscoped_uncertainty` (no safe component proof), `unresolved_component`
(unresolved relationships), `missing_file_declarations` (not all declarations
supplied), `complete_static_component` (static proof complete), and
`context_omitted` (packing removed required context). Only the complete static
component reason grants a complete basis.

Selection reason values are `change_seed`, `dependency`, `potential_consumer`,
`conservative_scope`, and `historical_cochange`. The last is a caller-attested,
unverified historical association, not a compiler dependency or causal link.
These values describe why context was chosen, not defect severity. Paths/steps
describe supporting graph traversal when present.
`coverage.complete` and `coverage.budget_omitted` must be recorded alongside impact;
an empty or truncated packet is not evidence that a change is safe.

## Structured failures

Core HTTP errors and CLI failures with `--json-errors` use:

```json
{"format":"graphicle.error.v1","code":"admission_scope","status":403,"error":"Not authorized"}
```

CLI JSON goes to stderr with a nonzero exit. The legacy human-readable `error`
string remains, bounded to 2,048 characters. JSON-RPC preserves its numeric error
code and places this envelope in `error.data`. SDK errors expose `.code`.

Codes derive from typed status: `admission_scope` (401/403), `transport_bound`
(413), `timeout` (408/504), `conflict` (409), `not_found` (404), `rate_limited`
(429), `internal` (other 5xx), and `invalid_request` (remaining failures). Core
request size refusals and recognized provider/I/O timeouts use the corresponding
status. Policy validation that returns 400 remains `invalid_request`. A caller's
transport limit or external process kill must be classified by the caller; it
cannot be reported by a terminated Graphicle process. Delegated engine commands
retain their own CLI contract.

## Source ranking diagnostics

Each context source and its evidence record carries `rank`, `rank_score`, and
`score_kind`. `rank` is its one-based packing input position, preserved if display
order changes. `hybrid_fusion_v1` scores are the actual hybrid fusion values before
diversity selection, not probabilities or model quality scores. Merged/expanded
windows inherit the maximum contributing score. Explicit windows without a fused
candidate have null score and `score_kind:"unscored"`.

`packing` (`graphicle.source-packing.v1`) reports `candidates`, `kept`, `dropped`,
`budget_dropped`, `scored_kept`, `unscored_kept`, and `lowest_score_kept` (null if
none are scored). These count windows reaching the packer, after upstream
candidate selection; they do not count every repository candidate. `dropped`
includes all packing omissions, while `budget_dropped` counts budget omissions.
Mandatory windows and diversity mean the minimum kept score is not a strict
ranker cutoff. Record it together with source identities and review outcomes to
evaluate whether useful context was omitted. These fields do not change ranking.

## Reviewed memories

Reviews remain source-only. Do not import historical fixed/declined finding prose
into the scratch store as authoritative review evidence. An outcome can supply
retrieval hints, but evidence must be revalidated against the pinned sources;
"declined" is not a reusable proof of safety. No new memory-import path is part of
this protocol. This keeps the source-only baseline comparable while source
ranking and admission costs are measured.
