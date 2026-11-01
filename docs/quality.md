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

The checked-in suite contains 146 authored memory questions and 49 authored
source questions across seven programming languages plus HCL/Terraform, Helm,
Kubernetes YAML and Bash. Memory questions run with symbol
links off, counted (the default), and on. Source questions run with identifier
routing off and on. These are **536 context calls**, covering relevant decisions, distracting memories,
negative controls, stale notes, caller/callee constraints, capacity pressure,
exact source branches, attached rationale, UTF-8 citation boundaries, configuration
references and overrides, template helpers, workload selectors and shell pipelines.
Coverage groups distinguish Helm from Kubernetes YAML and Terraform from generic
HCL; they do not replace the syntax inferred from the fixture file extensions.

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
The follow-up below addresses relevance filtering while preserving the recovered
callee recall.

### Precision and unsupported topics (1 November 2026)

Two changes were measured separately against the unchanged 309-call suite.
The starting point here is the Go repair above, including its two newly added
false positives. Questions, labels, settings and the accepted baselines were
not changed.

| Links-on measurement | Go repair | Step 1: prose scope | Step 2: topic support |
|---|---:|---:|---:|
| Relevant retrieved occurrences | 102 | 102 | 102 |
| Irrelevant retrieved occurrences | 86 | 76 | 37 |
| Precision | 54.26% | 57.30% | 73.38% |
| Recall | 100% | 100% | 100% |
| Negative questions returning no memory | 15/27 | 15/27 | 26/27 |

Step 1 keeps a fully covered prose topic on the ranked notes' verified live
symbols. It no longer expands that inferred scope into callers, callees or
owners. Explicit code questions and incompletely covered prose retain their
existing dependency/fallback behavior. Five focused prose cases across
TypeScript, Go, Java, Kotlin and Rust each lose two irrelevant notes. This
supersedes the dependency-closure behavior of the earlier prose-scope change.
The intermediate gate still fails the Go unsupported-policy case; that failure
is retained in the receipt.

Step 2 recognizes narrow English attribute questions about explicit code names,
separates the requested topic from the name, and checks existing stems in the
authorized candidate notes and source text. With no topical match, it withholds
both ranked and linked memories while retaining source retrieval. Any topical
support restores the existing selection behavior. It does not apply to broad
constraint/contract inventories, compound questions, rationale questions,
unrecognized forms, semantic ranks, rerankers, or requests with explicit needs
or rephrasings. There are no Go-specific terms, benchmark IDs or calibrated
score thresholds in the rule.

`memory_topic_support` reports the topic terms, matching terms and withheld
counts. `unsupported_in_candidates` means no lexical support in the candidates
inspected, not proof that a fact is absent from the repository. Candidate traces
record `selection_filter: unsupported_topic`; the evaluator attributes an
associated recall miss to `relevance_withheld` and still fails the ordinary
recall gate. Positive controls verify that adding supporting notes, using
inflected topic words, or finding support in source text preserves retrieval;
debugging does not change results.

Step 3 passes the frozen gate and comparisons against both the Go repair and
step 1. All 102 relevant occurrences remain, including both recovered Go callee
notes, with no per-case increase in irrelevant IDs. The synthetic source checks
retain all 33 required spans across 21 questions in each mode. The separate
historical-source replay retains all 17 spans across 11 questions in each mode
(22 calls). Go precision rises from 55.56% to 83.33%, with recall at 100%.

One negative case still returns a ranked tracing note for `eventKey(7)`; this
code-result question is outside the new attribute rule. Overall, 37 false
positives remain. This lexical rule can miss synonym-only attribute requests
without matching candidate wording; independent human-query validation remains
necessary. These figures are synthetic regression results, and do not establish
production accuracy or equivalent behavior for non-English questions.

The separate optional local-reranker replay preserves all outcomes across its
54 questions / 162 calls (52 relevant and 46 irrelevant occurrences with links
on). The new attribute filter is bypassed throughout that run; the model-free
precision gains above are not claimed for reranked retrieval.

Validation passes 18 symbol-link integration tests, 69 evaluator tests (one
optional model test initially skipped), all 21 symbol-link evaluator tests when
the local model assets are supplied, and runtime Clippy with warnings denied.
The broader 533-test integration run has 528 passes, three ignored tests and
two stale terminal-display assertions expecting seven graph nodes. Both the
starting and candidate binaries produce the identical eight-node graph. Those
assertions now compare the display with the structured sync count, and all 11
terminal tests pass on recheck. No retrieval behavior changed for that repair.

The [step-by-step receipt](../bench/memory/results/memory-precision-steps-20261101.json)
preserves comparisons, per-case changes, binary/evaluator hashes, validation
and the intermediate failed gate. The accepted baselines remain unchanged.

### Infrastructure and Bash coverage (1 November 2026)

`quality_infrastructure_memories.json` adds 30 memory questions; the matching
source fixture adds 20 source questions. The five projects cover Terraform
module inputs/outputs, validation and explicit tfvars overrides; a generic HCL
job and variable; Helm values, named helpers and template includes; Kubernetes
selectors, ConfigMap references and a Kustomize patch; and Bash sourcing,
same-named functions, argument quoting, pipeline failure and delay bounds.
Every group includes direct prose, dependency/constraint and negative questions.

These are authored English regression questions. Terraform is not planned or
applied, Helm is not rendered, Kubernetes is not reconciled and the shell scripts
are not executed. The source labels require exact supplied text, not a generated
answer or a claim that the referenced configuration behaves correctly at runtime.
All three Bash files pass `bash -n`.

The setup audit initially rejected symbol-linked notes in four groups. Inspection
of the current live-symbol table distinguished a Terraform naming mismatch from
missing symbol support: the nested Terraform output has a module-qualified
identity; generic HCL projects the variable but not its job blocks; these Helm
and Kubernetes fixtures have no live symbol identities. The Terraform fixture
uses the actual output identity. HCL job notes, Helm notes and Kubernetes notes
explicitly use Graphicle's existing file-dependency API. HCL variable and Bash
function notes still use symbols. No automatic fallback, relevance-label change,
question rewrite or runtime extraction fix was used to conceal the setup errors.
The failed setup trial is retained in the receipt and is not a quality baseline.

The evaluator now accepts and validates explicit `dependencies` in memory
fixtures. Only unique paths in the supplied project are admitted. Alongside
`memory-links.json`, `memory-files.json` and the scorecard's
`memory_file_grounding` preserve the saved dependency hashes. Empty symbol lists
for file-dependent notes do not count as successful symbol resolution. Future
symbol-link coverage for Helm and YAML still needs working projected symbols.

| New coverage group | Note grounding | Relevant / labelled | False positives | Precision | Recall |
|---|---|---:|---:|---:|---:|
| Terraform | Symbols | 6/6 | 11 | 35.3% | 100% |
| Generic HCL | File dependencies + variable symbol | 4/5 | 4 | 50.0% | 80% |
| Helm templates | File dependencies | 5/5 | 6 | 45.5% | 100% |
| Kubernetes YAML | File dependencies | 5/5 | 4 | 55.6% | 100% |
| Bash | Symbols | 9/9 | 8 | 52.9% | 100% |

The HCL `job-change` question misses the `drain` note on `var.drain_seconds`.
The note is an authorized candidate, but no retrieval arm, link encounter or
packing offer is recorded. The absent job symbol is a concrete extraction gap;
these observations do not implicate the final memory cap. Precision issues
include Bash's alternate implementation leaking into a path-qualified question,
and irrelevant notes on unsupported-policy and literal-configuration questions.
Only 3/10 new negative questions return no memories.

All 44 required source spans across the 20 new questions are delivered in both
routing modes. Together the new memory cases retrieve 29/30 relevant occurrences
with 33 false positives. These are new coverage measurements, not improvements
over the earlier 73.38% precision / 100% recall corpus. A single HCL miss and
substantial over-returning remain visible.

The full 439-call run preserves all original 309 outcomes exactly. Its 130 new
calls reproduce the first valid infrastructure run on fresh stores, with no
execution or parity errors. The expanded baseline records 131/132 relevant
memory occurrences (99.24% recall), 70 false positives (65.17% precision), and
29/37 correct empty-memory results. All 77 required source spans across 41
questions are retained in each routing mode. The lower pooled precision reflects
the harder added corpus, not a change to retrieval: the binary is unchanged.
The baseline was frozen only after the original-case comparison, exact repeat
check and setup audit; its known HCL miss remains explicit. Validation passes
77 Python evaluator tests, with one optional model test skipped.

The [infrastructure coverage receipt](../bench/memory/results/infrastructure-coverage-20261101.json)
records the setup correction, per-group results, unchanged original outcomes,
repeat comparison and baseline provenance. Passing the expanded regression
baseline means retaining observed outcomes; it does not certify complete HCL,
Helm, Kubernetes or Bash support.

### HCL recall and configuration precision follow-up (1 November 2026)

The infrastructure questions, source bytes, note bodies, grounding and relevance
labels remain unchanged. The runtime now retains labelled root blocks in generic
`.hcl` files and their nested references, so `job.worker` reaches the checkpoint
note on `var.drain_seconds`. Unknown Terraform blocks retain their old behavior.
The earlier setup description above records the pre-fix runtime; the HCL fixture
intentionally keeps its original file-grounded job notes for this comparison.

Identifier routing now recognizes lowercase HCL address namespaces and
configuration/template file extensions. This also lets the existing unsupported
attribute check handle `module.queue` and `job.worker`. Narrow setting-value and
explicit last-file override questions with matching selected source and setting
names withhold design memories. Rationale, compound requests, missing source or
settings, semantic ranking, reranking, and caller-supplied needs or query variants
keep ordinary retrieval. `memory_value_request` reports the path, setting and
withheld counts; candidate traces record `configuration_value_request`.

The existing fully-covered prose scope also accepts explicit file dependencies,
after checking their saved and live source hashes. This prevents the new HCL
dependency from adding the checkpoint note to a question asking only for the
handoff agreement. Unlinked or incomplete evidence retains the broad fallback.

| Links-on group | Relevant before → after | False positives before → after | Precision before → after | Recall before → after |
|---|---:|---:|---:|---:|
| Generic HCL | 4 → 5 | 4 → 3 | 50.0% → 62.5% | 80% → 100% |
| Terraform | 6 → 6 | 11 → 4 | 35.3% → 60.0% | 100% → 100% |
| Helm | 5 → 5 | 6 → 4 | 45.5% → 55.6% | 100% → 100% |

The complete 439-call comparison passes without a lost relevant note, new false
positive or lost required source span. All original 309 outcomes are identical;
the 130 infrastructure outcomes reproduce exactly in a separate fresh-store
run. Pooled links-on recall reaches 132/132 (100%); false positives fall from 70
to 60, precision rises from 65.17% to 68.75%, and correct empty-memory results
rise from 29/37 to 33/37. Both source modes still deliver 77/77 required spans.
The accepted baseline now records these gains. All 44 focused Rust tests pass
(20 symbol-link integration, 4 extractor, 9 question-shape, 6 identifier and
5 note-routing tests). The
[improvement receipt](../bench/memory/results/infrastructure-improvement-20261101.json)
records hashes, case-level changes, the rejected first trial, exact repeat,
remaining false-positive traces and baseline provenance.

These are model-free synthetic regression improvements, not universal language
accuracy. Helm and Kubernetes still use file-grounded notes; this change does not
add their symbol extractors or render configuration. Terraform still over-returns
four notes on the rationale, title and variable questions. Helm still over-returns
four notes on helper and replica questions. HCL's literal count question and
shutdown rationale retain three false positives. The next precision work needs
Helm helper/value references and individual-note relevance checks, validated with
independently labelled rationale questions; lowering the note cap risks losing
the dependency constraints these tests require.

### Helm references and rationale-note focus (1 November 2026)

Chart templates now expose static helper and render symbols, and chart values
expose addressable fields such as `Values.service.port`. Literal `include` and
`template` calls and `.Values` accesses form directed references within a chart.
References to a map include its addressable descendants; references to a leaf
exclude sibling fields. Symbol-grounded notes can therefore follow the requested
helper, template or value dependency. Existing file-grounded notes keep their
original granularity; this does not reinterpret their labels or grounding.

Helm scope narrowing requires a current, complete graph. Dynamic helpers,
custom helper contexts, `tpl`, indirect value lookups, unresolved references,
aliases and duplicate fields retain the broader retrieval fallback. Unsupported
syntax, explicit YAML tags and size/structure limits skip static extraction while
preserving text retrieval. Values are inspected through parser events without
constructing values or expanding aliases. Override files remain separate
annotations, not a calculated effective configuration. Library-chart template
resolution, rendering, values merging and field-level revision tracking remain
outside this static analysis. Chart membership participates in extraction cache
keys; charts use full reference resolution on sync until incremental dependency
closure is supported. AST schema 15 refreshes older graph records.

For model-free, single `Why ...` questions, an already retrieved note matching
one normalized duration or a three-word content phrase can focus the note set.
Matching notes, additional notes on the same resolved symbol, and ungrounded or
unresolved-symbol notes remain eligible. Unmatched paraphrases, compound
questions, semantic ranking, reranking, explicit needs and query variants retain
ordinary retrieval. This is a lexical selection heuristic, not proof of semantic
relevance; independently labelled rationale queries are still needed.

The packet exposes `memory_scope_support` and `memory_rationale_support`, including
the selection basis and withheld counts. Candidate diagnostics identify
`outside_helm_scope` and `rationale_premise_mismatch`, so a future recall miss can
be traced to the relevant selection step. The regression tests cover symbol
notes, parent values, template edits, dynamic fallback, equivalent duration units,
same-symbol additional reasons and caller-supplied expanded requests.

The graph-only trial on the unchanged 130-call infrastructure subset removed
the helper question's unrelated port note (Helm precision 55.6% → 62.5%). Adding
rationale focus produced the final 439-call results below; no fixture source, note, grounding,
question, label, request budget or scoring rule changed.

| Links-on group | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Helm | 5 → 5 | 4 → 0 | 55.6% → 100% | 100% |
| Terraform | 6 → 6 | 4 → 2 | 60.0% → 75.0% | 100% |
| HCL | 5 → 5 | 3 → 2 | 62.5% → 71.4% | 100% |
| Entire corpus | 132 → 132 | 60 → 39 | 68.75% → 77.19% | 100% |

The rationale cases also improve in TypeScript, Go, Java, Kotlin, Rust,
Kubernetes and Bash. No individual case loses a previously retrieved relevant
note, gains an irrelevant note or loses a required source span. Both source
modes retain 77/77 required spans. Links-off and counted preserve parity: their
precision rises from 67.5% to 81.8%, with recall unchanged at 54/132. Links-on
correct empty-memory results remain 33/37. These are precision gains on authored
regression cases, not universal accuracy for these languages.

All 37 focused Rust tests pass (8 extractor, 1 chart-cache invalidation, 23
symbol-link integration and 5 symbol-link unit tests). The final binary's 130
infrastructure outcomes reproduce exactly in a separate fresh-store run. The
439-call comparison passes against the previous baseline, and the accepted
baseline now records these gains. The
[Helm and rationale receipt](../bench/memory/results/helm-rationale-improvement-20261101.json)
contains the before/after metrics, intermediate graph-only result, unchanged
contract checks, per-case changes, binary/report hashes and repeatability checks.

Remaining target false positives are Terraform's handoff-contract question
(`ceiling`) and input-variable question (`interface`), plus the HCL literal-count
question (`drain`, `owner`). Helm has no false positives in these six authored
memory questions; dynamic templates and independently labelled real questions
still require broader validation. No memory cap was lowered.

### Configuration ownership, literal counts and computed Helm references (1 November 2026)

The remaining Terraform prose/input false positives came through ordinary note
ranking, before link expansion. Model-free requests naming a complete note title
now focus both ranked and linked notes on matching notes and their resolved
symbols. Unlinked or unresolved ranked evidence keeps prose retrieval broad.
A file-qualified question about the constraint governing one `var.*` input uses
that declaration's scope; a change-impact question still follows callers and
callees. Explicit needs, query variants and semantic ranking retain broad
retrieval. Packets report `memory_prose_support` (`exact_note_title`) or
`memory_scope_support` (`configuration_input`); candidate traces identify
`note_title_mismatch` and `outside_configuration_input`.

HCL count questions now reuse `memory_value_request` only after parsing the
selected source: exactly one block must match the requested kind and label,
with exactly one integer-literal `count` attribute. A comment, another block,
a computed count, ambiguous blocks, malformed or incomplete source, a missing
file, or an additional rationale clause cannot trigger this suppression.

Helm references now resolve bounded expression forms without executing Helm or
Sprig: literal `printf`/`print` helper names, immutable variable aliases,
literal-key `index`/`get`/`dig` accesses, root captures, `with` scopes, finite
`ternary` helper alternatives, and simple literal `tpl` fragments. Both possible
conditional helpers remain dependencies. Assignments that can mutate bindings
across a branch, runtime values, nonliteral `tpl`, unsupported control scopes
and external lookups remain explicit fallbacks. Value-file contents never become
assumed constants. AST schema 16 refreshes older graph records.

Nine additional authored Helm questions (27 calls) cover eight supported
expression forms and one runtime-value control. Their sources, note grounding
and closed-world labels were fixed before evaluating either binary. They extend
the suite from 160 questions / 439 calls to 169 questions / 466 calls. Results
on the original cases are compared separately from the added coverage.

| Links-on comparison | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Original Terraform cases | 6 → 6 | 2 → 0 | 75.0% → 100% | 100% |
| Original HCL cases | 5 → 5 | 2 → 0 | 71.4% → 100% | 100% |
| Entire unchanged 439-call corpus | 132 → 132 | 39 → 35 | 77.19% → 79.04% | 100% |
| Nine added Helm expression cases, same labels on both binaries | 12 → 12 | 24 → 2 | 33.33% → 85.71% | 100% |

All eight supported expression questions return only their relevant notes. The
runtime-value control still returns two unrelated value notes alongside both
potential helper notes. No effective release configuration was provided, so the
analyzer does not assume that `values.yaml` supplies the final helper name or
that a nonliteral `tpl` body is known. This part remains a conservative fallback,
not a closed precision gap for arbitrary rendered Helm releases.

The expanded baseline records 144/144 relevant occurrences, 37 false positives
and 79.56% precision. These pooled figures include new coverage and must not be
used as an improvement comparison against the old corpus. The original 439 cases
pass their prior baseline individually; no relevant note or required source span
was lost and no old case gained a false positive. Both source modes still deliver
77/77 required spans. Separate fresh-store repeats reproduce all 130 infrastructure
and 27 new-expression outcomes exactly with the same final binary.

Validation passes 49 focused Rust tests (24 symbol-link integration, 10
question-shape, 5 note-focus and 10 Helm extractor tests) and 27 Python evaluator
tests, including the binary-backed test. The
[configuration and expression receipt](../bench/memory/results/configuration-expression-improvement-20261101.json)
records the scope-only step, old/new corpus comparisons, hashes, remaining runtime
false positives and repeatability checks. The baseline was expanded only after
these comparisons passed.

### Bash scope and Python rationale precision (1 November 2026)

Path-qualified shell questions now apply their validated function dependency
scope to ordinary ranked notes as well as linked notes. This excludes a separate
same-named implementation while retaining the sourced helper and consumer
constraints. Ambiguous names without a path, dynamic command names, `eval`,
computed source paths, stale/missing graph data, expanded needs and semantic
ranking retain ordinary retrieval. Candidate diagnostics report
`outside_shell_scope`; packets identify the `shell_dependencies` basis.

Narrow shell cap-value questions use the existing source-value mechanism after
parsing the selected source. A unique arithmetic clamp must compare the requested
variable to a decimal integer and assign that same integer in its branch. Comments,
quoted examples, malformed excerpts, computed bounds, mismatched assignments,
leading-zero/octal literals, multiple caps and additional conditional writes cannot
establish this proof.
This reads a literal from source; it does not execute shell code or predict its
runtime output. Rationale and compound questions keep design-note retrieval.

A rationale focus can now begin when a matching note arrives through a symbol
link, before the linked-note delivery cap. This allows a caller/callee reason
with little lexical overlap to exclude unrelated linked notes after it is
actually retrieved. Earlier ranked evidence is retained when focus starts late.
`memory_rationale_support.anchor_route` distinguishes `ranked` from `symbol_link`.
`symbol_links.candidate_memories_before_filters` records the original linked
prefix, while `candidate_memories` records the prefix remaining before the cap.
The delivery cap itself is unchanged.

Same-symbol notes remain eligible as additional explanations unless a separately
quantified time policy has no topical overlap with the supported premise or its
matching notes. Calendar units are treated as text, never converted to assumed
durations. This lexical heuristic distinguishes the audit-retention note from
the rounding explanation while retaining unquantified reasons, related quantified
reasons and unresolved/unanchored notes. It does not infer that a bare `60` means
seconds or that an unmatched paraphrase expresses a known premise.

The unchanged 169-question, 466-call synthetic suite measures these gains with
symbol links enabled:

| Coverage | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Bash, 6 memory questions | 9 → 9 | 5 → 0 | 64.3% → 100% | 100% |
| Python, 50 memory questions | 50 → 50 | 22 → 12 | 69.4% → 80.6% | 100% |
| Entire memory corpus | 144 → 144 | 37 → 22 | 79.56% → 86.75% | 100% |

Bash removes the alternate implementation from the path-qualified change question
and four design notes from the literal-cap question. Python removes ten irrelevant
note occurrences across same-symbol, caller/callee, cross-file and transitive
rationale questions. No case loses a relevant note or gains an irrelevant one;
other-language outcomes are identical. Both source modes retain all 77 required
spans. The 186-call fresh-store repeat reproduces the corresponding full-run
outcomes exactly with the same final binary. Fixtures, labels, requests and scoring
are unchanged, and the accepted baseline now records the gains.

All 44 focused Rust tests pass: 27 symbol-link integration, 11 question-shape and
6 note-focus unit tests. The
[Bash and Python precision receipt](../bench/memory/results/bash-python-precision-20261101.json)
records comparisons, hashes, changed cases, selection diagnostics and remaining
false positives.

Python still returns 12 irrelevant note occurrences across eight questions:
direct recall, bare-number and unmatched-paraphrase rationale, partially unresolved
links, and prose-only requests. These are remaining precision gaps, not closed
cases. Default link behavior and delivery caps are unchanged; default counted-mode
recall remains 18% for Python and 55.6% for Bash. The 100% recall figures above apply
only with symbol links enabled on this authored corpus, not to general language
accuracy or production queries.

### JavaScript relevance and expanded Go/JavaScript coverage (1 November 2026)

The existing duration matcher now recognizes explicit halves and quarters of
fixed-duration units, so "three quarters of a minute" can support a rationale
about forty-five seconds. Normalization requires an exact whole number of
milliseconds; bare numbers, calendar months, signed values, unsupported mixed
fractions and sub-millisecond fractions do not acquire guessed meanings.

The existing note focus also recognizes narrow questions of the form "Which
constraints govern `sendEvent` retry timing?" (including backoff timing and
waiting interval). Focus requires an actually retrieved note with both an
explicit duration and waiting-policy terms. Its packet basis is
`retry_timing_policy`. This explicit topic scope also filters ranked notes when
the supporting policy arrives later through a symbol link; other late rationale
focus keeps its earlier ranked evidence. Broad inventories and compound requests
keep ordinary retrieval. This remains a bounded lexical heuristic, not a general semantic
relevance classifier.

Literal-call value questions now use the existing source-value filter for ranked
notes when selected source contains the graph-resolved target declaration. The
packet and candidate trace report `literal_call_value_request`. Existing linked
note suppression remains in place. Missing target source, symbolic inputs,
compound questions, semantic ranking and caller-supplied needs do not trigger
the new ranked-note filter. The filter does not execute code or prove a runtime
result.

The new Go and JavaScript fixtures add nine memory questions and four exact-source
questions per language. They cover async ES-module methods, aliased imports,
arrow helpers, Go pointer receivers and interfaces, cross-file/transitive calls,
alternate methods, literal reads, absent topics and compound questions. Both
binaries are measured against the same new labels, fixed before either run.
These remain authored, retry-focused synthetic cases; independent real-query
and broader application-domain coverage are still outstanding.

Paired comparisons with symbol links enabled:

| Same-case comparison | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Original JavaScript, 4 questions | 2 → 2 | 5 → 0 | 28.6% → 100% | 100% |
| New JavaScript, 9 questions | 9 → 9 | 13 → 4 | 40.9% → 69.2% | 100% |
| New Go, 9 questions | 9 → 9 | 10 → 2 | 47.4% → 81.8% | 100% |

On the unchanged 466-call corpus, overall links-on precision rises from 86.75%
to 89.44% (144 relevant occurrences retained; 22 → 17 false positives).
Other-language outcomes are identical. Adding the new cases produces the
195-question, 536-call baseline with 162 relevant occurrences, 23 false positives
and 87.57% precision. Those pooled figures describe the expanded population and
must not be used as a same-corpus improvement comparison.

The expanded language results are Go 90.5% precision over 16 memory questions
(19 relevant, 2 irrelevant occurrences) and JavaScript 73.3% over 13 questions
(11 relevant, 4 irrelevant). Both have 100% recall with links enabled. Default
link behavior is unchanged; counted-mode recall on the expanded fixtures is
52.6% for Go and 90.9% for JavaScript.

The new cases expose six remaining false positives across five questions:
JavaScript's qualified change question returns a diagnostic note; both languages'
alternate-method questions return an unrelated owner contract; and compound
questions retain extra owner/diagnostic notes. These gaps remain labelled in the
baseline. Broader compound requests are not narrowed to one timing explanation.

Validation passes 46 focused Rust tests (28 symbol-link integration, 11
question-shape, 6 note-focus and 1 duration unit test), the original 466-case
regression comparison, and the paired 70-call expansion comparison. A fresh-store
94-call repeat reproduces the corresponding final full-run outcomes exactly.
Both source modes deliver all 88 required spans. No existing or new paired case
loses relevant evidence or gains an irrelevant note. The accepted baseline and
suite now include the new coverage; the
[JavaScript and Go coverage receipt](../bench/memory/results/javascript-go-coverage-20261101.json)
records hashes, comparisons, selection diagnostics and remaining false positives.

### Closing the expanded Go/JavaScript findings (1 November 2026)

The six false positives exposed by the added fixtures had two retrieval causes:
ranked notes could bypass the qualified declaration scope already used for
linked notes, and compound rationale questions bypassed note focus entirely.

Qualified change requests now apply the existing dependency and enclosing-owner
scope to ranked notes, with packet basis `qualified_change` and candidate reason
`outside_qualified_change`. Unknown declarations, caller-supplied needs and
semantic ranking keep ordinary retrieval. Dotted Helm helper names remain on
the existing chart-specific route and retain its symbol-links switch.

Before narrowing this scope, the JS/TS resolution pass was extended to record
imported calls from direct named methods of top-level classes. An aliased call
such as `retryWait()` now connects `NoticeSender.send` to the imported `pauseFor`
arrow function. Previously lexical ranking rescued that relevant note despite
the missing call edge. Parameter and local-binding shadows do not produce this
import edge. The fresh/cached extraction test covers both the method caller and
shadow controls. AST cache schema 17 invalidates older extraction entries on
the next extraction; existing projected graphs need an index refresh to gain
the corrected edge.

A two-part rationale can focus only when both clauses have actually retrieved
support, using the union of ranked and linked candidates. Explicit durations or
consecutive content words support each clause. The union of both anchor notes
and their symbols remains eligible, including additional reasons on the same
symbol. Missing support for either clause, negation, disjunction, additional
subjects and more than two clauses keep broad retrieval. The packet basis is
`rationale_compound_agreement`; the rule remains a lexical heuristic rather than
a semantic proof of relevance.

On the unchanged 195-question, 536-call corpus, with symbol links enabled:

| Coverage | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Go, 16 memory questions | 19 → 19 | 2 → 0 | 90.5% → 100% | 100% |
| JavaScript, 13 memory questions | 11 → 11 | 4 → 0 | 73.3% → 100% | 100% |
| Entire memory corpus | 162 → 162 | 23 → 17 | 87.57% → 90.50% | 100% |

All six exposed false positives are removed. No paired case loses a relevant
note, gains an irrelevant note or loses required source evidence. Other-language
outcomes are identical, and both source modes retain all 88 required spans.
Defaults and delivery caps are unchanged: counted-mode recall remains 52.6%
for Go and 90.9% for JavaScript. The full-recall claims above apply only to this
authored corpus with links enabled; 17 false positives remain in other languages.

Validation passes 37 focused Rust tests (29 symbol-link integration, 7 note-focus
unit tests and 1 fresh/cached method-alias extraction test). The full 536-call
run passes the previous baseline; a separate 70-call fresh-store replay exactly
reproduces the corresponding full-run outcomes with the same binary. Fixture
files, questions, labels, requests and scoring are unchanged. The accepted
baseline now records the gains, and the
[Go/JavaScript findings receipt](../bench/memory/results/go-js-findings-20261101.json)
retains the paired comparisons, hashes and selection diagnostics.

### Python/Kubernetes factual precision and pilot follow-ups (1 November 2026)

Exact note-title focus now accepts a trailing `for ...` qualifier only when all
of its substantive terms occur in the matching note body. Existing full-title
matching remains intact, including titles containing `for`. Unknown qualifiers
and compound requests retain broad retrieval.

A narrow expiry question can focus on an actually retrieved three-word subject
phrase. It keeps same-symbol qualifications that mention the subject, such as
early lease expiry during maintenance, while withholding unrelated scheduling
policy. Unanchored and unresolved note grounding remains eligible. This does not
infer a unit for a bare number or map tenant/customer paraphrases by assumption.
Packets identify `factual_subject_phrase`; candidate traces report
`note_topic_mismatch`.

YAML field questions use the requested file and field to anchor matching notes;
another policy in the same file is not automatically about that field. The basis
is `named_field_topic`. Broad file-change and multi-field requests remain broad.
The existing absent-topic guard now also recognizes attributes of explicitly
named Kubernetes resources, separating the requested attribute from the resource
name. A phrase such as "the deployment" alone does not establish a resource name.
Any topical support in the authorized notes or source preserves retrieval, and
expanded/semantic requests keep their existing behavior.

Remaining cases stay in the baseline with their original relevance labels.
Missing symbol identities, unsupported prose/semantic matches and unknown Helm
render inputs require more evidence or separately validated retrieval changes.
The current Helm graph does not certify that known local helpers exhaust the
runtime helper namespace, so enumerating only those helpers is not a complete
replacement for effective chart/release inputs.

The pilot follow-up register is an offline benchmark artifact, not deployed
Sentinel telemetry. For real pilot requests, record the repository snapshot and
binary, effective switches/needs/query variants, independently labelled evidence,
candidate routes and exclusions, omissions/budgets/errors, and applicable Helm
render inputs. Evaluate the actual Sentinel request configuration: semantic
ranking and expanded requests may bypass these model-free filters.

The unchanged 536-call comparison, with symbol links enabled, measures:

| Coverage | Relevant before → after | False positives before → after | Precision before → after | Recall |
|---|---:|---:|---:|---:|
| Python, 50 memory questions | 50 → 50 | 12 → 9 | 80.6% → 84.7% | 100% |
| Kubernetes, 6 memory questions | 5 → 5 | 3 → 0 | 62.5% → 100% | 100% |
| Helm, 15 memory questions | 17 → 17 | 2 → 2 | 89.5% → 89.5% | 100% |
| Entire memory corpus | 162 → 162 | 17 → 11 | 90.50% → 93.64% | 100% |

The six removed false-positive note occurrences span four questions: qualified
settlement contract, upstream lease expiry, Service selector contract, and absent
backup-region requirement. Eleven false positives remain across seven questions:
five from incomplete symbol scope, three from unsupported prose/semantic scope,
one from the unstated duration unit, and two from the unbound Helm helper.
Their relevance labels and conservative fallback behavior are unchanged.

All 52 focused Rust tests pass (31 symbol-link integration, 9 note-focus and 12
question-shape unit tests). The full run passes the prior baseline with no lost
relevant notes, added irrelevant notes or lost source spans. Both source modes
retain 88/88 required spans, and the separate 171-call fresh-store replay matches
the corresponding full-run outcomes exactly with the same binary. Defaults and
delivery caps are unchanged; pooled counted-mode recall remains 50%, with Python
at 18%. The accepted baseline records the gains. The
[pilot gap tracker and measurement receipt](../bench/memory/results/pilot-gaps-20261101.json)
contains closed/open status, required follow-up evidence, comparisons, hashes,
candidate-filter diagnostics and the recommended pilot capture fields.

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
