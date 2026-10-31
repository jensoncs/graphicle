# Graphicle memory benchmark kit

Retrieval and answer benchmarks for Graphicle's memory, run through the real
`graphicle` program. This folder is the first part of the public benchmark
kit: the tools that measure the retrieval options of the runtime README (its
"Retrieval defaults" and "Benchmark kit" sections) and the transport the
answer evaluations use.

| Tool | What it measures | Model calls |
|---|---|---|
| `tools/symbol_link_precision.py` | Paired symbol-links off/counted/on: reviewed-memory precision, recall, false positives, correct empty-memory results, and context tokens. Includes synthetic coding regression fixtures; accepts separately labelled held-out projects | None by default; explicit local reranker optional |
| `tools/symbol_link_cutoff.py` | Calibrates a raw-score cutoff using development-only instrumented runs, with per-query recall constraints and a frozen selection | None; replays recorded scores |
| `tools/retrieval_recall.py` | LongMemEval-S/M retrieval recall. Each question's sessions are synced into a fresh store, and the `context` packet is scored against the dataset's support labels: session recall, all-sessions recall, turn coverage and tokens, with McNemar tests for `--paired` variants | None; dense and reranked variants run local ONNX models |
| `tools/qualify_longmemeval.py` | The frozen LongMemEval-S screen (`freeze`, `run`, `packets`, `score`), and the coverage scorer `retrieval_recall.py` and `amb/recall.py` use | Its `run` phase calls the configured answering model |
| `tools/amb/graphicle_provider.py` | A `graphicle` provider for an unmodified [Agent Memory Benchmark](https://github.com/vectorize-io/agent-memory-benchmark) (AMB) install, whose loaders, prompts, models and judging stay AMB's | AMB's reader and judge |
| `tools/amb/recall.py` | Retrieval recall on the documents AMB's loaders produce, per variant and AMB category | None |
| `tools/amb/qa.py` | AMB's answer and judge steps over the saved packets of an `amb/recall.py` run | AMB providers or local MLX models |
| `tools/expand_queries.py` | Model-written query variants, written ahead of time for `retrieval_recall.py --query-variants` (the `mq-` variants); `query_expansion_prompt.txt` is its prompt | Any OpenAI-compatible server |
| `tools/azure_memory_eval.py` | An auditable Azure OpenAI batch transport with content-addressed receipts, for answer evaluations | Azure OpenAI |

## What a run needs

- A `graphicle` binary. The tools default to the runtime crate's builds
  (`crates/graphicle/target/debug/graphicle` for `retrieval_recall.py`, the
  release build for `amb/recall.py`); pass `--binary`, or set
  `GRAPHICLE_BIN` for the AMB provider and the tests. Dense and reranked
  variants need a build with `--features local-inference` and local ONNX
  model folders (`--embedder`, `--reranker`).
- The datasets, which are not shipped. `retrieval_recall.py` reads the
  cleaned LongMemEval release from `--dataset` (`qualify_longmemeval.py` pins
  its SHA-256). AMB is imported from a checkout at run time (`--amb`); its
  repository had no licence at the pinned commit, so nothing is copied from
  it.
- For model-calling tools, credentials in the process environment, never on
  the command line: `azure_memory_eval.py` reads `AZURE_OPENAI_BASE_URL` and
  `AZURE_OPENAI_API_KEY1`.

Write run outputs under `runs/` (ignored here) or outside the repository.
The recall runners freeze a protocol with the SHA-256 of the binary, the
dataset and their own sources before any outcome is recorded; the answer
tools record the hashes of their prompts, provider and transport.

```sh
python3 tools/retrieval_recall.py --binary /abs/graphicle \
  --dataset /abs/longmemeval_s_cleaned.json --variants l12-lexfq,lex-rrf \
  --paired l12-lexfq,lex-rrf --out runs/lme-s
```

## Variants

`retrieval_recall.py` names its requests (`VARIANTS`): budgets are explicit,
and options a binary may default are sent as off unless a variant sets them
(`RECIPE_OFF`), so a name means the same request on every binary. A current
binary refuses retrieval options it does not support.

## Tests

```sh
GRAPHICLE_BIN=/abs/graphicle python3 -m pytest tests
```

Tests that need the binary are skipped without one. The AMB tests run without
AMB, on synthetic stand-ins for its loaders.

## Symbol links and recall precision

```sh
python3 tools/symbol_link_precision.py --binary /abs/graphicle \
  --out runs/symbol-links-regression
```

This creates a disposable store per project, syncs code, writes the fixture's
reviewed decisions with the public `note` operation, applies any subsequent
code changes, and queries the real `context` operation. Labels never enter the
store. Each question uses the same store and budget for three variants:
`symbol_links: false`, omitted (counted), and `true`. Execution order rotates.
The identifier shortcut is disabled so every result uses the ordinary context
packet; document links and cross-repository expansion are also disabled.
The exact request is recorded in `PROTOCOL.json`. No embeddings, reranker,
answer model, or judge is used. Production retrieval defaults are not changed.

The bundled [corpus](fixtures/symbol_link_precision.json) has 18 hand-authored
queries across four small Python projects. It includes relevant linked
decisions, direct lexical recall, an irrelevant decision on the same symbol,
code-only questions, unrelated questions, and decisions invalidated by a code
change. Labels and their rationales were written before the first run. These
are **synthetic regression cases**, not independent held-out coding sessions;
their aggregate scores cannot justify enabling links by default.

`RESULTS.json` includes individual results and aggregates by category:

- `micro_precision_completed`: relevant returned memory IDs divided by all
  returned memory IDs, over completed calls. Undefined when none are returned.
- `macro_recall`: mean fraction of relevant memory IDs retrieved, over all
  queries with positive labels. Failed calls contribute zero.
- `empty_memory_accuracy`: fraction of queries with no relevant memory that
  return no memories. Failed calls contribute zero. A code-only query can
  correctly return code; this metric does **not** assert empty context or
  answer abstention.
- False-positive memory counts and mean total packet tokens. Tokens include
  code and formatting; they are not an estimate of irrelevant-memory tokens.
- Paired on-minus-off changes in true positives, false positives and tokens,
  with the number of incomplete pairs explicitly excluded.

Precision is at memory-ID level: a returned excerpt may still omit information
needed for an answer. Unknown or duplicate returned IDs, malformed packets,
model calls, and budget overruns are errors. Raw requests and packets are kept.
Counted and off must preserve delivered content; a mismatch fails the run.
Poor retrieval scores are measurements, not runner failures. Stores are removed
after evaluation; results contain memory text, so keep private datasets' output
private. Existing nonempty output directories are rejected.

For a real held-out evaluation, use the same JSON format and pass
`--dataset /abs/labelled-projects.json --split held_out`. Assign entire projects
to `development` or `held_out`, freeze code and decisions as they existed before
the query, and have relevance labels reviewed without seeing retrieval results.
`relevant_memories` must enumerate every relevant fixture memory; everything
else is counted as irrelevant. Preserve query provenance and label rationale.
Do not tune queries, labels, thresholds, or budgets on the held-out results.
The runner freezes dataset, binary and source hashes before querying, but cannot
verify the independence of externally supplied labels. No held-out real-session
dataset is bundled.

### Initial synthetic screen: 30 October 2026

All 54 context calls completed on a fresh default-feature debug build of
Graphicle 0.2.0. The counted variant preserved off's delivered content for
all 18 queries. The two stale-memory queries returned no memories in every
variant. [Receipt, hashes and per-query results](results/symbol-links-regression-20261030.json).

| Metric | Off | Counted (default) | On |
|---|---:|---:|---:|
| Memory precision, pooled over returned IDs | 85.7% | 85.7% | 34.2% |
| Mean recall over 10 positive queries | 55.0% | 55.0% | 100.0% |
| Correct empty memory over 8 negative queries | 100.0% | 100.0% | 62.5% |
| Irrelevant memories returned | 1 | 1 | 25 |
| Mean packet tokens | 216.4 | 216.4 | 309.3 |

Links recover useful constraints but also add irrelevant decisions. For example,
the code-only `invoice_total([10, 20])` question receives four memories with links
on, including receipt migration and freight policy. The link step considers
symbols overlapping a selected code window; these short fixture files make
neighboring declarations easy to include. A valid link establishes a connection
to code, not relevance to the question. This screen supports retaining the
counted default while evaluating query relevance for linked candidates on
independently labelled real sessions. It does not estimate production accuracy,
answer quality, or latency.

### Query-ranking trial: rejected and reverted

A follow-up trial passed the additional linked-memory candidates through the
existing query ranker before packing. It reused the query's lexical/semantic
arms and settings, preserved memory records and link provenance, and introduced
no thresholds or default changes. The corpus, labels and runner were unchanged.
The acceptance criteria required better precision, retention of each relevant
memory recovered by the original links-on variant, and recovery beyond off.

| Metric | Counted default | Original links on | Query-ranked links on |
|---|---:|---:|---:|
| Memory precision | 85.7% | 34.2% | 85.7% |
| Mean recall | 55.0% | 100.0% | 55.0% |
| Correct empty memory | 100.0% | 62.5% | 100.0% |
| Irrelevant memories | 1 | 25 | 1 |

The trial lost seven relevant memory occurrences across five queries and
recovered no additional relevant memory beyond off. For example, querying the
constraints on `retry_delay` lost both the upstream lease and deferred-jitter
decisions. The existing ranker already finds the directly matching memories;
applying it again removes the differently worded decisions that links recover.
The additional implementation therefore failed the quality criteria and was
reverted, along with its trial-specific unit tests. The counted default and
original opt-in link behavior remain intact.

All 54 measured calls completed, counted preserved off's delivered content,
and stale memories stayed excluded. Two candidate unit tests passed before the
trial; all 19 evaluation/scorer tests passed against the preserved baseline
binary. The restored binary's hash matches the initial receipt, and the runtime
source has no retained diff. The
[rejected-trial receipt](results/symbol-links-ranked-rejected-20261030.json)
contains the protocol, acceptance checks, per-query results, and an inert copy
of the attempted patch for reproduction. This remains a synthetic regression
screen; semantic reranker quality and real-session effectiveness were not tested.

### Exact-symbol trial: rejected

The next trial reused the identifier route's query-name classification and the
identifier arm's definition resolver. It restricted code-to-memory expansion
to the explicitly named declarations only when every name resolved uniquely.
Explicit file paths could disambiguate definitions. Missing, ambiguous or
inaccessible names kept the existing window-based expansion. Counted/off and
memory-to-code expansion were unchanged.

Alongside the original 18 queries, eight fresh
[challenge queries](fixtures/symbol_link_exact_scope.json) exercised qualified
names, duplicate names and file basenames, multiple names, partially unresolved
names, prose and a relevant callee decision. Their labels were frozen before
either baseline or candidate measurement. They are hand-authored synthetic
cases, not independently labelled real-session holdout data.

| Links-on metric | Original corpus: before | Original corpus: trial | Challenge: before | Challenge: trial |
|---|---:|---:|---:|---:|
| Memory precision | 34.2% | 43.3% | 54.5% | 68.8% |
| Mean recall | 100.0% | 100.0% | 100.0% | 87.5% |
| Irrelevant memories | 25 | 17 | 10 | 5 |

All 78 candidate context calls and 24 new challenge-baseline calls completed.
Counted preserved off's delivered content, off/counted memory results stayed
unchanged across builds, and the original stale-memory cases stayed excluded.
However, the trial failed the predeclared requirement to retain relevant
memories on the challenge set: asking why `send_packet` caps backoff at sixty
seconds lost the peer-session-lifetime decision attached to `backoff_delay`,
which the named function calls. The original links-on behavior returned it.

The result supports using exact symbol identity as a potential ranking signal;
it does not support excluding all other symbols' memories from general context.
Any future experiment must preserve relevant dependency context. The production
trial was reverted; the challenge corpus and
[comparison receipt](results/symbol-links-exact-rejected-20261030.json) remain.
All eight symbol-link integration tests passed after correcting a duplicate-file
test fixture that had triggered existing content deduplication. The benchmark
inputs were unchanged. Runtime source and the baseline executable were restored.

### Exact-symbol priority trial: rejected

This trial used the same name resolver to give exact-symbol linked memories
stable priority **before** the four-memory cap. Other symbols' memories remained
eligible. Equal-priority candidates retained their original order; a memory
linked to several symbols could gain priority from a later exact match without
occupying a second slot. Freshness, authorization, one-hop expansion and the
existing limits were retained. Counted/off did not resolve query symbols.

Both previous corpora were rerun, alongside 11 new
[capacity-pressure queries](fixtures/symbol_link_priority.json), whose labels
and acceptance criteria were frozen before measurement. These cover six direct
targets, caller/callee competition, a memory with multiple links, and unresolved
or prose queries. They remain synthetic tests, not a real-session holdout.

| Links-on metric | Original behavior | Exact-symbol priority |
|---|---:|---:|
| Original 18-query corpus: precision / recall | 34.2% / 100% | 34.2% / 100% |
| Prior 8-query challenge: precision / recall | 54.5% / 100% | 54.5% / 100% |
| New 11-query pressure set: precision | 25.0% | 27.3% |
| New pressure set: mean recall | 69.7% | 78.8% |
| New pressure set: irrelevant memories | 33 | 32 |
| Direct-target pressure subset: mean recall | 66.7% | 100% |

The candidate recovered two direct-target notes previously crowded out by
neighbors. But it lost the relevant `backoff_delay` session-lifetime note when
asking why `send_packet` caps backoff at sixty seconds: four unrelated notes
linked directly to `send_packet` took all four slots. Both return statements
were present in the packet. This is a ranking/cap regression, independent of
the separate RBAC graph-size and Nexus evidence-packing blockers. Without
capacity pressure, the earlier callee challenge remained successful.

Keeping candidates eligible therefore does not guarantee they reach the final
packet. Aggregate recall improved while one useful dependency memory was lost,
failing the predeclared per-query no-loss criterion. The runtime change and its
trial-specific Rust tests were reverted; the original counted default and
opt-in links remain. The fixtures and
[comparison receipt](results/symbol-links-priority-rejected-20261030.json),
including an inert reproduction patch, are retained.

All 222 paired context calls completed. Counted preserved off's packet content;
off/counted memory results and fallback ordering were unchanged across builds,
and stale memories remained excluded. All nine candidate symbol-link integration
tests passed after removing accidental numeric lexical matches from test note
titles; benchmark fixtures and labels were unchanged. All 19 harness/scorer tests
passed against the baseline binary. Runtime source was restored and the restored
executable's SHA-256 matches the original baseline. These results measure memory
IDs, not answer quality or production effectiveness.

### Caller/callee diversity trial: fixes crowding, introduces another loss

Debugging the priority failure showed five eligible notes competing for four
slots. The callee's lease note was current, visible and linked correctly; both
return statements were packed. Exact-symbol sorting moved four irrelevant
direct notes ahead of it, so the memory cap dropped it before packing.

A follow-up prototype resolved the named symbols and their immediate callers
and callees with the existing code graph. It offered one note per such symbol
per round, named symbols first within a round, before more notes from one
symbol. Other candidates remained eligible afterward. Missing, ambiguous,
lagging or unreadable graph resolution retained the original order. The graph
lookup respected visible documents, and graph-off queries did not use it.
No limits, default switches or memory eligibility rules changed.

The three previous corpora were unchanged. Three new
[countercases](fixtures/symbol_link_diversity.json) were labelled before running
any version: four useful direct contracts competing with an irrelevant callee
note, a cross-file callee rationale, and a two-hop rationale. Production sources
were not edited; the prototype was built from the pinned HEAD in a temporary
checkout, excluding concurrent workspace changes.

| Links-on metric | Original | Exact priority | Caller/callee diversity |
|---|---:|---:|---:|
| 11 pressure queries: precision | 25.0% | 27.3% | 31.8% |
| 11 pressure queries: mean recall | 69.7% | 78.8% | 97.0% |
| 11 pressure queries: irrelevant notes | 33 | 32 | 30 |
| 3 fresh countercases: precision | 66.7% | 66.7% | 55.6% |
| 3 fresh countercases: mean recall | 100.0% | 100.0% | 91.7% |

The prototype retained every previously retrieved relevant note on the old
37-query sets, fixed the crowded-out callee, and recovered an additional caller
note. Precision and recall on the original 18-query and eight-query challenge
sets were unchanged. Cross-file and two-hop rationale retrieval also survived.

However, for `encode_payload` output compatibility, an irrelevant tracing note
on its callee replaced the useful trailing-newline requirement attached directly
to the named function. That query's recall fell from 4/4 to 3/4. Symbol diversity
therefore moves the tradeoff: a call relationship does not establish whether a
particular decision answers the question. The prototype failed the no-loss
criterion and was **not adopted**. Its
[receipt and reproduction patch](results/symbol-links-diversity-rejected-20261030.json)
are retained, and the baseline executable was restored with the original hash.

All 120 candidate calls and 18 new baseline/priority calls completed; previous
frozen runs supplied the other comparisons. All 11 Rust checks passed (ten
symbol-link integration tests and a saved/resident graph lookup parity test
that also checks file visibility). Counted preserved off's packet content;
off/counted memory results and fallback ordering were unchanged, the four-note
cap held, and stale notes stayed excluded. All datasets remain synthetic.

**Next precision experiment, not yet measured:** reuse the local cross-encoder
for expanded linked-note candidates before the four-note cap, giving it the
question, note text and linking code/call-path context. The current semantic
rerank in `query.rs` precedes `symbol_links::lift`, so newly lifted notes bypass
that scoring step. A structural prior can help preserve weakly worded but
relevant candidates, without deciding relevance by exact identity or a fixed
allocation. This differs from the rejected lexical query-ranking trial. It
requires an explicit local reranker profile and a local-inference build; these
zero-model-call tests do not establish its effectiveness. Independently labelled
queries are needed to validate it before any threshold or default changes.

### Semantic scoring before the memory cap

Linked-memory candidates now use the existing local cross-encoder when both
`symbol_links: true` and a local reranker profile are explicitly configured.
The original source rerank runs first. The link step then collects an eligible
prefix of up to `max(4, rerank_depth)` memories, scores each note with its title,
body, linked symbol and supplying selected code window, applies the existing
`rerank_mode`, and finally takes four. The note precedes the bounded code context
in the model input. There is no score threshold, exact-symbol boost or diversity
quota. Without a reranker profile, the existing behavior is unchanged.

The comparison froze all 40 queries, labels, model assets and input rendering
before querying. Both baseline and candidate builds enabled local inference and
used the same configured global reranker, isolating the additional linked-note
pass. The model was `cross-encoder/ms-marco-MiniLM-L6-v2`, pinned to revision
`233902d25c440f23af6f7d6e94d2946bac0bee0a`, using its ARM64 int8 ONNX weights,
a 256-token limit and `blend` mode. ONNX Runtime 1.22.1 and all model files were
digest-pinned. No remote model or answering model was used.

| Links-on metric | Global reranker only | Plus linked-note reranking |
|---|---:|---:|
| Original 18 queries: precision / recall | 34.2% / 100% | 34.2% / 100% |
| Eight-query challenge: precision / recall | 54.5% / 100% | 54.5% / 100% |
| 11 pressure queries: precision | 25.0% | 31.8% |
| 11 pressure queries: mean recall | 69.7% | 97.0% |
| 11 pressure queries: irrelevant notes | 33 | 30 |
| Three countercases: precision / recall | 66.7% / 100% | 66.7% / 100% |

Both previous failures are avoided: the crowded-out callee rationale survives,
and all four payload-format requirements survive while the irrelevant callee
tracing note stays out. Two direct-target notes and one caller note are newly
recovered. No query's measured precision or recall declines.

There is an explicit qualification to the frozen acceptance plan: its strict
requirement to retain every previously retrieved relevant memory ID is **not
met**. The broad `shared-pressure/prose` question has six relevant notes and four
slots; the candidate replaces the relevant ledger note with the equally relevant
probe note. Both versions retrieve four of six. The implementation is retained
as an **opt-in improvement with this identity tradeoff**, not as an exact
retention guarantee. Precision improves only on the pressure set, rather than
strictly improving on every corpus. The [receipt](results/symbol-links-semantic-20261030.json)
records these qualifications, all gains/losses, hashes and per-query results.

All 240 primary context calls completed, counted preserved off's delivered
content, off/counted memory results were unchanged across builds, and stale
notes remained excluded. The final implementation also preserves the four-note
allowance when source `rerank_depth` is below four; a dedicated real-model CLI
check covers that boundary. That correction does not change the primary
benchmark's depth of 32. The receipt distinguishes the measured and final binary
hashes. The harness supports optional `--reranker` and `--ort` arguments, verifies
asset digests before querying, and continues to reject remote model calls.

These are synthetic memory-ID results. They do not establish answer quality,
production effectiveness, or whether a relevance threshold would help. Ranking
still fills available slots: it does not remove unnecessary notes on the
original code-only questions. Model input truncation and the candidate prefix
remain bounds, and local reranking adds CPU work. The default remains counted,
and no local model is downloaded or enabled by runtime queries.

Example, using a build with `local-inference` and explicitly prepared assets:

```sh
python3 tools/symbol_link_precision.py --binary /abs/graphicle \
  --dataset fixtures/symbol_link_priority.json \
  --reranker /abs/pinned-reranker --ort /abs/libonnxruntime.dylib \
  --rerank-mode blend --out runs/symbol-links-local
```

### Relevance cutoff trial: rejected

The next experiment calibrated a single raw-logit cutoff for newly linked notes.
The accepted semantic-ranking implementation, model, renderer, blend weights,
candidate pool and four-note cap stayed fixed. The prototype filtered scores
below the cutoff in the already computed blended order, then applied the cap.
Ordinary retrieval memories were unaffected. Raw scores were not treated as
probabilities, and no production cutoff option was introduced.

The original 18 queries were development data. The calibration swept observed
score boundaries, requiring every positive development query to retain at least
its baseline true-positive count. It maximized micro precision, breaking ties
by more true positives and then the lower cutoff. Selection was frozen at
**−5.931154251098633** before evaluating the other 22 existing regression
queries or eight [new held-out synthetic queries](fixtures/symbol_link_cutoff_holdout.json).
The new projects cover Python pagination and JavaScript webhook timing; they
were held out from threshold selection, not independently labelled real sessions.

| Set | Precision: no cutoff → cutoff | Mean recall: no cutoff → cutoff |
|---|---:|---:|
| Development, 18 queries | 34.2% → 44.8% | 100% → 100% |
| Exact-scope challenge, 8 queries | 54.5% → 57.1% | 100% → 100% |
| Capacity pressure, 11 queries | 31.8% → 27.0% | 97.0% → 90.9% |
| Earlier countercases, 3 queries | 66.7% → 62.5% | 100% → 66.7% |
| New held-out projects, 8 queries | 25.0% → 25.0% | 100% → 80.0% |

The original same-file callee rationale, crowded caller/callee rationale cases,
and all four payload-formatting notes passed their explicit safeguards. However,
three other queries lost recall:

- The broad pipeline question lost all four previously delivered relevant notes.
  Its six eligible relevant notes scored around −11, below the learned cutoff.
- The cross-file caller question lost its callee's lease rationale, scored
  **−6.331**, while retaining an irrelevant caller audit note scored **−3.233**.
  Both function return statements remained in the packet.
- The held-out pagination change question lost its relevant upstream capacity
  contract, scored **−8.691**.

The cutoff was not lowered or retuned after these failures. This model's raw
scores do not provide a reliable global relevance boundary across these question
styles and linking contexts. The cutoff is **not adopted**. The previously
accepted opt-in semantic ranking is preserved, and its executable was restored.
The [receipt](results/symbol-links-cutoff-rejected-20261030.json) contains the frozen
selection, full development sweep, evaluation results, model/source hashes and
an inert instrumentation patch for reproduction.

All 288 CLI context calls completed. Every one of the 48 cutoff packets matched
its score-replay prediction exactly. Counted/off controls were unchanged. Six
Rust integration tests and 12 calibration tests passed, including rejection of
held-out or mismatched calibration inputs and refusal to overwrite a frozen
selection. The experiment failed the retrieval-quality criteria, rather than
execution or replay correctness.

`tools/symbol_link_cutoff.py` requires the authorized candidate ID/title/score/rank
diagnostics emitted by the archived experimental patch. Its no-cutoff replay
must reproduce the actual packet before it will calibrate, so packing losses
cannot silently become calibration assumptions. It accepts only a matching
`development` run and refuses to overwrite an existing selection file. A frozen
cutoff must still be tested through the real CLI on separate evaluation data.

## Not here yet

The answer-level harness of the October 2026 evaluations (case builders, the
`observer.context` renderer, paired statistics, each run's protocol and
results) and the BEAM recall runner are still research tools; they join this
kit with the published evaluation.
## Real-source query seed, completeness, routing and tighter note context

The 2026-10-30 trial establishes a small provenance-labelled source-delivery
suite in `fixtures/real_query_seed.json`: two captured diagnostic tool queries
and nine authored questions over historical Nexus code. These are **not eleven
independently collected human queries**. Nexus #963 is development/regression;
four questions from Nexus #944 are a separate PR holdout for this experiment,
although that PR was used in earlier investigations. No real reviewed-note
corpus was available, so memory precision remains measured on synthetic data.

Each source file is hash-pinned. Each required evidence span includes exact
bytes, its quote and line; repeated quotes require a declaration anchor.
`tools/real_query_eval.py` verifies delivered hashes and byte ranges, and checks
every required span. Only location questions may pass with a definition
citation; behavior and rationale questions require source text. Labels do not
enter requests. Retrieval uses source slices, not full repository indexes.

Results (`results/real-query-routing-note-context-20261030.json`):

| Check | Baseline | Candidate | Decision |
| --- | --- | --- | --- |
| Real-source evidence completeness | 11/11 | 11/11 with identifier routing | Existing route is useful |
| Four location questions, total tokens | 9,471 | 412 | 95.6% fewer tokens; all locations retained |
| Seven behavior/rationale questions | 7/7 complete | 7/7 complete | Keep retrieval for these intents |
| 48 synthetic queries, note precision | 37.59% | 36.09% with tighter context | Reject tighter context |
| Synthetic macro recall, 37 positive queries | 99.10% | 93.69% | Reject: two callee-rationale regressions |

The existing `identifier_route: true` switch recognizes location intent; no new
classifier or production default was added. It avoided four local reranker
calls on the real-source suite. All 48 synthetic queries retained the same
memory IDs with routing enabled. The tighter-context candidate restricted the
note scorer's code to the intersection of the linked symbol and its already
authorized window. It lost the lease rationale in both crowded caller/callee
cases. The same-file callee and all four formatting notes survived. The
candidate was never applied to product code; its patch is retained as
`results/symbol-link-tight-context-rejected-20261030.patch`. Keep the previously
accepted semantic scorer's full authorized source window.

Evidence completeness fixes already existed in the working tree, so this trial
reused them. An independent audit verified all 158 delivered source excerpts in
four saved full-snapshot replay packets. Nexus retains the caller read,
publication and callee failure return at both heads, plus 21/21 and 23/23 required
head declarations. RBAC retains 55/55 at both heads. The old saved Nexus packets
lacked the callee return. This was an audit of saved packets and matching tested
source hashes, not a new impact/reviewer replay. Global dependency coverage is
still incomplete; source delivery does not prove reviewer accuracy.

Reproduce with pinned local model assets and a binary built from
`crates/graphicle` (its Cargo configuration bundles the parsers):

```sh
python bench/memory/tools/real_query_eval.py \
  --dataset bench/memory/fixtures/real_query_seed.json \
  --binary /path/to/graphicle --out /tmp/real-query-run \
  --reranker /path/to/model --ort /path/to/libonnxruntime.dylib

python bench/memory/tools/query_ablations.py \
  --binary /path/to/graphicle --arm baseline --out /tmp/note-routing-run \
  --reranker /path/to/model --ort /path/to/libonnxruntime.dylib
```

Historical checkouts are external inputs. Use repeatable
`--source-root PROJECT=DIR` overrides if their saved local paths have moved;
every source hash must still match. To reproduce the rejected experiment,
apply its patch to an isolated copy of the baseline source, build it, and run
`query_ablations.py --arm tight` with a new output directory. The baseline
receipt pins source hashes, both executable hashes, datasets and model assets.

Validation: 188 scored context calls, 42 Python checks, three candidate Rust
unit checks and six symbol-link integration checks passed; two optional Python
checks were skipped (the CLI paths were exercised by the full runs). Zero
remote-model calls. Concurrent local work means timings are not a controlled
latency comparison. An initial build lacked bundled Python parsers and its
setup failures were excluded; final trials used the correct bundle. One
development label initially selected an identical publication call in another
function; it was corrected with a declaration anchor, a regression check was
added, and the complete real-source suite was rerun without changing queries or
policies. Raw protocols, packets, logs, correction history and the independent
source audit are at `/private/tmp/graphicle-query-eval-20261030/`.

## Caller-plus-callee scoring and full-repository retrieval

The follow-up trial on 2026-10-30 tested both hypotheses separately, with the
same pinned local MiniLM, blend, candidate limits and recall safeguards.
Receipt: `results/caller-callee-full-repository-20261030.json`.

**Paired scoring context was not adopted.** For a linked note, the isolated
candidate selected one recorded incoming/outgoing call, preferring a pair that
names a query identifier, then deterministic edge order. Both declarations had
to resolve uniquely in current, hash-matched authorized documents. The scorer
received caller and callee bodies within the existing 2,000-character allowance
(bounded prefixes/tails for long bodies); absent a usable pair, it kept the
original window. It changed neither note eligibility nor answer evidence.

Across 48 synthetic queries, paired context was exercised on 28 candidates in
10 queries. Precision stayed **37.59%**, macro recall stayed **99.10%**, and all
callee-rationale and four-formatting-note safeguards passed. First-relevant-note
MRR slipped from 0.8829 to 0.8761. The cross-file callee note moved ahead of an
irrelevant caller note, but other ordering changes offset that improvement.
The candidate remains only in
`results/caller-callee-scoring-not-adopted-20261030.patch`.

The fixed-output-count oracle is already 50 relevant notes out of 133 returned:
the baseline delivers every relevant ID that can fit at those per-query result
counts. Reordering alone cannot improve this suite's pooled precision. Testing
whether fewer notes should be returned needs separate evaluation; the previously
rejected global logit cutoff is not reinstated.

**Full-repository retrieval exposed source-delivery failures.** The same eleven
frozen questions were replayed over complete Git trees at three historical
Nexus commits. Normal source policy indexed 2,298, 2,299 and 2,300 documents,
versus 3, 3 and 1 in the slices. The graphs each have about 97,000 nodes and
139,000 edges. Source manifests, tracked-ignored files, indexed hashes and
omissions were recorded; required files match the original frozen hashes.
Credential paths and symlinks were excluded before reads; normal binary, size
and document-conversion exclusions still apply.

| Required evidence / valid location citation | Source slices | Full repositories |
| --- | --- | --- |
| Default retrieval | 11/11 | **6/11** |
| Existing identifier routing | 11/11 | **7/11** |
| Four location queries, default / routed | 4/4 / 4/4 | 3/4 / 4/4 |
| Seven behavior/rationale queries, either mode | 7/7 | **3/7** |

At the unchanged 3,000-token, 12,000-character, eight-candidate budget, misses
include gateway owner selection, the caller/callee failure chain, uncertain
waypoint inspection, and waypoint deletion conditions. Generic retrieval also
misses the waypoint function location, which identifier routing recovers.
The graph knows the named symbols and the needed files are indexed, but the
packets select neighboring/caller windows or clip required branches. No source
document or passage-limit omission explains these failures. Packing's
`omitted.sources` counts selected candidates, not every required declaration.

Post-hoc diagnostics on the Nexus #963 misses kept questions and candidate
limits fixed. Turning focused packing off did not recover either behavior
answer. An 8,000-token budget recovered the caller read and callee failure
return, but still omitted the caller publication; gateway owner selection also
remained incomplete. These are diagnostic runs, not a new acceptance trial.
The next source-side experiment should preserve complete named-symbol blocks
and required call-related branches before spending the budget on other windows.

An evaluator correction accepts an exact, hash-bound definition citation in
`evidence.code_graph` for location questions, just as it accepts the identifier
route's definition citations. It never substitutes metadata for behavior or
rationale source text. All saved packets were rescored consistently; questions
and evidence labels did not change. Original results and hash-verified launch
versions remain available for audit.

The initial debug index build was interrupted before queries. Both final slice
and full-repository runs used the same existing optimized executable; its
retrieval/indexing sources match the baseline, while review/history code and
its tests differ and are not exercised here. This is a source-delivery test,
not a controlled performance or end-to-end answer-quality evaluation. The
eleven-query set remains two captured diagnostic requests plus nine authored
real-case questions, and has no real reviewed-note labels.

Reproduce full-repository retrieval with the matching baseline executable:

```sh
python bench/memory/tools/full_repository_eval.py \
  --repo /path/to/nexus --binary /path/to/graphicle \
  --dataset bench/memory/fixtures/real_query_seed.json \
  --out /tmp/full-repository-run \
  --reranker /path/to/model --ort /path/to/libonnxruntime.dylib
```

The local clone must contain the exact commits named in the fixture. To replay
paired scoring, apply its saved patch to an isolated baseline checkout, build
from `crates/graphicle`, then run `query_ablations.py --arm paired` with that
binary and a new output directory. Primary protocols, packets, source manifests,
scoring correction, diagnostics and logs are at
`/private/tmp/graphicle-pair-context-20261030/`.

Validation: 98 new context calls; 55 Python checks, three candidate Rust unit
checks and six symbol-link integrations passed; two optional Python checks
were skipped. No remote-model calls. Product source and defaults are unchanged.

## Atlas and Nexus: full-tree, single- and cross-repository evaluation

This evaluation froze `fixtures/atlas_nexus_queries.json` before changing
retrieval: ten single-repository questions (five Atlas, five Nexus) and four
Atlas–Nexus HTTP flows. Atlas is pinned to
`aa5c6103d2b04db966ffab779a404f2c549fbad7`; Nexus to
`0225ca2f30694b7ea1a6d6f063e602f24a0fd05a`. These are authored questions about
actual source, **not captured production queries or independently judged answer
quality**. Six questions are marked validation before candidate implementation;
the same author created both splits, so this is not an independent holdout.
There are no real reviewed-note labels in these repositories.

Normal indexing of the complete safe tracked trees produced 1,574 Atlas files
and 2,429 Nexus files. Credential paths and symlinks are omitted before blob
reads; conversion and normal source-policy exclusions remain recorded. Both
projects share one workspace store. `graph.link` is explicitly run and recorded
before querying. The requests retain the previous 3,000-token,
12,000-character, eight-source limit, pinned local MiniLM/blend, graph retrieval
and symbol links. The single-project cases use project scope; workspace cases
run with cross-repository traversal both off and on.

The evaluator checks every excerpt against its repository's indexed hash and
exact bytes. The same relative file path in another repository cannot satisfy a
label. Every required project must contribute all its labelled source spans;
definition metadata alone cannot pass these behavior questions. Required-span
delivery is the primary measure. Supporting-excerpt overlap is a secondary,
coarse measure, not a statement that every token in an excerpt is relevant.
Fixture labels never enter requests.

The corrected baseline delivers **6/10 single-repository questions** (Atlas
2/5, Nexus 4/5) and **0/4 complete cross-repository flows**. Workspace packets
contain only **1/11 required spans**, equally with traversal off or on. The
link receipt has **zero cross-repository import edges** between these projects;
this is not a skipped link job. Atlas's HTTP calls into Nexus are not imports.
Graphicle already has an opt-in HTTP-route extractor, but its current framework
support is Python and JavaScript/TypeScript, not Nexus's Go/Gin routes; workspace
linking currently resolves imports. This identifies an existing implementation
to extend, rather than justification for a separate route engine.

Two source-loss mechanisms were traced independently of the model: graph
fusion maps a symbol onto any overlapping chunk, and evidence expansion/merging
can enlarge an explicitly selected declaration into a neighboring region which
packing then clips. The packer normally shares remaining source tokens across
up to three slots. The local reranker has another boundary: it tokenizes a
query–passage pair with a total maximum length of 256, so it can miss a later
branch before selection or packing. No truncation ablation is claimed yet.

The first isolated candidate reserves existing authorized graph ranges for
explicitly named symbols, leaving packing unchanged. Historical Nexus
completeness improved **6/11 → 8/11**; all 48 synthetic note cases retained
50 relevant notes out of 133 returned (37.59% precision, 99.10% macro recall).
It still lost the concrete callee's failure branch amid same-named mocks and
interface declarations, and merging still displaced the waypoint function.
The second isolated candidate preserves the requested range through expansion,
merging and packing, while retaining the hard packet budget and memory share.
Its completed source-delivery trial gives:

| Corpus | Baseline complete | Candidate complete |
| --- | --- | --- |
| Historical Nexus | 6/11 | **10/11** |
| New Atlas single repository | 2/5 | **5/5** |
| New Nexus single repository | 4/5 | **5/5** |
| Atlas–Nexus workspace, traversal off or on | 0/4 | **3/4** |

No previously complete case regressed. All 48 note cases retained 37.59%
precision and 99.10% macro recall; comparison by case found no lost relevant
notes, including callee-rationale and four-formatting-note safeguards. Workspace
required spans improved 1/11 → 8/11. These are **source-delivery improvements**,
not new HTTP graph edges or generated-answer scores. That v2 trial remained
isolated pending follow-up validation. Historical receipt:
`results/atlas-nexus-named-ranges-20261030.json`; reproducible candidate
patch: `results/named-range-preservation-v2-experimental-20261030.patch`.

The v3 follow-up preserved bounded attached comment paragraphs and recovered
historical `uncertain-link`, reaching 11/11. It was **rejected** because workspace
completeness fell from 3/4 to 2/4: expanding `NexusctlApi.listConfigs` into the
class and bare method reserved unrelated ranges; adding the comment displaced
required server evidence. Two regression tests also exposed overbroad qualified
names and loss of a second requested function with `notes_first:true`.

**The adopted v4 change** uses exact written references for range reservation,
while retaining the existing broad identifier matching for lexical retrieval.
It reserves at most six existing authorized, fresh graph ranges, preserves
bounded attached comments without merging neighboring code, and lets each
requested range use the remaining source allowance. Distinct requested ranges
in one file survive optional file diversity. The hard packet budget and memory
share still apply; oversized declarations may still be clipped. This does not
guarantee complete evidence for arbitrary questions.

| Source-delivery evaluation | Baseline complete | Adopted v4 complete |
| --- | --- | --- |
| Historical full-repository Nexus | 6/11 | **11/11** |
| New singles, explicit protocol | 6/10 | **10/10** |
| Workspace, traversal off or on | 0/4 | **3/4** |
| New singles, retrieval defaults | 4/10 | **9/10** |
| Workspace, retrieval defaults | 0/4 | **3/4** |

The defaults trial leaves retrieval switches and source limit at defaults but
retains the same explicit local reranker/blend and 3,000-token/12,000-character
budget. It is not an unconfigured-installation benchmark. Required workspace
spans improve **1/11 → 8/11** with the explicit protocol and **2/11 → 9/11** with
retrieval defaults. No complete case or required span regresses against the
matched baselines or the earlier v2 explicit trial. All 48 synthetic note cases
retain **50/133 precision (37.59%) and 99.10% macro recall**, with no lost
relevant IDs by case. The callee-rationale and four-formatting-note safeguards
pass. This change improves source completeness; **note precision is unchanged**.

The source fix and regression tests are applied to the product worktree.
Final receipt: `results/atlas-nexus-named-ranges-v4-20261031.json`. The receipt
records source/executable/protocol hashes, case results and validation logs.
The older v2 receipt remains historical, not the adoption result.

Validation in the product worktree passes 404 runtime unit tests, 38
graph/workspace/symbol-link integration tests, the targeted packing-budget unit
test and 38 Python evaluator checks.
The built product also passes source-completeness replays of
`caller-callee-failure` and `cross-partial-list`. The complete v4 empirical
matrix contains 91 context calls on the isolated candidate, followed by those
two product-build replays; there are no remote-model calls. The seven promoted
files match the formatted candidate. Other pre-existing product changes are
preserved, so the receipt distinguishes the evaluated candidate binary from
the integrated product binary.

Two measured limits remain. Default `nexus-update-identity` matches 5,245 graph
nodes but caps candidates at 300; generated BPF identifiers and a mock method
displace the desired declaration before reservation (`named_windows: 0`). The
explicit protocol passes this case through other retrieval. Workspace
`cross-delete-local` still lacks complete server evidence: three required spans
with explicit settings, two with defaults. Both runs record zero Atlas–Nexus
import edges. Searching both repositories improved; proven HTTP traversal was
not added. Generated-answer quality, real reviewed-note precision, reranker
truncation benefits and latency improvements remain unmeasured.

The next experiments, in priority order, are:

1. Prefer exact declarations before the graph candidate cap. Keep same-named
   mocks, generated code, qualified methods and ambiguous bare names as
   distractors; measure the default Nexus miss and preserve the full suite.
2. Collect independently labelled user queries, including no-answer questions
   and real reviewed notes. Judge unnecessary evidence as well as completeness
   before tuning how many notes to return.
3. Isolate MiniLM's 256-token pair truncation: compare bounded branch-aware
   passages at the same retrieval and packing budgets. Report source delivery,
   note precision/recall and controlled latency separately.
4. Extend the existing route extractor/linker for Go/Gin and cross-repository
   HTTP resolution. Test method/path/host ambiguity, stale routes and repository
   authorization before claiming client-to-server traversal.
5. Sweep small packet budgets and multi-symbol questions, then test source
   edits/renames/deletions through incremental reindexing. Retain the callee
   rationale and four-formatting-note cases in every acceptance run.

An initial runner launch mistakenly supplied `max_projects` to project scope.
Graphicle rejected those ten requests. That launch is retained under
`baseline/` for audit and excluded from the baseline quality result. The fixed
runner has a scope-specific request regression test; the authoritative baseline
is `baseline-v2/`. No source or question labels were changed by this fix.

Primary artifacts and reusable warm stores:
`/private/tmp/graphicle-atlas-nexus-20261030/`. Run a fresh matched trial with:

```sh
python bench/memory/tools/multi_repository_eval.py \
  --dataset bench/memory/fixtures/atlas_nexus_queries.json \
  --binary /path/to/graphicle --out /tmp/atlas-nexus-baseline \
  --prepared-root /tmp/atlas-nexus-index --prepare \
  --repos-dir /path/to/local-repositories \
  --reranker /path/to/model --ort /path/to/libonnxruntime.dylib
```

Then use the candidate binary, a new `--out`, and the same `--prepared-root`,
omitting `--prepare` and `--repos-dir`. Do not resync or change prepared source/graph content
between arms. Add `--defaults` for a separate matched retrieval-defaults trial.
The runner records dataset, executable, scorer, model,
link, manifest and index hashes. Debug/release differences and concurrent local
work mean these runs do not support latency comparisons. Queries use only local
inference and generate no answers. Cross-repository graph-edge use is reported
separately from workspace source delivery.

### Evaluation follow-up (2026-10-31)

The new exact-symbol candidate-cap experiment is **rejected and rolled back**.
One candidate improves default single-repository completeness **9/10 → 10/10**
and retains 11 historical Nexus cases, but explicit workspace completeness falls
**3/4 → 2/4**: a same-named service implementation displaces the partial-list
handler's final branch. Restricting promotion to absent names still fails that
case; restoring the old range order recovers it but loses the default gain and
an explicit single case. No variant passes the complete acceptance set. The
previously accepted symbol preference and complete-range packing remain.
The 48-note replay of the rejected candidate loses no relevant note IDs:
**50/133 precision (37.59%)**, **99.10% macro recall**, with callee rationale and
all four formatting notes preserved. That alone is insufficient for adoption.
After rollback, the final 48-case note replay has the same precision and recall,
with no relevant-ID loss against v4 and both safeguards intact. Targeted source
replays recover the accepted baseline: explicit partial-list and update-identity
pass; default update-identity remains the known miss.

`tools/note_abstention_eval.py` finds 18 irrelevant notes returned for six
authored no-answer controls. Combined with 48 frozen cases, cap four gives
**50 TP / 101 FP / 2 FN**, **33.11% precision / 96.15% pooled recall**; only five
of 17 negative cases return nothing. Cap three loses a formatting note; cap two
also loses caller-pressure callee rationale. Cap one raises precision to 57.14%
but cuts pooled recall to 53.85%. These are post-hoc output cuts, not production
filters or latency measurements. **No fixed cap is adopted.** Pooled recall
and macro recall are different aggregations.

The new controls declare `provenance_kind: authored_control`. Independent
real-query evaluation still needs captured questions and independently supplied
labels. `--require-captured` rejects authored controls or missing provenance.
Source-derived questions must not be relabelled as independent production data.

`tools/truncation_eval.py` and the `rerank_windows` Rust example compare full
passages with 160-token windows, stride 120, at most eight, using MiniLM's actual
256-token pair tokenizer and batch size one. Labels never enter the scorer.
Across 14 frozen packets, whole required-span visibility stays **6/21** (18/21
spans were delivered before scoring). Short passages recover
`nexus-forward-loop` but lose complete `atlas-apply-proof` evidence. Supporting
passages at three change **16/42 → 17/42**, inference calls **99 → 286**, model
tokens **18,169 → 43,512**. This measures conditional scoring, not end-to-end
recall or answer quality. Timing has cold-load/order confounds. **The production
scorer is unchanged.** Post-hoc analysis finds a complete required span in some
window for 11/21 spans, but maximum-score selection retains only six. Final
32-byte span tails are visible in six full inputs versus nine selected windows;
this is a diagnostic, not a new gold label.

Go/Gin handling extends the existing opt-in route extractor (`GRAPHICLE_ROUTES=1`)
and reuses export indexes, stored edges and permission-filtered traversal.
Literal groups, verbs and middleware supply route facts. A relative URL needs
one unique method/path match and is labelled `host_unknown`, not proof of a
deployed destination. Absolute URLs require a known matching origin. Ambiguous
routes are withheld. Both complete episodes must remain current: unchanged
handler bytes cannot validate changed registrations or middleware. Enabling the
extractor does not rebuild an already immutable episode; the trial uses a fresh
store. HTTP exports reuse the import exporter's test-file exclusion rule.
Production Go handlers do not resolve against same-named methods in `_test.go`
files. A regression includes duplicate test routers and mock handler methods.

The final matched full-tree replay (`http-cached-full`) indexes 1,574 Atlas and
2,429 Nexus files. All ten explicit single-repository queries pass. Across four
workspace questions, cross-repository linking improves complete evidence
**3/4 → 4/4** and required-span delivery **8/11 → 11/11**, with no case regression.
The delete-flow question recovers its route registration, handler identity check
and DELETE-local forwarding branch; partial-list retains its late response
branch. Supporting-excerpt precision improves **7/31 (22.58%) → 10/36 (27.78%)**,
but total excerpts increase from 31 to 36 (non-supporting excerpts 24 → 26).
Aggregate packet tokens are 11,527 versus 11,484. These are source-evidence
metrics on four source-derived questions, not note precision or generated-answer
quality. The stored-edge audit finds 44 HTTP edges from eight unique client
requests, all labelled `host_unknown`. **Retain HTTP linking as opt-in**; this
small evaluation does not establish a general precision gain.

Budget/indexing checks cover eight requested functions at 400, 800, 1,500 and
3,000 tokens, both note packing orders, UTF-8 citation boundaries, edits,
renames and deletions before and after graph rebuilding. All 13 graph-arm
integration cases pass in isolation. Small budgets enforce bounds without
promising impossible completeness.

Full-tree stress exposed repeated whole-file hashing in identity annotation:
71,666 nodes in Nexus's 2,616,664-byte generated `vmlinux.h` all use a conservative
file revision. Rehashing the file per node processes 187,525,842,224 bytes for
that fallback alone. `SourceText` now computes the immutable file digest once;
exact declaration and legacy slice revisions retain their existing semantics.
A new source snapshot recomputes the digest. All 16 identity tests pass. Comparing
all 71,666 persisted header nodes with the accepted baseline finds **zero changed
revision/citation records**, and both indexes' repository-scoped stable IDs
validate independently.

The final fresh Nexus index completes with 98,441 nodes and 145,501 edges;
identity annotation takes about 12 seconds in this observed run. This is not a
controlled speed comparison. The final build uses per-package `opt-level=2` for
`graphicle`, `graphicle-memory` and `graphicle-runtime`, and a 16 MiB worker stack.
The 41 focused integration tests pass, including HTTP freshness/permissions,
budget/indexing stress and symbol-link recall guards; 19 route-extraction and
three pure HTTP-linker tests also pass. Evaluator tests pass (72 passed, two
skipped; the 12 multi-repository tests were rerun after live-stderr logging).

Raw trials are in `/private/tmp/graphicle-loops-20261031/`. Earlier HTTP indexing
attempts hit a debug worker stack overflow or the 2,400-second Nexus timeout;
only `http-cached-full` is authoritative for final full-repository quality.
Go import discovery now examines only top-level imports, avoiding needless
traversal of generated function bodies. The first scorer hit the existing
macOS ONNX shutdown issue; its successful retry uses the runtime shutdown
helper. Temporary identity probes failed on transport size or duplicate crate
identities; the broken example was removed, and the persisted-index comparison
is authoritative. Failed attempts are excluded from quality results.
The six no-answer controls were rerun with the cached-identity binary and final
accepted 48-case guard results; all reported counts remain unchanged.

The machine-readable receipt is [evaluation-followup-20261031.json](results/evaluation-followup-20261031.json). It separates retained changes, rejected experiments, raw-artifact hashes and outstanding independent-query data.
