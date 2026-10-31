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

The checked-in suite contains 54 authored memory questions and six authored
source questions. Memory questions run with symbol links off, counted (the
default), and on. Source questions run with identifier routing off and on. These
are **174 context calls**, covering relevant decisions, distracting memories,
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
case gained an irrelevant ID. The original baseline and corpus labels remain
unchanged. The scope change alone removed 39 irrelevant occurrences; the
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
original report hash and protocol, and removes timings from the comparison
baseline. Baseline updates belong in a reviewed change with the old comparison
and a reason for every accepted tradeoff, never as an automatic response to red
CI. A supplied manifest (`--suite`) can select additional memory corpora in the
existing format; source fixtures use the inline `quality_sources.json` format.

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
