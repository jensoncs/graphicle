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

Missing-object discovery uses one batched SQLite statement per page, with the
existing `(repository_id, sha256, bytes)` index covering availability checks.
The total and selected paths use the same read snapshot. Only a page plus one
lookahead path reaches the response builder, which enforces the encoded byte
limit. Existing cursors and pending syncs need no migration or client change.

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

For admission gaps, send the exhaustive `metadata.withheld_paths` attestation
described in [pinned reviews](pinned-review.md#admit-source-snapshots). This avoids
a blanket incomplete verdict for known credentials outside the Go build, while
retaining uncertainty for missing source and possible embedded inputs.

For large-repository timing, distinguish prepare/missing discovery, upload,
commit parsing/chunking, projection, and native review analysis. Reusing donors
removes repeated uploads but a fresh scratch project still needs its own parse
and chunk records. Directly reviewing retained context-project snapshots also
avoids that scratch admission work. A reused target keeps parse/chunk records
for unchanged path/hash pairs, subject to analyzer revision changes. Missing-list
pages still rescan the manifest and check each unique object inside the batched
query to keep totals current. Batching removes per-object Rust query executions;
it does not eliminate the underlying index probes or repeated manifest scans.
The reported Nexus 252-second sync has not been
reproduced here, so the local byte-saving fixture is not a production latency claim.

`sync.commit.analysis_cache` (also forwarded by local `sync`) reports
`structural_revision`, `parsed_files`, `reused_files`, `revision_reparsed_files`,
and `uncached_files`. Counts describe nonbinary files in that invocation;
`parsed_files = revision_reparsed_files + uncached_files`. `cache_rebuilt` is true
when an existing parse for the same path/hash had an obsolete revision and was
reparsed. This includes changes to analysis limits and retries of transient
failures, not just parser releases. Cold admission is `uncached_files`, not a
revision rebuild. A partially completed sync retry counts only its remaining
work. A replayed `sync.prepare` with no sync ID did no parsing and needs no
commit. Compact commit replies retain these counters.

Record these sync counters alongside the review packet; review-time
`analysis_cache.base_hit/head_hit` describes a different cache of Go compiler
facts. A historical sync rebuild must not be attributed to each later review.
`build.info.sync_analysis_cache` advertises `parse-reuse-and-rebuild-counts-v1`.

Full Go-analysis cache hits do not load the dependency-export pool. On a full
analysis cache miss in a Go review, the pool is loaded once and base-created exports
remain available to head analysis. With `GRAPHICLE_PHASE_TIMINGS=1`, that load is
reported as `review_load_package_cache`; it is absent when both sides hit the full cache.
Verified, unchanged cache payloads refresh usage and snapshot bindings without
re-encoding or resubmitting the payload. Evicted or replaced rows fall back to a
normal save, and newly compiled exports replace rejected data. Checksums, source
authorization, final snapshot-retention checks and cache limits still apply.
These savings concern review-time caches, not dependency preparation or sync.

## Risk measurements

When `entries` is empty, `empty_reason` explains why. It is separate from
`impact`, and diffs may still be present:

| Value | Meaning |
| --- | --- |
| `unsupported_source_language_or_contract` | No source entries were available and unsupported inputs exist; inspect the histogram for types. |
| `no_changes_in_scope` | Full analysis found no changes in its scope. |
| `analysis_incomplete` | No source entries were available and analysis has limitations. |
| `budget_omitted` | Entries existed but none fit the packet. |
| `already_provided_in_diff` | All candidate entries were fully covered by caller-declared diff spans. |
| `unchanged_from_previous_packet` | Selected entries were already delivered under the prior receipt. |
| `diff_only` | No entries are delivered, but diffs are. |
| `no_selected_entries` | No entries or diffs were selected; inspect impact and coverage. |

An unsupported TypeScript-only review stays `impact: incomplete`; an explained
empty packet is an ingestion outcome, not a successful risk assessment. The
field is absent on nonempty entry arrays. `build.info.review_empty_reason`
advertises `entry-delivery-reasons-v1`.

`coverage.issue_histogram` counts **all** issues before `coverage.issues` is
reduced to 20 examples. `groups` contains `{reason, cause?, count}` rows; native
Go package suffixes are grouped under their common reason. `counted` equals
`coverage.issue_count`, and the group counts plus `other_count` always sum to it.
The same limitation on base and head counts twice, as it does in `issue_count`.
This is occurrence accounting, not a count of distinct root causes. At most 128
groups are emitted; `other_groups` and `other_count` account for any additional
groups. Counts include selection/scope issues as well as native analysis and
basic-language issues. Budget omissions remain separately counted in
`coverage.budget_omitted`. `build.info.review_issue_histogram` advertises
`full-analysis-reason-cause-counts-v1`.

For `unsupported_source_language_or_contract`, histogram groups additionally
contain `extension` and `side` (`base` or `head`). Extension labels are lowercase,
without the dot; `""` means no extension and `<overlong>` represents suffixes
longer than 32 bytes. `cause` identifies the first matching detection rule:

- `contract_extension`: `.proto`.
- `assembly_extension`: `.s` or `.S`.
- `build_filename`: `Makefile` or `Dockerfile`.
- `shebang`: an otherwise unsupported file beginning with `#!`.
- `code_extension`: another extension classified as code, including SQL and
  plain JSON/YAML outside a supported Terraform/Helm context.

These labels explain existing detection; they do not add language support or
change completeness. When unsupported issues exist, the histogram also includes
`unsupported_count` (all occurrences, including overflow groups),
`unsupported_examples` (at most eight `{file, side, extension, cause}` rows), and
`unsupported_examples_omitted` (`unsupported_count` minus example count).
Examples represent emitted groups, prefer prevalent groups, deduplicate the same
path/extension/cause across sides, and share a token allowance of `max_tokens / 64`.
Counts remain independent of this allowance and of the 20-issue sample. An
unchanged unsupported file counts once per side; examples are not a distinct-file
census. `build.info.review_unsupported_inputs` advertises `extension-cause-side-v1`.

`coverage.analysis_diagnostics.base/head` separates native failures from their
effects on graph references, before packet trimming:

- `python_imports.unresolved_occurrences`, `files`, and `by_cause` count every
  unresolved Python import occurrence, distinct source file, and resolver cause.
  `modules` contains at most eight token-bounded `{module, cause, count}` groups;
  `module_groups` and `module_groups_omitted` account for the rest. Sampling
  balances causes and prefers frequent modules within each cause. These names
  are import spellings, not inferred package-manager distributions.
  `external_or_unresolved` includes standard-library and third-party imports
  absent from the admitted source, as well as missing local candidates. It is
  not evidence of a package-layout bug. `ambiguous` means multiple candidates.
  Histogram rows for `basic_unresolved_import` also carry `language` and `cause`.
  The `python_imports` object is absent when there are no unresolved Python imports.

- `go.incomplete_package_instances`, `discovery_incomplete_package_instances`,
  `type_check_failed_package_instances`, and `unavailable_external_package_instances`
  count distinct `(consumer build, import path)` pairs. These sets overlap.
- `go.incomplete_package_references` counts unresolved calls/references whose
  containing package failed analysis. A single broken package can contribute
  thousands of these; they are not thousands of independently broken packages.
- `go.examples` preserves bounded compiler diagnostics with `package`, `cause`,
  `consumer_build`, `message`, and source `file`/`line` where available. Causes
  include `discovery_error`, `missing_package`, `outside_snapshot`, `import_cycle`,
  `parse_error`, `type_error`, and `unsupported_cgo`. Import errors can cascade;
  examples are evidence, not automatic root-cause classification.
- `python_environment.incomplete_files` counts files, and `files_by_cause`
  distinguishes parser/analysis availability, dynamic execution, ambiguous or
  escaping bindings, unsupported imports, operation limits, and local `os`
  modules. A file may contribute to several causes. Its examples carry paths
  and causes, not inferred missing environment files.

Native Go diagnostics retain at most 32 examples per consumer build, three per
package, eight per cause, and 512 message characters plus a truncation marker.
Each side's response samples at most eight examples per language family under a
shared allowance of `max_tokens / 32`; omission counts remain explicit. Counts
describe observed diagnostics, not unique root causes. Example selection rotates
through causes so missing-dependency messages cannot consume every slot. Cached
compiler facts retain diagnostics. Existing issue counts, completeness, risk
verdicts and reuse barriers are unchanged.

`build.info.review_analysis_diagnostics` advertises
`bounded-go-errors-and-python-causes-v1`. To investigate the live probes, retain
these summaries along with source-admission counts and exact snapshot/build
identities. A missing generated declaration, absent import and source type error
can then be distinguished using compiler evidence rather than the total limit
count. The b0773acb probe alone does not establish which inputs were missing.

Packets carry `risk_schema:"graphicle.review-risk.v2"`. The machine contract is
[review-risk-v2.json](schemas/review-risk-v2.json). It specifies the measurement
subset and permits other packet fields. Consumers must record this marker and the
service version. Changes to enum values or their meaning require a new schema
version; additive unrelated packet fields do not.

`impact` is the existing **string enum**, not an object:

| Value | Meaning |
| --- | --- |
| `potential_impact` | Selected changes may affect supplied context; not a defect verdict. |
| `no_changes_in_scope` | No selected changes in the analyzed scope. |
| `incomplete` | Analysis or source admission cannot establish the complete requested impact. |

In v2, `impact` is decided from the full analysis of the supplied snapshot set,
before trimming entries and diffs to the response budget. Packing cannot change
it. `coverage.budget_omitted` counts entries and diffs that did not fit;
`coverage.complete` remains false when either analysis or packet coverage is
incomplete. Per-entry `review_basis` still becomes incomplete when supporting
context is omitted. A `potential_impact` packet with omitted context therefore
does not justify skipping review or treating the delivered context as complete.

The historical [v1 schema](schemas/review-risk-v1.json) used `incomplete` for both
analysis gaps and packet trimming. Record the schema marker when comparing
builds; consumers that recognize only v1 must add v2 before interpreting the new
verdict. Basic-language limitations, unavailable inputs, and `scope_complete:false`
admission remain genuine analysis gaps in v2. This change does not turn impact
into a risk-severity or defect prediction.

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

Pinned reviews can opt in to supplemental context with `provided_diff`. Omit it
for the existing full review behavior. Each range names the exact admitted file
on one side and the inclusive, one-based lines already supplied to the reviewer:

```json
{"provided_diff":[{"side":"head","repository_id":"REPOSITORY_ID",
  "file":"pkg/service.go","file_sha256":"FULL_FILE_SHA256",
  "line_start":40,"line_end":58}]}
```

There are at most 4,096 ranges. Invalid sides, paths, hashes, repositories or
line bounds are rejected. Overlapping and adjacent ranges merge; a gap remains
uncovered. Only entries whose complete byte span lies in those lines are
excluded before packing. A function partially overlapping a diff remains
eligible at lower priority. With nonempty ranges, packing uses
`additional_context_then_overlap_v1`: graph-linked entries outside the ranges
first, then overlapping entries and remaining raw hunks, then entries supported
only by uncertain scope or history. Each tier preserves the existing semantic
priority and graph-distance order. Source selection precedes optional history
and execution-flow annotations, with no raw-hunk reservation. Whole declarations
are retained; this does not slice functions or infer that uncovered lines were
already supplied. Without ranges, the original packing policy is unchanged.
A raw hunk is excluded only when every nonempty base/head span is
covered, so declaring head lines alone cannot suppress deleted or old context.
Send only ranges actually present in the review prompt, after any diff trimming.

`coverage.provided_diff` reports `entries`, `diffs`, and normalized `ranges`.
It also reports eligible `overlapping_entries`, `selected_outside_entries` and
`selected_overlapping_entries`. Selected counts cover the canonical view,
including unchanged delta entries; any byte overlap counts the whole entry as
overlapping. These are entry counts, not counts of duplicate tokens or evidence
of usefulness. Outside means outside the supplied ranges, not outside all actual
changes if the caller supplied only part of its diff.
These exclusions are separate from `budget_omitted_entries`
and `budget_omitted_diffs`. Original `required_entries`/`required_diffs` count
the full candidates: each equals selected (including unchanged delta entries)
plus provided-diff exclusions plus budget omissions. Packing ranks and cutoffs
apply to the remaining eligible candidates. `current_head.provided_diff` counts
excluded current-head change seeds.

The full graph still determines `impact`. The declaration is a caller assertion
that text was supplied, not a completed-review attestation: excluded context
keeps packet coverage and affected component reuse proofs incomplete. Its
fingerprint participates in `review_reuse.context_sha256` and receipt identity. Deltas invalidate entries
removed by this option and restore them when requested again; never accumulate
the old packet's excluded entries into the supplemental view.
`build.info.review_provided_diff` advertises `fully-covered-spans-v1`.
`build.info.review_context_ranking` advertises `additional_context_then_overlap_v1`.

For the next Sentinel probe, hold admitted snapshots, token budget and diff
prompt constant; compare entry counts and source-token counts inside versus
outside the supplied diff, then review citations/usefulness. The option removes
known duplication; usefulness still needs production measurement.
The extra accounting can cross a packing threshold at very small budgets, so
this option does not guarantee a higher entry count on every request.
For latency attribution, `GRAPHICLE_PHASE_TIMINGS=1` logs
`review_provided_diff_validate` and `review_provided_diff_annotate` separately
from `review_pack` and analysis phases on stderr. Compare cold and warm runs
separately and alternate probe arms; total time alone cannot attribute a
slowdown to range validation, ranking, token counting, or cache state.

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
