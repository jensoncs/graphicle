# Measuring Graphicle quality

Run the local regression scorecard against a built binary:

```sh
python3 bench/memory/tools/quality.py run \
  --binary crates/graphicle/target/debug/graphicle \
  --baseline bench/memory/quality-baseline.json \
  --out bench/memory/runs/my-change
```

Python 3.11 or later is required. The runner uses only the standard library and
the existing evaluators. No model assets, credentials or external repositories
are needed. Each run uses disposable stores, strips provider credentials from
the CLI environment and disables update notifications. Use a new output folder
for each run. Output includes synthetic source and memory text; runs with private
datasets must stay private. Nothing is sent as telemetry.

## What the scorecard measures

The checked-in suite contains 89 authored memory questions and 21 authored
source questions across seven code languages. Memory questions run with symbol
links off, counted (the default), and on. Source questions run with identifier
routing off and on. These are **309 context calls**, covering relevant decisions, distracting memories,
negative controls, stale notes, caller/callee constraints, capacity pressure,
exact source branches, attached rationale and UTF-8 citation boundaries.

These cases are regression fixtures, not an independent estimate of user
effectiveness. Existing fixtures labelled `held_out` remain authored synthetic
cases. This suite runs the model-free configuration; semantic reranking,
full-repository and cross-repository evaluations remain separate experiments.

The initial model-free baseline (31 October 2026) records:

| Variant | Questions | Memory precision | Pooled memory recall | Correct empty memory | Source completeness |
|---|---:|---:|---:|---:|---:|
| Links off / counted | 54 each | 66.7% | 19.2% | 88.2% | — |
| Links on | 54 | 31.6% | 92.3% | 29.4% | — |
| Source, routing off / on | 6 each | — | — | — | 100% |

All 174 calls completed and counted preserved off's delivered content. A second
fresh-store run preserved every case's relevant IDs, irrelevant IDs and delivered
spans. These figures make irrelevant linked memories and lost unlinked constraints
visible together; they do not justify choosing one variant on precision alone.
They are not directly comparable with the local-reranker trials in the benchmark
README, which use a different retrieval recipe.

### Linked-memory correction (31 October 2026)

The follow-up fix narrows automatic code-to-memory expansion when explicit
symbols resolve unambiguously. It keeps their callers, callees and transitive
code dependencies, rather than treating every neighbouring declaration in a
source window as relevant. Ambiguous names, unavailable or stale graph data and
oversized dependency components keep the existing window-based fallback.

Questions asking only for a call's result with literal inputs (for example,
`What does retry_delay(3) return?`) do not automatically add linked decisions.
Ordinary memory retrieval still runs. Questions asking why, containing symbolic
inputs, or making additional requests retain the ordinary expansion path.
Both changes apply only with `symbol_links: true`; the counted default, source
access checks, freshness checks and four-note limit are unchanged.

| Links-on measurement | Original | Fixed |
|---|---:|---:|
| Relevant retrieved occurrences | 48 | 48 |
| Irrelevant retrieved occurrences | 104 | 56 |
| Precision | 31.6% | 46.2% |
| Pooled recall | 92.3% | 92.3% |
| Negative questions returning no memory | 5/17 | 10/17 |
| Mean packet tokens | 430 | 389 |

The fixed binary passed all 174 calls against the original frozen baseline:
no previously retrieved relevant ID or required source span was lost, and no
case gained an irrelevant ID. That comparison kept the original baseline and
corpus labels unchanged. The scope change alone removed 39 irrelevant occurrences; the
literal-value rule removed another nine. The
[comparison receipt](../bench/memory/results/symbol-links-quality-fix-20261031.json)
records the input hashes, per-case changes and validation.

A separate matched run enabled the same pinned local MiniLM reranker in both
builds: all 324 calls completed across the 54 memory questions and three modes.
Irrelevant links-on occurrences fell from 102 to 54, precision rose from 32.9%
to 48.1%, and all 50 previously retrieved relevant occurrences were retained
(96.2% pooled recall). No case gained an irrelevant ID. Off/counted memory IDs
were unchanged, and counted preserved off's content within both runs. The
dedicated shallow-source-reranking test still preserves four linked-note slots.

This fixes two causes of noise, not all relevance problems. Notes attached to
the same symbol can still be off-topic. Two caller/callee rationale questions
still lose a relevant note under the model-free four-note cap, and a broad
question with six relevant notes can return only four. Default counted-mode
recall remains 19.2%. These are synthetic regression results, not measured
production or generated-answer quality.

Follow-mode packets expose `symbol_links.query_symbol_scope` (`applied`,
`symbols`, `excluded_links`) and `symbol_links.literal_call_value` (`applied`,
`withheld_links`) when links exist. These counters describe expansion choices;
they are not relevance labels. The scorecard saves them in case diagnostics.

| Measurement | Definition |
|---|---|
| Memory precision | Relevant returned memory IDs / all returned IDs, completed calls only; undefined if no memory is returned |
| Memory recall | Relevant returned IDs / labelled relevant IDs, including failures as misses |
| Macro recall | Mean per-positive-query recall, including failures as zero |
| Correct empty memory | Negative queries returning no reviewed notes / all negative queries; failed calls are incorrect |
| Evidence recall | Required spans delivered / all required spans, including failed calls |
| Source completeness | Queries delivering every required span / all source queries |
| Tokens | Actual packet reference-token counts, completed calls only |
| Latency | Observed per-call wall time including CLI startup, with sample count and nearest-rank p50/p95/p99 |

The existing source scorer verifies excerpt hashes and exact bytes. A definition
citation can satisfy a location question, but cannot substitute for a behavior
branch or rationale. Memory precision is ID-level, not a claim that every token
is relevant. Correct empty memory is not generated-answer abstention.

`SCORECARD.json` includes every case, category breakdowns and sample counts.
`SCORECARD.md` provides a readable summary. `PROTOCOL.json` freezes corpus,
binary and evaluator hashes, request options, variants and the expected case
inventory before querying. Changed or removed run inputs fail the gate; the
output directory is exclusively reserved to prevent concurrent overwrites.
`CASES.json` is updated after each project so an
interrupted trial leaves partial evidence, but a partial trial cannot pass.

## How the gate works

The command exits 1 for a regression and 2 for invalid input or incompatible
report structure. Exit 0 without `--baseline` means **unbaselined**, not a passed
comparison. The gate rejects:

- Any failed/invalid observation, stale-memory leak, or failed/unmeasured parity
  between links off and counted.
- Lost previously retrieved relevant memory IDs, or newly returned irrelevant
  IDs, in any individual case.
- Lost previously delivered required source spans in any individual case.
- Changed datasets, request settings or scoring contract, missing cases or
  variants, and duplicate observations.

Aggregate improvements cannot hide a regression elsewhere. Low baseline quality
remains visible: passing means preservation or improvement, not that the system
has reached a universal quality target. Timing and total tokens are reported but
not gated: a shared CI runner is not a controlled latency experiment, and adding
necessary evidence may legitimately consume more tokens.

The Linux runtime CI job runs the Python evaluator tests and this suite, posts
the scorecard in the job summary and uploads the public fixture evidence for 14
days, including failed runs. It never silently replaces the baseline.

## Diagnosing a miss

Each case and variant has matching request, packet and diagnostics files under
`cases/<dataset>/<project>/<query>/`. The artifact path serves as the correlation
ID without changing the runtime's public packet or storing user queries in a
new production database.

Diagnostics reuse `GRAPHICLE_PHASE_TIMINGS=1` and packet mechanism counters.
They retain phase time and resident memory, route selection, source state,
omissions and link/reranker details where available. Timings are nested; adding
them does not yield end-to-end latency. The scorecard's wall time is authoritative
for the observed call. Errors and timeouts remain failed observations.

For source queries the runner also requests the existing top-400 candidate
diagnostics. Required spans are classified as delivered, present in an observed
candidate but lost during selection/packing, not wholly contained in any observed
candidate, or unavailable for diagnosis. A missing candidate is **not proof of
an indexing failure**: the debug list is bounded and identifier routing may
bypass it. Inspect the saved packet and existing index/graph reports to continue
the investigation. This avoids inventing a precise loss stage from incomplete
evidence.

## Improving the system

### Memory diagnostics (1 November 2026)

The scorecard now requests `debug_candidates: 400` for memory questions as well
as source questions. A direct `context` request can set this existing option
to 1–400 to return `memory_candidates_debug`. It is absent by default. The trace
contains IDs and stage observations from already authorized, servable memory
candidate rows; it adds no memory bodies or titles and retrieves no extra notes.
It does not change ranking, limits or packing. Trace output is additional debug
JSON, outside the served context token/character budget; keep private artifacts
private just as you would the original packet.

For each recorded memory, the trace includes retrieval-arm provenance, its
memory fusion rank/score when observed, code-to-memory link decisions, its rank
in the linked candidate prefix and final order, optional local-reranker score
and input-truncation flag, pack position, and delivery status/route. Scores are
not calibrated relevance probabilities. Link-outcome counters count encounters,
so multiple windows or symbols can contribute to one memory's counters.

The evaluator joins these observations to its relevance labels, which are never
sent to retrieval. Each `.diagnostics.json` adds `memory_quality`, and each
memory scorecard case retains the same diagnosis. Summary and category metrics
include:

| Metric | Interpretation |
|---|---|
| Precision/recall at 1, 4, 8 | Quality of the delivered-memory prefix; precision divides by the number actually returned, up to k, and is undefined for an empty prefix |
| Mean reciprocal rank | Position of the first relevant delivered memory, averaged over completed positive questions; a miss contributes zero |
| Observed candidate recall | Relevant IDs in the recorded eligible candidate rows / labelled relevant IDs on traced questions; a lower bound, not index recall |
| Observed packing recall | Relevant IDs offered to packing in the recorded trace / labelled relevant IDs on traced questions |
| Missing by observed stage | Packing, linked final cap, linked candidate cap, observed link withholding, eligible but unselected, unobserved candidate, or unavailable diagnostics |
| Irrelevant notes by route | Whether each labelled irrelevant delivered memory came from ordinary ranking or symbol-link expansion, with per-case ranks and linked symbols |
| Measurement coverage | Planned, measured and traced questions, truncated traces, and the denominator/sample count for each diagnostic aggregate |

Diagnostic rank metrics describe completed calls and always expose sample
counts; the primary scorecard recall still counts failed calls as misses.
An absent ID is **not** labelled an indexing, authorization, freshness or
ranking failure: it may fall outside retrieval or the recorded prefix. Even
with a complete candidate-row trace, fusion itself is bounded. A link-withheld
diagnosis describes the observed link path, not a claim that other retrieval
paths could not have found the note. Missing or older-version traces remain
explicitly unavailable. Diagnostics report mechanisms; relevance comes only
from the evaluation labels.

The memory debug setting is part of the frozen request contract, so old
scorecards cannot silently compare under the new settings. The baseline refresh
uses matched runs of the previous and instrumented executables with the same
debug setting and unchanged questions/labels. Regression gating still compares
relevant/irrelevant IDs and exact source spans; diagnostic fields do not relax
that gate. The matched 174-call runs preserved every memory outcome and required
source span. All 162 candidate memory traces were available and untruncated.
They locate all four links-on misses at the linked-candidate cap; 51 of the 56
irrelevant occurrences came through symbol links and five through ordinary
ranking. All 52 labelled relevant occurrences were observed in eligible rows.
These findings target candidate prioritization and linked-note relevance; they
do not establish how an independent user workload behaves. See the
[instrumentation receipt](../bench/memory/results/memory-diagnostics-20261101.json).

### Selection fixes (1 November 2026)

The follow-up fixes address three mechanisms exposed by these traces:

- Dependency scoping now uses the graph's existing nesting-relation definition.
  Nesting follows only toward the owner, retaining enclosing-class contracts;
  a `method` edge no longer turns unrelated sibling methods into dependencies.
  Ambiguous names retain the union of all matching definitions and their code
  dependencies instead of widening to every declaration in the source windows.
  Unresolved names and unavailable/stale or oversized graphs retain the fallback.
- Model-free link expansion collects a bounded prefix of 32 eligible notes
  before its delivery cap. Exact duration agreement ranks that prefix: for
  example, `sixty seconds` and `one minute` represent the same duration. This
  signal supports nonnegative integer phrases in milliseconds, seconds,
  minutes, hours and days, including English numbers below 100. Unsupported
  quantities and bare numbers have no inferred unit. Nonmatching notes keep
  their order and are not rejected. Configured local rerankers keep their
  existing scoring path and bounded candidate depth.
- Explicit project-wide inventories such as `What decisions govern this
  pipeline?` use the request's `limit` as their linked-note allowance, with a
  floor of four and ceiling of 32. Focused questions keep four. This expands
  only eligible links reached through retrieved code; it does not enumerate
  every stored note or guarantee a complete inventory. Packing still enforces
  token/character budgets and reports omissions. `symbol_links.memory_limit`
  and `candidate_memories`, plus the trace's `linked_memory_limit`, expose the
  allowance and candidate count.

Against the preceding instrumented baseline, all 174 model-free calls passed
the unchanged per-case gate:

| Links-on measurement | Before | After |
|---|---:|---:|
| Relevant retrieved occurrences | 48 | 52 |
| Missing relevant occurrences | 4 | 0 |
| Irrelevant retrieved occurrences | 56 | 51 |
| Precision | 46.2% | 50.5% |
| Pooled recall | 92.3% | 100% |
| Negative questions returning no memory | 10/17 | 10/17 |

The two lease rationales are recovered by duration agreement, and the broad
pipeline question now retains all six labelled notes. The class/ambiguous-scope
fix removes three irrelevant occurrences; the two recovered rationale notes
displace two others. No case loses a previously retrieved relevant ID or gains
an irrelevant ID, and every required source span remains delivered. Off/counted
memory outcomes remain unchanged. Labels and corpus questions were not edited.

The matched optional-reranker comparison also passed all 324 calls: relevant
occurrences rose from 50 to 52, irrelevant occurrences fell from 54 to 51,
precision rose from 48.1% to 50.5%, and pooled recall rose from 96.2% to 100%.
The final model-free replay reproduced every outcome. The baseline now protects
these recovered IDs and removed distractors, with no request-contract or label
changes. Validation passed 61 Rust tests, 136 Python evaluator tests and the
real-model boundary test; runtime library lint passed. Memory-library strict
lint encountered an existing `collapsible_if` warning in unchanged
`graph/cross_repo_http.rs:117`, recorded in the receipt.

Precision remains limited: 46 irrelevant occurrences still arrive through
symbol links and five through ordinary ranking. A shared duration is a ranking
signal, not proof of relevance; a code dependency alone also does not establish
that a note answers the question. No global model-score cutoff or implicit
model download was introduced. These synthetic regressions establish neither
production recall nor generated-answer correctness. Full results and validation
are recorded in the [selection receipt](../bench/memory/results/memory-selection-20261101.json).

### Prose memory scope (1 November 2026)

Focused prose questions could retrieve the appropriate note, then expand into
unrelated notes attached to neighbouring declarations in the same source window.
With symbol links enabled, a prose query can now use the current symbol links of
its already-ranked memories as scope anchors. Their callers, callees, transitive
dependencies and enclosing owners use the same traversal as explicit symbols.
This only restricts code-to-memory expansion; ordinary ranked notes are retained.

Narrowing requires every matching query stem to occur in the ranked notes' titles
or text, using the existing note-ranking stem/stop-word rules. An uncovered topic
or paraphrase keeps the broader fallback. Every ranked note must have resolved,
current links whose source hashes match; missing or stale data, incomplete graphs
and traversal bounds also retain fallback. Explicit symbols and paths keep their
existing handling, and recognized project inventories are never narrowed this
way. Lexical coverage is a prerequisite, not proof of semantic completeness.

`symbol_links.query_symbol_scope.basis` now distinguishes `explicit_symbols`,
`ranked_memories`, and `null` (no scope). Existing excluded-link counters and
candidate traces identify which notes were withheld. Debugging remains optional
and does not change delivered content.

| Links-on measurement | Before | After |
|---|---:|---:|
| Relevant retrieved occurrences | 52 | 52 |
| Missing relevant occurrences | 0 | 0 |
| Irrelevant retrieved occurrences | 51 | 46 |
| Precision | 50.5% | 53.1% |
| Pooled recall | 100% | 100% |
| Negative questions returning no memory | 10/17 | 10/17 |

All 174 model-free calls passed the frozen per-case gate with unchanged questions,
labels and settings. Three queries improved: the settlement query lost an unrelated
receipt note, the receipt query lost two billing notes, and the freshness query
lost two cache-key notes. No query lost a relevant ID, gained an irrelevant ID, or
lost a required source span. Off/counted memory outcomes are unchanged.

The optional local-reranker run completed all 162 calls and reproduced the same
three improvements against the previous frozen 162-call run: precision rose from
50.5% to 53.1%, all 52 relevant occurrences remained, and irrelevant occurrences
fell from 51 to 46. No case lost relevant IDs or gained irrelevant IDs. Model
assets, requests and datasets matched. After reviewing both comparisons, the
baseline was tightened to protect the five removed distractor occurrences.

The regression test covers dependency retention, multiple ranked anchors,
uncovered topics, unresolved explicit names, inventories, unlinked-note fallback
and debug parity. All 17 symbol-link-filtered integration tests and strict runtime
library Clippy passed. The
[comparison receipt](../bench/memory/results/memory-prose-scope-20261101.json)
records the protocols, hashes, per-case changes and optional-reranker comparison.

The remaining 46 irrelevant occurrences comprise 41 symbol-link occurrences and
five ordinary-ranking occurrences. Same-symbol distractors and unsupported
questions remain unresolved. These authored synthetic results do not establish
production recall, answer correctness or semantic completeness of inferred scope.

### Regression workflow

1. Reproduce a failure with a frozen question, source revision and labels. Add a
   regression fixture before changing retrieval. Keep captured user questions
   separate from authored controls and obtain independent relevance labels.
2. Run the same suite and options against baseline and candidate binaries. Keep
   the raw packets and diagnose candidate selection, reranker visibility and
   packing separately. Rerun the full-repository and semantic suites when a
   change touches those paths.
3. Require both preserved relevant evidence and reduced unnecessary evidence.
   Do not optimize precision by dropping useful callee constraints or by turning
   retrieval off. The per-case gate checks this tradeoff.
4. Review and explicitly accept a new baseline only after checking the cases,
   provenance and validation. Changing labels or the scoring definition requires
   a new matched baseline; bump `scoring_version` when metric semantics change.

Freeze to a **new file**, then review its diff against the accepted baseline:

```sh
python3 bench/memory/tools/quality.py freeze \
  --report bench/memory/runs/my-change/SCORECARD.json \
  --out bench/memory/runs/proposed-baseline.json
python3 bench/memory/tools/quality.py compare \
  --report bench/memory/runs/my-change/SCORECARD.json \
  --baseline bench/memory/quality-baseline.json
```

Freezing refuses invalid observations and existing destinations. It records the
original report hash and protocol, and removes timings and diagnostic traces from the comparison
baseline. Baseline updates belong in a reviewed change with the old comparison
and a reason for every accepted tradeoff, never as an automatic response to red
CI. A supplied manifest (`--suite`) can select additional memory corpora in the
existing format; source fixtures use the inline `quality_sources.json` format.

## Language and historical-query coverage

The November 2026 coverage expansion adds 35 memory questions and 15 source
questions without changing retrieval code, budgets or the existing questions.
The new fixtures exercise qualified and ambiguous method names, enclosing
contracts, cross-file dependencies, focused prose, unsupported policy questions
and literal results. Source labels include interface/receiver declarations,
TypeScript inheritance, Java/Kotlin overloads and Rust trait implementations.

| Code language | Memory questions | Source questions |
|---|---:|---:|
| Python | 50 | 6 |
| JavaScript | 4 | 0 |
| TypeScript | 7 | 3 |
| Go | 7 | 3 |
| Java | 7 | 3 |
| Kotlin | 7 | 3 |
| Rust | 7 | 3 |

On the unchanged binary, the expanded links-on memory baseline measures:

| Code language | Relevant retrieved / labelled | Irrelevant occurrences | Precision | Recall |
|---|---:|---:|---:|---:|
| Python | 50 / 50 | 41 | 54.9% | 100% |
| JavaScript | 2 / 2 | 5 | 28.6% | 100% |
| TypeScript | 10 / 10 | 8 | 55.6% | 100% |
| Go | 8 / 10 | 6 | 57.1% | 80% |
| Java | 10 / 10 | 8 | 55.6% | 100% |
| Kotlin | 10 / 10 | 8 | 55.6% | 100% |
| Rust | 10 / 10 | 8 | 55.6% | 100% |

Pooled memory precision is 54.3% and recall is 98.0% (100/102 relevant
occurrences); 15/27 negative questions return no memory. The changed corpus means
these aggregates are not a before/after retrieval improvement claim. All original
174 outcomes remain unchanged, including the earlier precision and recall gains.
All 21 source questions deliver their complete required spans in both variants.

The two new Go misses are `qualified-method` and `ambiguous-method`: both miss
the `lease` note on `RetryDelay` in `policy.go`. Its link resolves successfully
and the note is an eligible candidate, but no link encounter or packing offer is
observed. The delivered code covers `sender.go`; the qualified-method scope
contains only two symbols. This identifies a dependency/source-reachability
investigation, not evidence that a score cutoff or the four-note cap caused the
loss. The baseline records these misses explicitly; no runtime fix or label
change was made to conceal them.

These are authored regression fixtures, all with English questions, not
independently sampled language effectiveness estimates. A syntax appearing in a
fixture does not establish complete graph extraction or support for every use
of that syntax. Other supported code languages remain outside this baseline.

Scorecards now include `by_language` and `by_origin` summaries in JSON and
Markdown, with completed/planned counts for every variant. Language comes from
the project file extensions; mixed-language projects form a separate group and
unknown extensions are explicit. Per-case gating still prevents an improvement
in one language from hiding a regression in another. `memory_grounding` and
each project's `memory-links.json` record the links saved during note setup so
binding failures can be distinguished from later retrieval misses. They contain
link metadata, not additional memory bodies, and do not influence retrieval.

The separate `quality_real_suite.json` replays 11 existing questions on pinned
historical Go source slices: **two captured diagnostic tool queries** and **nine
authored real-source questions**, producing 22 context calls. They are reported
separately by origin. Capture validation checks the original log hash, line,
tool, arguments and exact question; source labels must match pinned bytes.
Sources and captures are checked again after the run to detect input drift.
Neither category is an independently labelled human-query sample, and no
reviewer-memory relevance dataset is claimed for these source-only cases.
Both variants delivered all 17 required spans across all 11 questions in the
historical replay. Its own baseline preserves those outcomes separately from
synthetic memory metrics.

```sh
python3 bench/memory/tools/quality.py run \
  --binary /absolute/path/to/graphicle \
  --suite bench/memory/fixtures/quality_real_suite.json \
  --baseline bench/memory/quality-real-baseline.json \
  --out /private/output/real-quality
```

Historical checkouts and capture logs must be available at the fixture's paths;
they are not bundled or fetched automatically. A checkout can be relocated with
`--source-root historical/nexus-963-h1=/absolute/path/to/checkout` (repeat for
other projects); its hashes must still match. Capture logs retain their pinned
paths. Keep these run outputs private: their packets contain historical source.
The portable synthetic suite remains the CI gate; the historical suite is an
explicit local run with its own baseline.

Both suites were replayed on fresh stores before accepting their baselines. The
expanded baseline preserves the original 174 outcomes and records the new Go
misses; passing it means no regression, not that recall is complete. The
[coverage receipt](../bench/memory/results/coverage-expansion-20261101.json)
records corpus/binary/evaluator hashes, per-language results, query-origin
breakdowns, repeat comparisons and the unchanged-runtime check. Validation
passed 146 Python evaluator tests (two optional model tests skipped), including
captured-request integrity, source relocation, language-specific regression
gating, and the live CLI's language/origin summaries and note-link diagnostics.

Independent real-user coverage still requires de-identified captured questions,
the source and reviewed-memory snapshot available when each question was asked,
and relevance labels reviewed without looking at retrieval output. Record the
capture provenance and reviewer decision, separate projects used for tuning
from held-out evaluation, and keep unsupported/ambiguous questions in the sample.
Do not reclassify the existing diagnostic captures as human traffic.

### Go helper repair (1 November 2026)

The two Go recall misses above came from helper compilation, before retrieval:
`go build` inherited the user's build cache, but the evaluator intentionally
omits both `HOME` and `GOCACHE`. Compilation failed and the graph lacked the
compiler-resolved call from `JsonSender.Send` to `RetryDelay`. The helper now
uses a writable temporary `GOCACHE`, preserving its offline build policy.
Graph cache schema 14 invalidates the degraded graphs on the next `sync`.
An installed Go toolchain is still required.

A regression test runs in a fresh process without either environment variable,
checks standalone and module packages, resolves a callee across files, and
rejects a same-named function in another package. All 10 graph resolution tests
pass. An old-binary/new-binary upgrade check also recovers both missing notes
after resyncing the same store with unchanged source bytes.

This repair also exposes a precision problem in linked-memory expansion:
dependency reachability admits the `lease` note for the focused wire-agreement
question and the unsupported encryption-policy question. The Go links-on cases
improve from 8/10 to 10/10 relevant occurrences (80% to 100% recall), while false
positives rise from 6 to 8 and precision falls from 57.1% to 55.6%. Correct call
edges establish a code relationship; they do not establish question relevance.
The accepted baseline and relevance labels remain unchanged. The new irrelevant
notes must remain visible as gate failures, rather than being accepted by
refreshing the baseline or hidden with a Go-specific filter.

The complete 309-call replay has no execution errors or lost relevant notes.
Pooled links-on recall rises from 98.04% (100/102) to 100% (102/102), while
precision moves from 54.35% (100/184) to 54.26% (102/188). The strict gate fails
exactly the two new Go false positives above. All other per-case outcomes,
including the original 174 cases and all 21 complete synthetic source
questions in both modes, remain unchanged. The separate 22-call historical
Go-source replay passes its gate with all 17 required spans in both modes.
The [Go repair receipt](../bench/memory/results/go-recall-fix-20261101.json)
records the paired outcomes, unchanged baseline hashes, candidate binary and
evaluator hashes, upgrade check and the remaining precision failure. This is
a verified extraction/recall repair, not an accepted precision improvement.
Relevance filtering after dependency expansion remains to be addressed while
preserving the recovered callee recall.

## What remains unmeasured

The scorecard explicitly marks independent real-query quality, generated-answer
correctness, agent task success, total agent tokens and production latency as
unmeasured. Existing memory-answer adapters can support subsequent experiments;
they do not supply coding-task outcomes for this suite. Do not interpret served
token counters, a successful tool call, source completeness or user approval of
a memory as proof that an agent completed a task correctly.

For latency comparisons, use repeated runs of the same optimized builds on an
idle machine, pin hardware/settings, separate cold and warm states, and retain
failure rates and sample counts. The CI percentiles alone are not an SLO.

To test the measurement code itself:

```sh
python3 -m pip install -r bench/memory/requirements-test.txt
GRAPHICLE_BIN=/absolute/path/to/graphicle python3 -m pytest bench/memory/tests -q
```
