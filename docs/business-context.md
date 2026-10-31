# Static business context for pinned reviews

Graphicle owns the evidence connecting a change to code, dependency paths and
historical intent. The reviewer decides whether the proposed change violates a
constraint. This feature supplies candidates, not business-criticality
scores or measured hot paths. It makes no model or telemetry calls.

## Contract and ownership

`impact.report` accepts an optional `business_context: {history: [...]}` (the
history array defaults to empty). Existing callers retain their current output.
With this option, selected symbol entries gain `business_context`: lexical
domain candidates, review questions, and matching historical assertions. The
entry's existing `reasons` contain the compiler-backed dependency path explaining
why it belongs in the review. With history analysis enabled, statistical peers
carry a separate `historical_cochange` reason and `compiler_dependency: false`.
Classification never adds graph edges or claims
that behavioral impact propagates along every edge.

The local `graphicle review-history --path CHECKOUT --pin pin.json --file PATH`
collector produces one history bundle. Repeat `--file` for explicitly selected
files. Put its JSON output in the request's `business_context.history` array.
Collect separate bundles for base and head as needed. The server never runs Git
against a caller-supplied filesystem path. Python and TypeScript SDKs pass this
option through their existing impact methods.

For example:

```sh
graphicle review-history --path /path/to/checkout --pin base-pin.json \
  --file access.go > base-history.json
graphicle review-history --path /path/to/checkout --pin head-pin.json \
  --file access.go > head-history.json
```

```python
import json

with open("base-history.json") as f:
    base_history = json.load(f)
with open("head-history.json") as f:
    head_history = json.load(f)
packet = project.impact([base_pin], [head_pin], profile=profile,
                        business_context={"history": [base_history, head_history]})
```

Both sides matter: if a new edit overwrites a guard, its original correctness-fix
subject can remain visible on the **base** symbol even though head blame names
the new commit. Reuse that base bundle for later pushes; recollect head history
for the new pin. Do not send duplicate bundles when the two pins are identical.

## Evidence and limits

The collector reads blobs and blame at the full pinned Git commit, independently
of the working tree. It attributes surviving lines to their last-writing commits
and retains commit subjects. No source checkout, hooks, network, model calls or
repository mutations are needed. Collection is bounded by time, files, bytes and
attribution ranges. Shallow history and omitted ranges are explicit.

The server verifies repository access, the complete pin, file SHA-256 and line
bounds against admitted sources. Git ancestry and subjects remain **caller
attested**: a hash verifies source correspondence, not the truth of the message.
Subjects are untrusted data, never instructions. Matched assertions carry exact
line intersections, commit IDs and content-derived evidence IDs. Every assertion
is marked `validity: unverified`; no automatic owner confirmation is performed.

Identifier words suggest domains such as authorization or payments. Commit
subjects can suggest the same domains, but retain a separate evidence basis.
No category is inferred from graph degree, edit frequency or filename alone.
Revert subjects are identified as revert candidates and excluded from positive
domain inference; the system does not infer which earlier claim they supersede.

Blame is intentionally narrower than full historical diff analysis: deleted
lines, superseded fixes, merge intent, and moved/copied lines are not exhaustively
recovered. Rename continuity is not assumed. A function gets history only where
attribution overlaps its exact current declaration. Nested declarations may
share enclosing context; this is line overlap, not proof of semantic ownership.
No matches means unknown, never low business importance. Owner-reviewed memories
remain in the existing memory system; joining them to historical snapshots needs
a separate revision-safe design.

## Freshness, budgets and privacy

History is request-local and stored only as hashes in the existing review receipt;
there is no second evidence database or implicit sharing. Existing final access,
snapshot and source-retention checks also protect enriched entries. An unavailable
or mismatched history pin/file rejects the request; it is never joined by name.
Clients must resupply history for subsequent pushes, with the new exact pin.

Each entry's revision includes its context. Changing, removing or disabling
context updates or invalidates the prior entry through normal packet deltas.
Only a bounded number of assertions per symbol is packed; omitted matches are
counted. Context shares the existing token budget with code and diffs. Budget
omissions and incomplete dependency coverage continue to be reported; absence
of a candidate never certifies no business impact.

## Measuring review value

The deterministic fixture compares equal-budget packets with and without this
option. A later authorization-helper change must retain an unchanged caller,
the caller-to-helper path, and a past tenant-isolation fix associated with the
helper's surviving lines. An unrelated function and a revert must not become
positive business assertions. Tests also cover stale input rejection, permission
isolation, context changes/removal in deltas and token limits.

This measures evidence availability, required-context recall and token overhead.
It does not establish improved model-review accuracy or production value. A later
blind review evaluation should score supported findings, missed regressions and
false positives at equal budgets, using the same pinned input set in both arms.
Dependency coverage and input consistency remain prerequisites for that trial.

Run the deterministic value fixture from `crates/graphicle` (using the normal
repository build environment):

```sh
GRAPHICLE_BUSINESS_EVAL_DIR=/tmp/graphicle-business-context-evaluation \
  cargo test --locked --test it \
  pinned_review::business_context_adds_historical_constraints_and_revision_safe_deltas \
  -- --nocapture
```

It writes `baseline.json`, `enhanced.json`, and `summary.json` for side-by-side
inspection. The fixture uses a real local Git history for the overwritten guard;
separate caller-attested examples test irrelevant and revert-subject handling.

The base blame collector is language-neutral; `--analyze` requires declarations
from the structural parser. Review selection follows the pinned Go pipeline;
this feature does not add JS/TS/Python call-resolution support.

## Function history and review priority

Add `--analyze` to collect a bounded historical sample alongside surviving-line
blame. The same JSON bundle works through both SDKs:

```sh
graphicle review-history --path /path/to/checkout --pin head-pin.json \
  --file access.go --analyze > head-history.json
```

This reads up to 64 first-parent integration commits reachable from the pin.
A merge is compared with its first parent; a squash commit is naturally one
unit. Ordinary commits are not automatically grouped into PRs. Neither the
branch name nor staged/unstaged changes affects a pinned review.

| Evidence | What the reviewer receives | Limits |
| --- | --- | --- |
| Function history | Prior changed declarations, bounded patches, commit subjects and revisions | AST correspondence uses path, name and kind; no rename continuity, ambiguous identities are withheld |
| Co-change coupling | Conditional probability, lift, support counts, weighted totals and supporting commits | Association within the sampled history, not a call edge or proof of a required companion change |
| Hotspots | Age-weighted changed lines × AST decision-count proxy | Not runtime traffic, business importance or a calibrated defect probability |
| Ownership | Recency-weighted shares of surviving-line blame | Repository-scoped hashed contributor IDs; useful for routing, not author-performance assessment |
| Fix-to-cause candidates | Parent-blame commit IDs for removed lines in likely correctness fixes | At most four removed lines per change; not validated SZZ or proof that a commit introduced a bug |
| Similar historical changes | Bounded patches ranked by hashed lexical-token Jaccard overlap | Local lexical matching, not embeddings or semantic equivalence |

For each retained commit, `w = 2^(-age_days/180) / files_touched`. Age is relative
to the pinned commit timestamp, with future timestamps clamped to age zero.
For target A and partner B, `P(B|A) = sum(w for A and B) / sum(w for A)`;
`lift = P(B|A) / (sum(w for B) / sum(w for all sampled units))`.
Suggestions need complete retained commit membership, at least three joint units, conditional probability at least
0.7, and lift at least 1.2. Sampling stops at the target's introduction or
reintroduction. These are explicit heuristic thresholds, not calibrated risk
estimates. Commits with no retained function changes do not enter the sample.

The review layer recomputes these scores from the submitted sample. It may add
up to six peer suggestions, prioritizing current-head seeds and reporting
`omitted_peer_suggestions`, materialized only from exact authorized source
hashes and native declaration spans, retaining applicable build views. Peers
are still subject to packet limits. Historical evidence is marked
`caller_attested` and `unverified`; hashes do not prove historical provenance.

`review_priority` explains an `elevated` recommendation through prior
correctness-fix candidates, unchanged historically coupled functions, or at
least three sampled changes in a function with five or more AST decisions.
Otherwise priority is `unassessed`. This neither sets business criticality nor
permits skipping review. Compiler uncertainty and `coverage.complete` retain
their existing meaning.

Each evolution candidate is bounded to one eighth of the requested token budget.
The packer reserves required code and hunk space first, then fits optional
business/history annotations into the remaining capacity, capped at one quarter
of the packet budget. Base history and peers do not bypass that reservation. Updating or
removing history changes entry revisions and delta invalidations.

### Collection hygiene and bounds

Declaration syntax fingerprints ignore whitespace and comments while retaining
literal values. Generated/vendor paths, lockfiles and commits touching more
than 32 files are excluded. A mass rename is treated as additions/deletions or
excluded at that bound. Collection allows 128 historical file pairs, 32 current
candidate files, 512 current functions and 128 changed functions per commit;
blobs are bounded to 1 MiB and patches to 2 KiB. The evolution pass has a
45-second deadline in addition to the existing blame collector's deadline.
Errors, shallow history, exclusions and bounds make the sample partial. Parser
support is limited to declarations exposed by the existing structural analyzer.
Tests exercise Go, Python, JS and TS metrics; pinned review selection remains Go.

Historical source and messages pass the existing credential-shape detector
before they enter emitted evidence, and the server scans supplied bundles too.
Structured history is scanned as decoded fields, including dictionary keys; it
is never flattened into one enormous artificial JSON line. Individual source
fields that exhaust the detector still fail closed, with an explicit scan-failure
message. Credential checks also run on candidates before budget selection.
This is a pattern-based filter, not a guarantee of discovering every secret.
Raw email addresses are not emitted. Contributor hashes are pseudonymous, not
anonymous, and can be recomputed if the address is known. Historical patches
are source material and must follow the same access policy as the review.

Deferred: working-tree/merge-base overlays, remote PR grouping, exhaustive
pickaxe or `git log -L` search, language diff-driver configuration, full SZZ,
embedding retrieval, LLM summaries/git notes, and commit-graph maintenance.
The implementation performs no model calls, network requests or Git mutations.

### Deterministic acceptance

```sh
GRAPHICLE_HISTORY_RISK_EVAL_DIR=/tmp/graphicle-history-risk-evaluation \
  cargo test --locked --test it \
  pinned_review::historical_coupling_adds_exact_context_and_explains_review_priority \
  -- --nocapture
```

The fixture creates actual commits in which two functions repeatedly change
together, then changes only one. It requires the changed helper, its unchanged
compiler-proven caller and the historical peer to coexist, with distinct
inclusion reasons. It checks source mismatch rejection, candidate cause links,
ownership, bounded packets and peer invalidation when history is removed.
It writes baseline/enriched packets, the history bundle and a summary. This
establishes context delivery; reviewer accuracy, cost and speed need a separate
paired evaluation.


## History byte budgets

`review-history --analyze` now emits at most **240 KiB of compact JSON** by
default. Two default bundles fit the server's existing **512 KiB aggregate
`business_context` limit**, including the JSON envelope. Use `--max-bytes N`
(16–512 KiB) when allocating a different share. Pretty-printed CLI output can
be larger; the limit measures the same compact UTF-8 JSON representation as the
server. The server continues to reject oversized supplied requests.

Selection bounds retained data while collecting, rather than constructing a
multi-megabyte result and failing at the end. Pinned file metadata stays intact;
blame ranges compete within a separate allowance and dropped ranges increase
`omitted_ranges`. Requested-file declarations take priority over incidental peers. Analysis and
peer discovery stay within `pin.modules`, so unrelated modules do not introduce
current sources that are absent from the pinned review input.
Complete change units are retained newest first. All function memberships,
revision hashes and change counts in that recent prefix take priority over
optional patch detail. Lexical sketches and then whole patch excerpts can be
omitted to keep complete units; incidental-file patches are omitted before
requested-file patches. If even a unit's membership cannot fit, collection stops
retaining older units rather than concealing the gap. The existing continuity
check still stops at a partial unit that does not contain the target.
Source spans, revision hashes, line counts, and retained patch
excerpts are preserved. Optional lexical sketches retain at most 16 term hashes and can be omitted
entirely to preserve exact declaration anchors;
`lexical_terms` counts omitted hashes.

`evolution.omissions` reports `output_functions`, `output_changes`, and
`output_units`; `output_patch_excerpts` counts discarded optional excerpts, whose
`patch_truncated` flag stays explicit. Membership is not discarded merely to
preserve an excerpt. Supplied partial commits still report `omitted_changes`.
An omitted change is never interpreted as an unchanged function: partial units
are excluded from co-change statistics, and coupling is withheld when discarded
commit membership prevents safe historical correspondence. This includes units
omitted by output limits, unavailable or withheld units, and mass-change units.
Function history,
patches, churn and ownership can remain available, all explicitly partial.
This is bounded analysis, not a fallback to blame-only collection.

Review packing first reserves unannotated current-head declarations and required
call contracts, plus room for an affordable complete diff hunk. Optional business metadata for that required code then competes
for at most one quarter of the packet budget; remaining capacity funds hunks
and other context. Nonempty analyzed history takes priority over metadata-only
annotations, with changed current-head entries first within each group. Among
blocks at the same priority, smaller complete blocks come first. Their event records,
source bindings, sample limitations and continuity omissions remain intact. Metadata
that cannot fit is omitted without dropping its code entry. The
`business_context.packing` counters report the allowance and omitted annotations.
`used_tokens` measures annotation capacity allocated to selected entries;
`retained_entries` counts annotations actually delivered. Analyzed history gets
first use of optional capacity before execution-flow enrichment. Flow citations
are not charged as required code: a complete path and its cited reason upgrades
must fit together. Unused flow/hunk reservations can fund a second pass over
selected entries without history, including optional code, within the same quarter-budget cap.
`refill_tokens` reports that second pass. No history bundle is truncated to fit.
Full and delta packets use the same deterministic selection; dropping or changing
an annotation changes its entry revision. A history-enabled packet with no actual
analyzed evidence remains an unsuccessful history treatment in evaluation.

The evaluation adapter's `collect_histories` allocates a shared byte budget
across distinct base/head pins before calling the collector, reserving the exact
array/envelope overhead. Re-review requests check the aggregate budget before
transport. Preserve these omission records in the reviewer prompt and resupply
current pinned history on a later push. Changed omission metadata changes entry
revisions and deltas, even when the source excerpt itself is unchanged.

History metadata is collected in bounded Git batches for the retained blame
ranges (up to 32 files × 128 ranges), rather than failing at 128 distinct commits
across the request. Each record, subprocess response, deadline, combined
base/head byte budget and credential check remains enforced. Attribution and
history omissions remain explicit; complete recent membership still gates use
of older evidence.
