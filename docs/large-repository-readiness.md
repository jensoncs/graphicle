# Large repository readiness experiments

Assessment: suitable for a controlled pilot; not yet production-qualified for
interactive context and graph management of very large repositories.
E1–E20 are complete. E15 passes the bounded million-file qualification,
1,341 runtime tests and the 536-case quality gate. E16 retains indexed retrieval
for graph-enabled and filtered file queries: its 999,999-file replay reports
zero corpus omissions, 33.8–36.4% lower median latency, and 4.2–5.9% lower peak
RSS. E17 then avoids unnecessary full source-graph loading, preserving complete
responses while reducing median latency by 46.1–49.6% and peak RSS by
28.0–80.0% against its paired E16 control. E17 passes 79 targeted tests and
16 scale queries. E18 removes the remaining resident-graph reads from healthy
cross-repository endpoint lookup: its paired E17 comparison reduces median
latency by 19.7% and peak RSS by 71.1%, with identical complete packets,
108 targeted tests and 16 scale queries passing. The full E15 regression and
quality suites were not rerun for E16–E20. E19 adds profiling: metadata
construction and state hashing account for 45.0% of measured indexed time;
it identifies streaming metadata construction as the next action and claims
no performance improvement. E20 implements that change, moving SQLite field
values directly into final document records. It passes 59 targeted tests and
16 paired scale queries; the measured impact and initial mixed replay are
recorded in [E20](#e20-streamed-repository-document-construction-retained).
Observed median latency falls 6.1–7.0%, while maximum RSS rises 5.3–7.0%;
this is not a demonstrated memory saving.
General production qualification remains open.
Starting revision: `a1fa611` on
`context-improvement-plan`. This work covers both very large monorepositories
and context/graph operations across multiple repositories. It is not a claim
that the starting revision is production-qualified.
The user has now selected **one million files across repositories** as the
acceptance target. E15 now passes all 73 checks on exactly that initial file
count across ten repositories. This qualifies the tested graph and bounded
retrieval behavior. E16 expands eligible indexed file retrieval on the retained
fixture after its deliberate deletion; independent recall and interactive
production performance remain unqualified.

## Readiness decision

The million-file E15 run passes all 73 public CLI checks: exact source hashes,
999,979 generated local calls, unchanged graph equality, cross-repository episode
bindings, repeated context packets, citation bytes, token budgets, revoked
access, and changed/deleted exporters. The initial graph contains 2,999,990 nodes
and 2,999,973 edges. All three 100,000-file Go graphs pass after the bounded
compiler-analysis correction. Final regressions pass: 1,341 runtime tests,
8 ignored, and the 536-case quality gate without regression findings or
input-integrity errors.

**This remains a controlled-pilot result.** E15's 18 main graph-enabled workspace
context samples report 600,000 omitted documents, and their median latency is
32 seconds without cross-repository retrieval and 43–44 seconds with it. Peak
context memory reaches 15.8 GiB. The reviewed answer sources are found, but
complete million-file context coverage, independent retrieval quality, warm-server
latency and concurrent production use are not qualified.

E16 addresses the indexed-file cutoff without raising limits. On the final
999,999-file fixture, paired E15/E16 calls reduce omitted documents from 599,999
to zero and search 999,998 passages instead of 400,000 (one file is empty).
Median context latency falls from 31.6 to 20.9 seconds with cross-repository
retrieval off, and from 37.1 to 23.6 seconds with it on. Peak RSS remains as high
as 18.82 GiB. These are three measured samples per mode on one authored question,
with build-matched caches prepared outside timing. The property graph still
degrades at its existing budget; code-graph retrieval, citation checks and
answer budgets remain active. See [E16](#e16-indexed-recall-beyond-query-chunking-bounds-retained).

E17 skips full source-graph loading only when the filtered document count
already guarantees property-graph degradation and valid saved file relationships
are available. Complete E16/E17 packets match. In the fresh paired replay,
context medians fall from 29.05 to 14.64 seconds with cross-repository retrieval
off and from 31.09 to 16.77 seconds with it on. Maximum RSS falls from
18.55 to 3.70 GiB and from 18.31 to 13.18 GiB, respectively. Cross-repository
endpoint lookup still opens six resident graphs in that version. Earlier E16
timings are a different run; reductions are calculated only from this paired
control. See [E17](#e17-avoid-unused-source-graph-loading-retained).

E18 resolves those endpoints from saved query files, with resident fallback
for absent or invalid caches. A fresh paired E17/E18 comparison reduces
cross-repository median latency from 12.58 to 10.10 seconds and maximum RSS
from 13.14 to 3.80 GiB; resident-graph reads fall from six to zero. All 16 packets
match, with 160 byte-citation checks and zero corpus omissions. With cross-repository
retrieval off, median latency changes by only -1.4% and peak RSS by +0.4%;
that mode has no endpoint work to remove. Timings vary within this run, so these
are observed comparisons, not latency guarantees. See
[E18](#e18-saved-cross-repository-endpoint-lookup-retained).

The retained E1/E2/E5 candidate demonstrates graph construction, updates, cited
context and navigation on 100,000 synthetic Python files. Every generated topology and
source-head check passes, and the final graph equals a fresh-cache rebuild.
Independent full/incremental equality also passes on the much smaller real
CPython corpus. This establishes useful correctness and feasibility evidence.
E6 is also retained after a separate 25,000-file comparison: verification is
59% faster and complete single-file edits are 10% faster, with identical graphs.
E7 reduces fresh-process navigation time by 30% on the retained 98,999-file
graph, with identical responses. The 100,000-file results below are from the
E5 build, before E6 and E7.
E8 is retained after a separate ten-repository comparison: rebuilding all
export indexes and relinking drops from a median 100.9 to 3.67 seconds, with
exact index/link equality and unchanged peak memory. It does not speed up the
single-repository parsing and update operations in the table.
E9 also reuses verified navigation snapshots: in its separate comparison,
caller/callee startup improves 24.6% and search 22.9%, with lower peak memory.
These remain first-request whole-graph operations, not warm-server latency.

E12 retains a smaller memory-listing change after 288 exact-response comparison
calls. On 25,000 reviewed notes, filtered recall and memory-disabled requests
use 22.5% and 25.9% less peak memory. Ordinary and graph-assisted recall use
2.8% and 0.6% more peak memory; latency changes are small. This targeted saving
does not resolve the recall bound, source-reversion finding or graph update
costs, and does not change the readiness decision.

E10 separately qualifies the retained E9 build on a pinned Babel checkout:
29,217 admitted files and 47.6 MB, with 16 successful CLI calls. The first graph
takes 71.4 seconds and 2.75 GiB; a single-file rename update takes a median
56.6 seconds and up to 2.56 GiB. The mixed corpus selects the existing full
resolution fallback, so these updates do not qualify selective incremental
resolution. Two unfiltered graph-context probes reach the 40,000-passage bound
and explicitly omit 4,778 and 5,096 documents. Their reviewed answer sources
are found, but that is not complete corpus coverage. Details and omissions are
recorded below.

The measured operating costs still prevent an interactive scale guarantee:

| Operation at 100,000 files | Observed wall time | Peak memory |
| --- | ---: | ---: |
| Cold graph, after source admission | 494.9 s | 5.71 GiB |
| Unchanged refresh | 28.5 s | 0.43 GiB |
| Single-file edit | 308.7 s | 5.64 GiB |
| Context with graph retrieval | 3.9 s | 3.56 GiB |
| Caller/callee navigation | 12.7 s | 3.17 GiB |

Refresh and edit times are medians of three calls; other rows have one call.
Memory is the maximum observed process peak for each operation. The corpus is
only 9.38 MB of tiny files with a common function name, on macOS arm64 with a
warm OS cache. File count alone does not qualify a realistic monorepository.
Every measurement starts a fresh CLI process. Navigation includes loading its
resident graph; repeated requests in an already warm server are not measured.
Context and navigation run after deletions, at 98,999 files. The context probe
asks about one exact symbol and disables reviewed-memory retrieval.

The default admission limits are 100,000 files and 1 GiB per project. Files
over 8 MiB are left out; files over the 4 MiB analysis limit remain searchable
as text but do not receive structure analysis. These limits are configurable
and omissions are reported; raising a limit does not establish performance at
that size. Workspace context searches eight projects by default and allows up
to 32 per request. See [`limits.rs`](../crates/graphicle/src/limits.rs) and
[`workspace.rs`](../crates/graphicle/src/workspace.rs).

The main demonstrated update limit is conservative invalidation: one changed
file in this shared-name workload still re-resolves every file. Dependency
compression reduces the internal graph's memory cost without reducing that
correctness-preserving resolution set. Expanded retrieval also uses a bounded
full path (50,000 documents and 40,000 passages), with explicit omissions.
Ten-repository mixed-language context passes 73 calls on 100,000 generated
files, including revoked access and stale/deleted exporters. Expanded/memory-linked context now
passes 90 calls on 98,999 sources, with exact citations and explicit coverage
limits. E11 exercises 25,000 API-authored reviewed notes over 40 small sources:
ordinary and graph-assisted recall omit the oldest 5,000, while a source filter
recovers the oldest probe. A source reversion also leaves stale memory markers
in the freshness report despite restored note eligibility. Combined large
source/memory workloads and independent retrieval quality remain unqualified.
The full property-graph walk reports degradation beyond its
50,000-node bound; the code-graph retrieval arm still runs.

Use the candidate for a measured pilot when these observed costs are acceptable.
Before claiming general large-repository readiness, qualify representative
mixed-language repositories against agreed refresh, query and memory budgets,
and qualify combined source/memory and concurrent workloads. The final
regression checks are recorded separately below.
These measured results are not a released production guarantee.

## Pending work for full production

The completed E1–E20 experiments establish the controlled-pilot result above.
The following work remains open, in recommended order. Each checkbox requires
recorded evidence before closure. Production latency, memory, concurrency and
freshness budgets have not yet been agreed; the measured values are baselines,
not acceptance targets. Progress notes identify implemented fixes; unchecked
completion criteria remain open.

- [ ] **Define the production acceptance envelope.** Keep the one-million-file
  target and specify the intended repository distribution, source bytes,
  languages, reviewed-memory volume, deployment hardware and concurrent load.
  Set query p50/p95/p99, peak memory, indexing throughput and update-freshness
  budgets. **Done when:** these budgets and workloads are recorded and used as
  explicit gates for the remaining performance and qualification work.

- [ ] **Improve corpus retrieval coverage (E15-F1).** The graph-enabled full
  retrieval path reports 600,000 omitted documents in every main million-file
  context sample; its property-graph arm also degrades. Reuse or extend indexed
  retrieval and bounded traversal so relevant candidates can be found across
  the intended admitted corpus. Keep answer budgets, authorization, freshness
  checks and omission reporting. Raising limits alone is not qualification.
  **Done when:** independent labelled queries exercise currently omitted parts
  of the corpus and meet the agreed recall and resource budgets with accurate
  coverage diagnostics. **Progress:** E16 removes the indexed repository-file
  cutoff and reports zero omissions on the 999,999-file replay; a 50,001-file
  regression retrieves a stemmed match omitted by the bounded full path.
  Independent labelled recall, oversized connector catalogues, non-indexed
  query options and bounded property-graph coverage still need qualification.
  Evidence: [E15](evidence/large-repository-e15-2026-11-06.json) and
  [E16](evidence/large-repository-e16-2026-11-06.json).

- [ ] **Reduce context and graph-update costs.** E15's fresh-CLI workspace
  context baseline medians are 32–44 seconds with up to 15.82 GiB peak RSS. One exporter
  edit takes 132.8 seconds to admit and 349.5 seconds to refresh its graph;
  deletion refresh takes 194.5 seconds. Profile remaining whole-corpus loading,
  verification, graph assembly and persistence costs before selecting changes.
  **Done when:** repeated comparisons show useful improvements and the agreed
  budgets pass, with exact citations, preserved permissions and graph equality
  after edits, renames and deletions. Measure warm-server requests separately
  from fresh-process startup. Evidence: [E15](evidence/large-repository-e15-2026-11-06.json).
  **Progress:** E16 removes the indexed-file coverage cutoff. E17 avoids full
  source-graph loading when the property graph must degrade and saved file
  relationships suffice. Its paired medians are 14.64/16.77 seconds; maximum
  RSS is 3.70/13.18 GiB with cross-repository retrieval off/on. Complete packets
  match E16 and corpus omissions remain zero. These are separately paired runs.
  Evidence: [E17](evidence/large-repository-e17-2026-11-06.json).
  E18 completes the endpoint change: healthy queries use saved query files,
  with validated resident fallbacks. Its paired cross-repository median falls
  from 12.58 to 10.10 seconds and maximum RSS from 13.14 to 3.80 GiB, with six
  resident reads reduced to zero and identical responses. Evidence:
  [E18](evidence/large-repository-e18-2026-11-06.json).
  **Profile complete (E19):** metadata construction uses 30.6% of indexed
  time and state hashing 14.4%; catalogue clones use 6.6%. All ten catalogues
  remain retained through workspace fusion. Evidence:
  [E19](evidence/large-repository-e19-2026-11-06.json).
  **Completed (E20):** repository rows now stream into final document records,
  with decoded fields moved into their final slots. The intermediate generic
  JSON row collection is gone. SQL, metadata, state hashes and complete query
  responses are preserved. See the paired measurements and limitations in
  [E20](evidence/large-repository-e20-2026-11-06.json).
  **Next focused change:** stream state fingerprint serialization while keeping
  byte-identical digests. Then investigate avoidable catalogue copies and
  retained workspace metadata. Those changes still need independent correctness
  and impact checks. Graph-update behavior and costs have not changed in E16–E20.

- [ ] **Reconcile memory freshness after source restoration (E11-F1).** Restored
  source bytes make dependent notes eligible again while `gf_stale` still marks
  them stale; the reproduced large catalogue reports 625 stale notes despite
  current returned evidence. **Done when:** edit/restore checks show consistent
  eligibility and freshness diagnostics, without returning stale notes during
  the changed-source interval. Evidence: [E11](evidence/large-repository-e11-2026-11-06.json).

- [ ] **Address memory recall limits and qualify combined scale.** Ordinary and
  graph-assisted recall consider the newest 20,000 eligible notes, omitting the
  oldest 5,000 in the 25,000-note fixture. E12's memory saving does not resolve
  this bound. The million-file run disables reviewed-memory retrieval.
  **Done when:** old and recent relevant notes are retrievable across the
  supported catalogue, with explicit limits, and a combined million-file and
  large-note workload passes relevance, source/symbol freshness, permission,
  latency and memory gates. Evidence: [E11](evidence/large-repository-e11-2026-11-06.json)
  and [E12](evidence/large-repository-e12-2026-11-06.json).

- [ ] **Qualify representative repositories and independent retrieval quality.**
  The million-file corpus contains only 124.5 MB of synthetic source. Test real
  source sizes, uneven repositories, shared identifiers, generated code,
  dependency changes and the language/import forms required by the deployment.
  Qualify any required per-project sizes beyond the current tested 100,000-file
  envelope explicitly. **Done when:** representative corpora and independently
  labelled user queries pass the agreed coverage, relevance and resource gates;
  the existing authored quality suite remains a regression check.

- [ ] **Qualify concurrency and operational recovery at target scale.** Exercise
  simultaneous queries and indexing, edits and access revocation during builds,
  worker/server restart, interrupted projection, cancellation and recovery using
  the existing mechanisms. Validate backup restoration and rollback on the
  intended deployment. **Done when:** sustained-load and recovery evidence shows
  consistent published graphs, preserved authorization/freshness, bounded
  resource use and acceptable queue delay. These are qualification gaps, not
  findings that the existing recovery mechanisms are absent or broken.

- [ ] **Complete release validation and rollout.** Review and commit the retained
  changes and evidence through the repository's PR/CI workflow. Run the applicable
  engine and Python suites; run the eight ignored runtime tests where required
  dependencies are available, or record justified deployment-specific exclusions.
  Validate the release artifact in staging with query latency, memory, indexing
  lag and error monitoring and a tested rollback procedure. **Done when:** the
  reviewed release has reproducible build identity, passing required checks and
  recorded staging/rollback results.

## Million-file measured envelope

E15 uses ten repositories of exactly 100,000 initial files: four Python, three
TypeScript and three Go. The 999,979 generated code files and 21 manifests/probes
contain 124,488,968 source bytes (124.5 MB). This is a synthetic tiny-file corpus,
not a million realistic production files. Admission limits remain unchanged.

| Operation | Observed wall time | Maximum observed process peak memory |
| --- | ---: | ---: |
| Admission, each 100k-file repository | 165.7–286.0 s | 1.45 GiB |
| Cold graph after admission, each repository | 383.2–793.2 s | 11.40 GiB |
| Unchanged refresh, each repository | 23.4–30.0 s | 0.45 GiB |
| Workspace context, cross-repository retrieval off | 31.8–32.1 s median | 15.82 GiB |
| Workspace context, cross-repository retrieval on | 42.8–44.2 s median | 15.76 GiB |
| Admit one exporter edit | 132.8 s | 0.95 GiB |
| Rebuild its graph after the edit | 349.5 s | 9.67 GiB |
| Admit exporter deletion | 35.4 s | 0.91 GiB |
| Rebuild its graph after deletion | 194.5 s | 12.33 GiB |

Each initial repository operation and exporter mutation is observed once.
Context ranges compare the Python, TypeScript and Go question medians, each
with three fresh-process samples per cross-repository setting. They are not
warm-server latency or A/B speedup claims. All phases run sequentially with no
competing build/test/fixture workload deliberately started; unrelated host load
is not verified. OS caches are warm. Every raw sample is retained in E15 evidence.

The edit reuses 99,999 source files and resolution partitions, resolves one file
and persists one dirty group, yet its graph refresh still takes 349.5 seconds.
Deletion reuses all 99,999 surviving partitions and resolves no file, yet takes
194.5 seconds. These costs show that selective resolution alone does not make
whole-graph refresh interactive. Admission and graph rebuild are separate calls;
the harness deliberately queries between them to test stale evidence.

All 18 main context samples consider the one-million-document inventory but
report 400,000 selected source passages and 600,000 omitted documents. The
500,000-document-limit counter overlaps those omissions; do not add the two
counters. Each project's full path reaches the existing 50,000-document and
40,000-passage bounds. The bounded property-graph arm degrades, while code-graph
retrieval continues and returns the reviewed answer sources. These requests
disable reviewed-memory retrieval. No bound is raised or requested feature
silently removed for the qualification.

Ten-project requests explicitly set `max_projects=10`. The default searches
eight projects and reports the other two as left out. Revocation reduces the
reader's authorized scope to nine without leaking the revoked repository; the
owner retains access. A changed exporter is withheld until rebuilt. Deletion
removes its cross edges and leaves exactly three expected Python imports
unresolved. The final inventory has 999,999 files after that deliberate deletion.

Final E15 regressions pass: 1,341 runtime tests (8 ignored) and all 536
quality cases, with no regression findings or input-integrity errors. Complete
source-head maps remain in the 103.7 MB raw result; repository evidence preserves
counts and hashes, every CLI sample, coverage diagnostics and build identity in
[`evidence/large-repository-e15-2026-11-06.json`](evidence/large-repository-e15-2026-11-06.json).

## Acceptance method

Try one change at a time against a saved baseline executable. Keep a change
only when repeated measurements show a useful improvement and its correctness
checks pass. Record rejected experiments and remove their runtime changes.
Do not improve timing by dropping files, graph edges, source validation,
authorization, freshness checks, or requested context features.

Measure cold admission and graph construction, unchanged refresh, a small edit,
large edits/deletions, ordinary and filtered context, memory-linked context,
and workspace context with cross-repository links. Distinguish indexing,
materialization, parsing, resolution, persistence and retrieval. Report wall
time, peak resident memory, corpus and binary hashes, effective settings,
sample counts, errors and omissions. A warm OS cache is not a cold-cache test.

The confirmed workspace target is one million files across repositories.
The first scale envelope is ten repositories of exactly 100,000 files each,
with cross-repository changes. E14 failed Go call correctness; E15 subsequently
passes all 73 checks after the bounded analysis correction. Uneven million-file
layouts remain unqualified. The earlier single-repository and 100,000-file
workspace qualifications remain separate historical evidence. `bench/qualify_workspace.py --files-per-project 100000` now
plans exactly 1,000,000 initial files including manifests and probes, without
raising the default per-project admission limit. Its new equal-size mode passes
all 73 workspace checks on exactly 400 files, including source/call coverage,
citations, budgets, repeated packets, revoked access and stale/deleted exporters.
The existing weighted mode remains available through `--units`. Include
mixed languages, common identifiers, shared dependencies, generated files and
uneven repository sizes. Synthetic scale probes supplement real repositories;
they do not establish independent retrieval quality or production SLOs.

Correctness requires graph/content equality against the control or a fresh
build, exact source citations, no lost labelled evidence, preserved token
budgets and explicit coverage limits. Exercise source changes, renames,
deletions, corrupted/stale caches and permissions. Run focused regressions per
experiment, then the broader relevant suites and quality gate for retained
changes. The six review/SDK failures from the November 6 merge receipt have
corrected expectations and pass targeted tests; broader regression and quality
checks are recorded below, including ignored tests and formatting drift. This
assessment does not claim complete release qualification.

## Existing mechanisms to reuse

- Incremental parse and conservative dependency-resolution caches.
- Verified source-tree reuse, immutable snapshots and paginated admission.
- Passage indexes and memory-mapped graph query files.
- Workspace retrieval with per-project authorization, rank fusion and one-hop
  cross-repository imports; default fanout eight, maximum 32.
- Phase timing, coverage diagnostics and the authored retrieval quality gate.

## Experiment register

| ID | Candidate | Baseline observation | Decision |
| --- | --- | --- | --- |
| E1 | Indexed retrieval for file/source filters | `indexed_query::ineligible` sends every filtered query through full snapshot retrieval | Retain: final release improves filtered latency and memory; 41 focused regressions, 120 matched benchmark calls and the 536-case quality gate pass |
| E2 | Shared-provider dependency vertices | 5,000 files with a common called name create 24,995,000 conservative dependency links | Retain: about 60% lower build/update peak memory; 60 paired graph checks and four cache migration checks pass; latency gains are modest |
| E3 | Workspace scale and cross-repository context | Existing implementation already reuses `project_packet`; avoid a duplicate federation layer | 73 calls pass across ten repositories and 100,000 generated files; the exposed export-linking cost is addressed separately by E8 |
| E4 | Memory/expanded-query performance | Optional retrieval features and oversized property walks use the full path | 90 calls pass on 98,999 sources; measured feature costs and coverage limits documented; no runtime change |
| E5 | Iterative Python receiver traversal | CPython full/debug extraction aborts during receiver analysis | Retain: 20 focused tests, receiver-fact equality for 2,060 files, and full/incremental graph equality before and after an API rename |
| E6 | Parallel source-cache verification | Isolated 24,749-file update spends 24.84 seconds in graph materialization despite writing only one file | Retain: 25,000-file median edit time falls 10%, verification 59%; exact graphs and cache-switch checks pass |
| E7 | Index community grouping positions | Navigation's community grouping scans all earlier groups for every node; many disconnected components make this quadratic | Retain: median fresh-process navigation falls 30%, with identical packets and 0.7% higher peak RSS; 34 focused checks pass |
| E8 | Group export symbols by normalized file path | Export indexing scans all graph nodes once per exported file; the workspace qualification spends about 49–53 seconds in linking for its largest TypeScript/Go additions | Retain: median forced workspace export-index rebuild/relink falls 96.4%; 79 focused checks and 24 scale calls pass with exact indexes/links and similar peak memory |
| E9 | Reuse verified graph snapshots for navigation | Navigation reconstructs the complete graph and search index even when this build's saved snapshot is available | Retain: first-request navigation/search improve 24.6%/22.9%, peak RSS 6.1%/12.5%; 47 focused checks and 24 scale calls pass |
| E10 | Real Babel repository qualification | Synthetic scale and the smaller CPython corpus do not qualify realistic TypeScript/JavaScript repositories | 16 full-corpus calls and 16 smoke calls pass; source updates match a fresh graph; full-resolution updates and bounded context coverage remain; no runtime change |
| E11 | Large reviewed-memory catalogue | E4 uses only three reviewed notes; ordinary recall considers at most 20,000 eligible records | 25,000 notes authored through the API and 50 context checks completed; oldest probe omitted without a source filter, and source reversion leaves stale freshness markers; no runtime change |
| E12 | Borrow the catalogue while deriving memory annotations | `memory::list_of` clones all records before computing dependency reasons and visibility | Retain for 22.5%/25.9% lower filtered/disabled-request peak RSS; ordinary/graph RSS rises 2.8%/0.6%; 99 distinct focused tests and 288 exact-response calls pass |
| E13 | Schedule large parser inputs first on existing workers | A Babel update spends 15.4 seconds in per-file parsing; large CommonJS inputs contribute substantial worker time | Reject and remove: five paired edits show only 1.4% median paired end-to-end improvement, with two regressions and 1.6% higher peak RSS; all 19 full-corpus graph checks pass |
| E14 | One million files across ten repositories | Earlier workspace qualification covers only 100,000 files in total | Fails on the third repository: 100,000-file Go compiler analysis times out and leaves all calls unresolved; Python and TypeScript pass at 100,000 files each; full target remains unqualified |
| E15 | Bound large Go compiler analysis at five minutes while honoring shorter request deadlines | The complete 100,000-file analysis needs 116 seconds, exceeding the existing one-minute allowance | Retain: all three 100,000-file Go graphs, all 73 million-file checks, 1,341 runtime tests and the 536-case quality gate pass. Completeness fix, no speedup claim |
| E16 | Keep graph-enabled and filtered repository-file queries on the passage index | Crossing full-path chunking bounds discards the usable index and omits 599,999 documents in the replay | Retain: zero corpus omissions, 33.8–36.4% lower median latency and 4.2–5.9% lower peak RSS; 70 targeted tests and 16 scale queries pass. Property-graph and connector bounds remain |
| E17 | Avoid the full source graph when the property graph must degrade and saved file relationships are valid | E16 still loads a full graph for each over-budget project | Retain: identical complete packets; 46.1–49.6% lower median latency, 80.0% lower peak RSS with cross-repository retrieval off and 28.0% with it on; 79 targeted tests and 16 scale queries pass |
| E18 | Resolve cross-repository endpoints from saved query files, with resident fallbacks | E17 still opens six resident graphs for endpoint paths and citation ranges | Retain: identical complete packets; cross-repository median latency 19.7% lower, peak RSS 71.1% lower, resident reads 6 → 0; 108 targeted tests and 16 scale queries pass |
| E19 | Profile indexed catalogue processing and workspace retention | Existing timings leave metadata construction, state hashing, copies and cleanup unattributed | Profile complete: metadata 30.6%, state hashing 14.4%, catalogue clones 6.6% of indexed time; 56 tests, 16 compared queries and two diagnostics-off parity probes pass. Next: stream metadata construction; no optimization claimed |
| E20 | Stream repository document construction | Intermediate generic JSON rows and cloned decoded fields add allocation during catalogue construction | Retain direct SQLite-to-document construction; 59 targeted tests, 16 identical-response scale queries and diagnostics-off parity pass. Median latency falls 6.1–7.0%, but maximum RSS rises 5.3–7.0%; final catalogue retention remains |

## Results and remaining work

E1 and E2 are retained, E5 fixes the reproduced Python receiver crash, and E6
reduces source verification and update time in the repeated-edit comparison.
E7 also reduces navigation startup time without changing responses.
E8 removes repeated export-index scans and substantially reduces workspace
relinking when indexes must be rebuilt; ordinary cached linking is unchanged.
E9 reuses verified graph/index snapshots for navigation, reducing startup time
and memory while preserving cache fallbacks and citation freshness checks.
The baseline release was built from the starting revision and
saved with its source and binary hashes. The synthetic 100,000-file graph
qualification, full runtime regression suite and combined retrieval quality
gate now pass. Expanded context also passes its 90-call scale check. Update
costs remain high. Ten-repository synthetic workspace qualification also
passes. E10 adds a real Babel corpus with 29,217 admitted files, source-reviewed
calls and context, explicit admission/coverage limits, and updated/fresh graph
equality. E11 adds an isolated 25,000-note catalogue and documents the recall
limit and freshness-report discrepancy. Larger real repositories, additional
language combinations, real memory collections and combined large source/memory
workloads still need qualification.
E12 reduces catalogue overhead for filtered and memory-disabled requests while
preserving complete responses. Its scoped checks pass, with the measured
ordinary/graph memory tradeoff documented below. The historical full runtime and quality
results below predate E12. The final E15 gate is recorded separately after them;
the earlier counts are not reused as new results.

The SDK assertion now expects Python's documented basic-analysis limitations.
Its product workflow and budget-recovery test passes against the combined E5
release (one test, 3.44 seconds). All five native review corrections pass
(15.22 seconds, then 15.02 seconds on the resumed run). They separate
tight-budget source priority and explicit omissions from complete
history/dependency delivery and delta checks at larger budgets. This follows
the existing `fixed_hex_identifiers_v1` selection policy in
[`pinned-review.md`](pinned-review.md) and the optional-history reservation in
[`business-context.md`](business-context.md); actual wire tokens and conservative
selection charges are different. No review runtime behavior is changed.
The earlier E1/E2/E5 runtime gate passed 1,338 tests, with none failed
and eight ignored. The combined E1/E2/E5 release also passes the 536-case
retrieval quality gate with no regression findings or input-integrity errors.
That gate does not establish complete default memory recall: the authored
suite measures 50% pooled memory recall and 75.7% precision with the default
counted symbol-link policy, versus 100% recall and 93.6% precision with symbol
links enabled. Both source variants deliver all labelled source evidence in
these fixtures. Independent user quality and the larger context workloads
remain unmeasured.

The quality gate ran alongside runtime correctness tests, so its latency is
not an isolated performance measurement. The entire engine and Python suites
were not rerun in this continuation; their focused checks remain recorded
separately. `git diff --check` passes. Formatting checks pass for the changed
runtime/engine sources. The added review expectation lines are now formatted;
the two review test files retain four and two pre-existing formatting hunks,
respectively, matching their baseline counts. This assessment
does not claim a clean release gate. Targeted and final results, source hashes
and the checks retained at each budget are recorded in
[`evidence/large-repository-regressions-2026-11-06.json`](evidence/large-repository-regressions-2026-11-06.json).

### Final E15 regression gate

The complete runtime `cargo test --locked --offline` invocation passes
1,341 tests, with zero failures and 8 ignored tests. The existing authored
retrieval-quality gate also passes all 536 cases with no regression findings or
input-integrity errors. Both source variants deliver every labelled source span;
default counted memory recall remains 50% with 75.7% precision, versus 100% recall
and 93.6% precision with symbol links on. These are regression fixtures, not an
independent user-quality assessment or proof of complete million-file retrieval.

These final correctness checks run concurrently only after isolated scale timing
finishes. Their elapsed times are not performance claims. All 408 recorded
runtime/build source hashes and the release binary remain unchanged before and
after both runs. `git diff --check` passes. The entire engine and Python suites
are not rerun; their applicable focused checks are preserved above and in the
experiment evidence. Ignored tests and pre-existing formatting drift remain
explicit. The Go correction is retained for recovered call coverage, with its
request deadlines, output bounds and other timeout limits preserved.

### E1: filtered passage-index retrieval

The candidate opens the existing index against the files admitted by the
validated source/metadata filter. This changes BM25 corpus statistics before
ranking. It reuses `query::restrict` for derived-memory visibility, filter
diagnostics and snapshot identity. Claim visibility still uses the complete
authorized memory catalogue. Text loaded for citations remains hash-verified.
Requests beyond the full path's bounded corpus selection retain that path.
No new switch, persisted format or dependency is introduced.
Unfiltered requests keep their existing index-open-before-metadata order,
including the early fallback for a missing or stale index.

Validation so far: 41 focused integration tests pass, including 18 indexed/full
packet comparisons across filters and retrieval options, connector metadata,
missing index layouts, memory visibility, workspace cross-links and permission
revocation. A separate 40-call baseline-versus-baseline harness check passes
exact packet, citation-byte, repository identity, token-budget and labelled
source-evidence assertions. These are correctness checks, not speed claims.
The final binary also passes the existing quality regression gate: 195 authored
queries, 536 context cases, no regressions and no input-integrity errors. This
gate measures regression against the checked-in baseline, not perfect recall
or independent real-user quality.

The final release comparison passes 120 calls across both datasets, including
warmups. Each table cell reports the median of five measured calls; memory is
the maximum sampled process peak RSS, in MiB. Both arms use the same retained
store, alternating execution order, with no competing build or fixture setup.

| Corpus | Request | Baseline seconds | E1 seconds | Baseline MiB | E1 MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| 100,000 files, one repository | Ordinary | 1.243 | 1.260 | 959 | 956 |
| 100,000 files, one repository | Filtered to 5% of files | 2.139 | 1.021 | 1,912 | 686 |
| 100,000 files, one repository | Empty filter | 1.980 | 1.049 | 1,664 | 681 |
| 100,000 files, one repository | Workspace | 1.292 | 1.262 | 957 | 956 |
| 100,000 files, one repository | Workspace, filtered | 2.206 | 1.056 | 1,910 | 682 |
| Two repositories, 5,000 files each | Ordinary | 0.455 | 0.460 | 127 | 127 |
| Two repositories, 5,000 files each | Filtered to 5% of files | 0.530 | 0.458 | 174 | 120 |
| Two repositories, 5,000 files each | Empty filter | 0.495 | 0.421 | 160 | 110 |
| Two repositories, 5,000 files each | Workspace | 0.567 | 0.568 | 141 | 141 |
| Two repositories, 5,000 files each | Workspace, filtered | 0.650 | 0.556 | 182 | 126 |

At 100,000 files, filtered queries take 52% less time and 64% less peak memory.
The smaller fixture improves filtered latency by 14–15% and peak memory by
31%. Ordinary and unfiltered workspace controls stay within about 2%.

An earlier candidate saved memory but had inconsistent speed on the smaller
fixture, so that run alone did not justify acceptance. It also prefetched
metadata for unfiltered requests before discovering an index fallback; that
ordering was removed. The final build was rechecked and measured separately.
E2 separately qualifies a graph dependency optimization below. No new
federation layer has been added.

These are synthetic document queries with graph and memory retrieval disabled.
The 100,000-file workspace cases still contain only one project. They establish
this optimization's benefit, not large mixed-language graph readiness or a
production latency guarantee. Raw samples, source/binary/harness hashes and
the exploratory results are in
[`evidence/large-repository-e1-2026-11-06.json`](evidence/large-repository-e1-2026-11-06.json).

The reproducible CLI harness is `bench/large_repositories.py`. `prepare` creates
only synthetic sources in a new directory; `compare` alternates saved baseline
and candidate executables against the same store, checking every result. It
retains individual packets, phase logs, observations and binary hashes. Setup
done alongside compilation is excluded from performance conclusions.

`engine_max_workers_env: 4` records `GRAPHICLE_MAX_WORKERS=4`; it is not a
process-wide thread limit. Native memory work uses up to 16 threads, bounded
by available CPUs and work items. Earlier raw results called the environment
setting `workers`; their recorded timings are unchanged.

```sh
python3 bench/large_repositories.py --binary /path/to/baseline \
  --output /tmp/new-scale-fixture prepare --files 5000 --projects 2
python3 bench/large_repositories.py --binary /path/to/baseline \
  --output /tmp/new-scale-fixture compare --candidate /path/to/candidate \
  --label e1 --samples 5
```

### E2: graph dependency profiling

`bench/graph_scaling.py` generates two Python corpora: unique function names,
and a common function name called within every file. It runs cold admission,
repeated unchanged syncs and single-file edits, then a rename and deletion.
Each result must have exactly the generated files, declarations, containment
edges and local call edges, with no unresolved local calls. Phase logs and
extraction/persistence work counts are retained. An unchanged sync reuses an
episode and does not report that episode's older extraction as current work.

A small diagnostic confirms the suspected dense dependency representation:
100 files with a common function name produce 9,900 conservative dependency
links and one strongly connected component, while the code graph itself has
300 edges. The dependency index is separate from the output code graph.
This is a structural finding, not a measured optimization. The experiment must preserve conservative file
reachability and invalidation even if it compresses the internal links.

The controlled 5,000-file run passes all 18 graph checks. Both corpora produce
15,000 nodes and 15,000 output edges before deletion. The sparse corpus has no
cross-file conservative dependency links; the common-name corpus has
24,995,000. A single-file edit invalidates one file in the sparse corpus and
all 5,000 files in the common-name corpus. Every recorded episode's complete
source-head hash map also matches the observed corpus for that operation.

| Operation | Samples per corpus | Sparse seconds | Common-name seconds | Sparse peak MiB | Common-name peak MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Cold admission and graph | 1 | 18.388 | 20.464 | 563 | 1,319 |
| Unchanged sync | 3 | 1.260 | 1.156 | 49 | 49 |
| One-file edit | 3 | 5.497 | 10.597 | 444 | 1,275 |
| Rename | 1 | 5.552 | 10.073 | 448 | 1,299 |
| Delete | 1 | 5.278 | 10.001 | 388 | 1,240 |

Times are medians where repeated; peak memory is the maximum sampled process
RSS. These are baseline workload measurements, not an A/B optimization result.
The dense representation alone would need approximately 10 billion neighbor
entries for this pattern at 100,000 files, before its reverse adjacency and
other graph state. That count is a structural projection, not a completed
100,000-code-file run. Compressing the links can reduce storage and traversal
cost, but must not silently reduce the conservative re-resolution set.

Raw rows, phase times, work counts and corpus/harness/binary hashes are in
[`evidence/large-repository-graph-baseline-2026-11-06.json`](evidence/large-repository-graph-baseline-2026-11-06.json).

```sh
python3 bench/graph_scaling.py --binary /path/to/graphicle \
  --output /tmp/new-graph-scale-fixture --files 5000 --samples 3
```

This probe's topology oracle is deliberately narrow: synthetic Python local
calls do not qualify mixed-language resolution, every graph attribute,
cross-repository links or equality with an independent full rebuild.

#### E2 candidate and acceptance checks

The candidate keeps a direct dependency for a name with one provider. A shared
name instead gets an internal vertex: each user points to that vertex, which
points to all providers. For a name used and provided by every file, this stores
2N links instead of N(N−1). The existing component algorithm and conservative
closure still run over this graph; only file vertices reach the resolver or
appear in file counts. Provider input order is retained. Internal vertices
participate in component hashes under cache schema `resolution-files-v15`;
old cache entries are misses, with no persisted graph format change.

Tests compare file adjacency, reachability, component membership and content
invalidation with an independent all-pairs reference. Common-name extraction
is compared with full extraction after edits, renames and deletions, alongside
the existing multi-language/cache-corruption/relocation regressions. The
candidate must still invalidate every conservative dependent; reducing that
set is not this experiment's optimization.

`bench/compare_graph_scaling.py` restores the same admitted store at the same
path for each arm and sample, preserving repository IDs and materialization
paths while letting each binary create its own caches. It alternates arm order
and compares every canonical node/edge attribute and graph source-head hash
after cold graph build, unchanged sync, edit, rename and deletion. Extraction
work counters and persistence timestamps are excluded from graph equality.
Initial source admission is setup, outside the cold-graph timing. A run using
the baseline in both arms validates the harness before comparing candidates.
Cache upgrades and rollbacks are checked against the same edited graph,
outside the timing samples. Disposable stores use native copy-on-write on
macOS where available; other filesystems use an ordinary independent copy.

Correctness checks: 12 incremental-engine tests, 16 resolution/route
tests and 46 runtime graph/workspace/passage-index tests pass. The harness
self-check passes 60 matched calls and four cache-switch checks. The actual
5,000-file release comparison also passes all 60 graph checks and four cache
upgrade/rollback checks. Both versions still resolve all 5,000 common-name
files after an edit or rename, and all 4,999 remaining files after deletion.

E2 is retained for its substantial memory benefit. Each table cell reports
the median of three calls and the maximum observed process peak RSS. The
original raw samples, including timing variability, remain in the receipt.

| Corpus | Operation | E1 seconds | E2 seconds | E1 peak MiB | E2 peak MiB |
| --- | --- | ---: | ---: | ---: | ---: |
| Sparse names | Cold graph | 24.100 | 23.794 | 539 | 535 |
| Sparse names | Unchanged | 1.375 | 1.415 | 49 | 49 |
| Sparse names | Edit | 11.162 | 11.227 | 447 | 442 |
| Sparse names | Rename | 6.856 | 5.659 | 444 | 445 |
| Sparse names | Delete | 6.342 | 5.592 | 401 | 409 |
| Common name | Cold graph | 24.432 | 23.189 | 1,303 | 526 |
| Common name | Unchanged | 1.260 | 1.379 | 49 | 49 |
| Common name | Edit | 13.674 | 13.766 | 1,291 | 519 |
| Common name | Rename | 11.665 | 10.443 | 1,273 | 518 |
| Common name | Delete | 11.095 | 10.957 | 1,378 | 503 |

Common-name builds and updates use 59–64% less peak memory. Median cold graph
and rename time improve by 5% and 10%; single-file edit time is essentially
unchanged. These results do not establish a general latency improvement.
The first sparse cold-build sample was 58.432 seconds, versus 20.861 and
24.100 for the other baseline samples; it is retained, not discarded.

An unchanged-sync follow-up runs seven measured calls per arm/corpus, plus
warmups, alternating executables against the same unchanged store. All 32
calls retain the same episode and exact canonical graph. Baseline/candidate
medians are 1.354/1.335 seconds for sparse names and 1.372/1.305 seconds for
the common name. The initial unchanged slowdown does not reproduce. RSS is
compared within each experiment: the follow-up's process-accounting floor
is higher, while CLI phase logs still report about 49 MiB.

The later harness correction launches each measurement through a fresh Python
worker. A real-CLI reproducer confirmed that launching after a large graph
snapshot can include inherited pre-exec memory in `wait4`'s high-water mark:
one direct launch reported 193 MiB, while all three isolated launches reported
about 49 MiB. Both versions of the main common-name experiment exceed this
floor by a wide margin. Historical numbers are retained as measured. Exact
original scripts are preserved in
[`evidence/large-repository-harness-v1-2026-11-06.json`](evidence/large-repository-harness-v1-2026-11-06.json);
the resource-accounting reproduction and success/error/timeout checks are in
[`evidence/large-repository-resource-accounting-2026-11-06.json`](evidence/large-repository-resource-accounting-2026-11-06.json).

Full raw samples, source/binary/harness hashes and migration results are in
[`evidence/large-repository-e2-2026-11-06.json`](evidence/large-repository-e2-2026-11-06.json).
This accepts dependency compression at the measured scale. It does not
qualify 100,000 code files, mixed-language completeness or ten repositories.

A copied CPython 3.15.1 corpus adds real-code qualification. It contains 2,062
Python files; normal runtime admission keeps 2,057. Three non-UTF-8 fixtures
and the two `venv` files are not admitted and remain recorded. The E1 release
builds a 96,231-node, 198,626-edge graph with no incremental fallback. At this
stage, a separate debug equivalence checker aborted with stack overflow,
including when full extraction ran before the incremental path. E5 below
resolves that failure. The release control/candidate comparison on the admitted corpus passes
after renaming the `colorsys.rgb_to_yiq` API: all canonical graph data is equal
(96,231 nodes, 198,624 edges), including when E2 replaces the v14 resolution
cache. This release comparison establishes control equivalence; E5 separately
adds successful full/debug qualification.
Neither diagnostic timing is an optimization result. The actual E1/E2 smoke
comparison also passes 60 matched calls and four upgrade/rollback checks.

```sh
python3 bench/compare_graph_scaling.py \
  --baseline /path/to/e1 --candidate /path/to/e2 \
  --output /tmp/new-paired-graph-fixture --files 5000 --samples 3
```

### E5: Python receiver traversal

An isolated receiver-fact probe reproduces the stack overflow while visiting
CPython's `test/test_traceback.py`, after completing 1,779 files. The same
original implementation completes all 2,060 engine-collected files with a
32 MiB diagnostic worker stack. The production fix replaces `PyWalker::walk`
recursion with a heap traversal, following the existing Go walker pattern.
Explicit exit markers restore class and function scopes. Class/function body
selection, assignment right-hand-side selection, visit order and binding order
remain unchanged.

The fixed walker completes all 2,060 files on the default worker stack. Every
complete receiver-fact hash, source hash, count and parse diagnostic matches
the original larger-stack run. Twenty focused tests pass: twelve incremental
tests, six receiver-resolution tests, the existing Go depth regression, and a
new Python regression. The latter visits a 12,000-term expression on a 512 KiB
stack and checks nested classes/functions, field ownership, shadowed bindings,
call order and enclosing scopes.

The full/debug checker now passes exact graph equality on the same corpus:

| Operation | Files | Nodes | Edges | Full/incremental equal | Fallback |
| --- | ---: | ---: | ---: | --- | --- |
| Cold cache | 2,060 | 96,712 | 202,063 | Yes | None |
| Rename `colorsys.rgb_to_yiq` to `rgb_to_yiq_probe` | 2,060 | 96,712 | 202,061 | Yes | None |

The edit parses one changed file and reuses 2,059 file records. Conservative
resolution still includes 2,022 dirty files and 38 context files; their graph
matches a full extraction. The disposable fixture's original bytes are
restored after the checks.

These are debug correctness results, not release performance measurements.
The engine checker enables receiver resolution and test identities, includes
three non-UTF-8 fixtures decoded by the engine, and excludes the two `venv`
files. Its graph counts are not directly comparable with the runtime's
2,057-file admission probe. JavaScript traversal and recursive type helpers
are unchanged. Larger code-graph, workspace and final release qualification
remain open.

The reproduction, hashes, test results and graph receipts are in
[`evidence/large-repository-e5-2026-11-06.json`](evidence/large-repository-e5-2026-11-06.json).

### Larger code-graph qualification

The combined E1/E2/E5 release is saved separately with source and binary
hashes. `bench/qualify_large_graph.py` checks a common-name corpus without
constructing the old quadratic dependency representation. It measures source
admission, cold graph construction, repeated unchanged refreshes and edits,
rename/deletion, and one-percent batch edits/deletions. Every step checks the
complete generated topology, current source hashes and conservative dirty set.
The final graph must equal a rebuild from an admitted but unprojected seed at
the same store path, including all canonical node and edge attributes.

It also checks a symbol-specific context request against exact source bytes
and its token budget, and verifies graph-navigation citations for the caller
and callee. The 20-file harness smoke test and the 25,000-file qualification
both pass all 15 calls. The larger corpus initially contains 2,327,780 source
bytes, 75,000 nodes and 75,000 edges. These are tiny synthetic files, not a
realistic mixed-language repository of the same file count.

| Operation | Samples | Seconds | Peak MiB |
| --- | ---: | ---: | ---: |
| Admission without graph | 1 | 33.950 | 283 |
| Cold graph | 1 | 91.921 | 2,285 |
| Unchanged refresh | 3 | 4.784 | 127 |
| Single-file edit | 3 | 63.444 | 2,219 |
| Rename | 1 | 61.267 | 2,189 |
| Delete | 1 | 59.862 | 2,015 |
| Edit 250 files | 1 | 60.659 | 2,195 |
| Delete 250 files | 1 | 63.197 | 2,103 |
| Fresh-cache final rebuild | 1 | 99.544 | 2,333 |
| Context with graph retrieval | 1 | 0.995 | 879 |
| Caller/callee navigation | 1 | 1.886 | 852 |

Repeated rows report median time and maximum peak RSS. All changed builds
conservatively re-resolve every remaining file, with no fallback. The final
24,749-file graph equals the fresh-cache graph, including every canonical
node/edge attribute and source head. That rebuild uses the same incremental
implementation with empty caches; the independent full-extraction comparison
is the separate CPython check above. Cited reads match exact source bytes and
stay within their token budget.

The shared-name edit remains expensive: a one-file edit takes about a minute.
Memory compression did not remove that conservative resolution work. The
100,000-file run below also passes; ten-repository and expanded-context qualification
remain open. This is feasibility and correctness evidence, not an A/B speedup
or mixed-language completeness. Raw rows and provenance are in
[`evidence/large-repository-25k-2026-11-06.json`](evidence/large-repository-25k-2026-11-06.json).

The 100,000-file qualification passes all fifteen calls. Its initial corpus
is 9,377,780 bytes, producing 300,000 nodes and 300,000 edges. Every topology,
source-head and conservative dirty-set check passes. After all changes, the
98,999-file graph has 296,997 nodes and edges and equals the fresh-cache
rebuild exactly. Context citations match source bytes and fit the token budget;
navigation returns the expected caller/callee with valid current-source spans.

| Operation | Samples | Seconds | Peak MiB |
| --- | ---: | ---: | ---: |
| Admission without graph | 1 | 195.245 | 883 |
| Cold graph | 1 | 494.891 | 5,852 |
| Unchanged refresh | 3 | 28.504 | 442 |
| Single-file edit | 3 | 308.659 | 5,780 |
| Rename | 1 | 304.642 | 6,187 |
| Delete | 1 | 297.490 | 5,744 |
| Edit 1,000 files | 1 | 299.031 | 5,630 |
| Delete 1,000 files | 1 | 292.831 | 5,771 |
| Fresh-cache final rebuild | 1 | 468.494 | 6,145 |
| Context with graph retrieval | 1 | 3.914 | 3,650 |
| Caller/callee navigation | 1 | 12.698 | 3,250 |

Repeated rows report median time and maximum peak RSS. Every update still
resolves all remaining files without fallback. These observed costs are too
large to infer an interactive update guarantee. The corpus has tiny Python
files and a shared name; it does not qualify realistic mixed-language size or
independent retrieval quality. Fresh-cache equality uses the same incremental
implementation; independent full-extraction evidence remains the CPython check.
Raw results and provenance are recorded in
[`evidence/large-repository-100k-2026-11-06.json`](evidence/large-repository-100k-2026-11-06.json).

```sh
python3 bench/qualify_large_graph.py --binary /path/to/combined-release \
  --output /tmp/new-large-graph-fixture --files 100000 --samples 3
```

### E3 and E4 workspace and context qualification

`bench/qualify_workspace.py` is a smoke-validated harness for ten uneven
repositories. It uses three library repositories and seven consumers across
Python, TypeScript and Go. Forty times `--units` gives the generated code-file
count, in addition to manifests and import probes; ten percent of those files
live under `generated/`. Go consumers use a local replacement pointing to the
same synthetic library whose bytes are admitted as a separate repository.
The compiler resolver's existing offline policy applies. Go filler sources
share packages in groups of twenty inputs, split into eighteen ordinary and
two generated files in separate package directories.

The harness checks all generated local calls (including the absence of
contradictory unresolved-call records), source heads, linked symbols
and their episode bindings, identical repeated workspace context with exact citations,
the default eight-project cap, and explicit fanout to ten projects. It also
checks stale/deleted exporter sources and access revocation using the existing
HTTP grant and CLI access APIs. Its 40-generated-file smoke passes all 73
calls, including exact diagnostics for the three consumers of the deleted
Python module. Two earlier smoke attempts corrected harness assumptions about
Go diagnostic fields and the direction of unresolved-import reporting; no
runtime change was needed. This verifies the harness at small scale and adds
no runtime federation layer. The receipt is
[`evidence/large-repository-workspace-smoke-2026-11-06.json`](evidence/large-repository-workspace-smoke-2026-11-06.json).
The follow-up passes all 73 calls against the retained E6 binary, with 100,000
generated files plus 21 manifests/probes (12.23 MB) across ten repositories.
All 100,000 generated local calls, seven cross-repository import probes, source
hashes, episode bindings, citation bytes, repeated packets and budgets pass.
The default fanout reports two omitted projects; explicit fanout searches all
ten. Revoking a library leaves nine readable projects without leaking its ID.
Stale and deleted exporter checks pass, including the three unresolved Python
imports after deleting their shared module.

| Workspace question | Median without cross-repository lift | Median with lift |
| --- | ---: | ---: |
| Python consumer | 2.21 s | 2.82 s |
| TypeScript consumer | 2.16 s | 2.89 s |
| Go consumer | 2.18 s | 2.91 s |

Each cell has three fresh-process calls over all ten projects, with a warm OS
cache. Maximum context peak RSS across all checks is 2.35 GiB. These are observed costs,
not a production SLO or an A/B optimization result. Host load observations are
retained in the receipt; no assistant build, test or competing benchmark ran
during these measurements.

The run also exposes expensive cold export linkage. The 15,000-file TypeScript
consumer's cold sync takes 182.68 seconds, including 49.18 seconds in linking;
the 20,000-file Go consumer takes 214.97 seconds, including 53.09 seconds in
linking. The import-link transaction holds its write lock for approximately
those linking durations. Source inspection finds that `symbols_of` scans all
graph nodes for every exported file. This is a concrete follow-up candidate,
not a measured or accepted optimization. The completed receipt is
[`evidence/large-repository-workspace-100k-2026-11-06.json`](evidence/large-repository-workspace-100k-2026-11-06.json).

All generated local calls resolve, and all seven cross-repository import
probes link to their expected symbols. The Go probe graphs still report two
`type_information_unavailable` records at numeric conversions and one
`target_not_in_graph` in each Go consumer for its external-library call.
Successful cross-repository import linkage does not establish a complete
compiler call graph across repositories; these diagnostics remain recorded.

Expanded and memory-linked requests have a separate scale qualification. The full
retrieval path limits a query to 50,000 documents and 40,000 passages; past
those bounds it selects documents by the question and reports omissions.
The follow-up tests symbols from the beginning, middle and end of
the corpus, beyond the original `run_7` probe. Ordinary indexed retrieval
and expanded retrieval have different bounds and must be reported separately.

`bench/qualify_context.py` passes its 90-call, 18-file smoke check. It clones
a completed large-graph fixture's store and adds three source-bound reviewed
notes through the CLI. It directly seeds three exactly quoted observer records
in that disposable clone to exercise `note_keys: "merge"` without model calls;
observer authoring and invalidation are outside this fixture. Seven context
workloads (source-only, graph, memory, memory with graph,
query variants, key expansion and source filtering) must retain the labelled
source near the beginning, middle and end of the corpus, exact citation bytes,
the requested note, and the token budget. It checks identical repeated packets
and separates warmups from measured calls. It requires the graph and variant
arms to run and observer keys to be applied. These few notes test retrieval
against a large source corpus; they do not qualify a large memory catalogue,
note-only question recall or observer-note generation.
The smoke also checks exact caller/callee declaration spans against Python's
AST. Its receipt is
[`evidence/large-repository-context-smoke-2026-11-06.json`](evidence/large-repository-context-smoke-2026-11-06.json).
The harness now copies SQLite and query caches while omitting only the
materialized sources and parse/resolution caches used by graph builds. All 90
smoke calls pass with those build caches absent throughout. This changes
fixture preparation only.

The larger run passes all 90 calls on the completed 100,000-file graph fixture:
98,999 sources and 9.28 MB after its deletion checks, plus three reviewed notes
and three seeded observer records. Each of seven context workloads has one
excluded warmup and three measured calls for each of three source probes.
All 84 context packets retain their labelled source, exact citation bytes,
requested memory, token budget and deterministic repeated result. The other
six calls verify navigation spans and create the notes. This run uses E5,
matching the fixture; E6 changes graph-build materialization, not these reads.

| Request | Range of probe median times | Maximum measured peak RSS |
| --- | ---: | ---: |
| Source only | 1.16–1.18 s | 917 MiB |
| Reviewed memory, without graph retrieval | 1.18–1.22 s | 916 MiB |
| Graph retrieval | 3.61–3.67 s | 3,686 MiB |
| Reviewed memory and graph retrieval | 3.66–3.71 s | 3,693 MiB |
| Query variants | 2.76–2.92 s | 3,634 MiB |
| Observer key expansion | 2.82–2.84 s | 3,648 MiB |
| One-file filter, with memory and graph | 2.01–2.03 s | 3,120 MiB |

These are feature costs, not comparable A/B optimization arms. Source-only and
indexed-memory requests search all 98,999 indexed passages. The full-path
workloads select 40,000 passages by the question and explicitly report 58,999
omitted documents; all three probes still retrieve their labelled evidence.
Their local property-graph walk reports `degraded` at 98,999 nodes against a
50,000-node bound, while the code-graph arm runs. Filtering reports one source
and no omissions, but still incurs substantial graph-loading memory. No
runtime change or independent quality claim follows from this qualification.
The receipt includes all samples and an audit of every packet's coverage:
[`evidence/large-repository-context-100k-2026-11-06.json`](evidence/large-repository-context-100k-2026-11-06.json).

### Update phase diagnostic

After the runtime and quality suites completed, an isolated
`GRAPHICLE_PROFILE=1` run edited one file in the completed 24,749-file fixture.
The store clone retained the original cache path. All topology, source-head
and dirty-set assertions passed; the original source bytes and store were
restored, and restoration was checked. Cloning was setup, excluded from the
89.36-second diagnostic operation.

| Phase | Seconds |
| --- | ---: |
| Source refresh read | 8.78 |
| Graph source materialization/verification | 24.84 |
| Incremental extraction | 45.46 |
| Graph identity | 6.17 |
| Graph persistence | 0.43 |

Only one source was written during materialization; 24,748 were reused. Only
one graph persistence group changed (three nodes and three edges compared),
but conservative resolution still processed all 24,749 files. Inside the
45.46-second extraction span, cached parser-record loading took 12.98 seconds
and resolver assembly took 7.59 seconds, including 6.29 seconds for receiver
analysis. These nested spans are not additional time on top of extraction.

This narrows the next investigation to whole-corpus validation, conservative
resolution and cache work. It does not isolate dependency construction,
partitioning, hashing and per-file cache writes individually, so attributing
all remaining extraction time to cache writes would be unsupported. No further
runtime optimization follows from this diagnostic alone. It is one profiled
run, not an A/B improvement or production latency guarantee. The complete
profile, hashes and restoration receipt are in
[`evidence/large-repository-profile-2026-11-06.json`](evidence/large-repository-profile-2026-11-06.json).

### E6 retained: parallel source-cache verification

The materialization layer hashes existing files through the engine's existing
ordered parallel map and `io_workers` policy. Database reads, corrupt/missing
file repairs and atomic replacements remain sequential in source order, under
the source-tree lock. Every reused file is still content-hashed; there is no
new option, dependency or persisted format. The candidate does not change the
resolver's conservative invalidation set.

All five source-cache tests and 30 graph projection/serving regressions pass.
The additional test checks 128 files, including missing materialization,
same-size/same-mtime tampering, a file larger than the hashing buffer, and a
corrupt authoritative object. Existing tests cover symlink rejection,
deletion/rename transitions and concurrent-builder isolation.

The first build invocation ran from the repository root and missed
`crates/graphicle/.cargo/config.toml`, producing a binary without the configured
bundled parsers. Its graph tests failed at source admission. That artifact is
excluded; the corrected build/test workflow runs from `crates/graphicle`, as
the repository CI does. The corrected release also passes 60 paired smoke
calls and four cache upgrade/rollback checks on two 30-file corpora. Every
canonical graph attribute and source head matches the E5 control.

The 5,000-file comparison also passes all 60 matched calls and four cache
upgrade/rollback checks, with three samples per arm/operation. For common-name
files, median edit time is 11.21 to 10.75 seconds, rename 10.34 to 9.69 seconds,
and deletion 10.16 to 9.68 seconds. Materialization medians for those operations
fall from 0.77–0.83 seconds to 0.40–0.41 seconds. Peak memory is similar.
Cold-build results vary in direction between the two corpora, so this does
not establish a cold-build speedup. Every raw sample is retained.

The follow-up `bench/compare_graph_edits.py` compares repeated edits using
separate retained caches at the same absolute working path. It parks stores by
directory rename between arms, excludes one warmup edit per arm, and alternates
arm order across measured edits. It checks complete canonical graphs, current
source hashes, materialization and parse counts, and conservative resolution
sets on every call. Baseline-versus-baseline and baseline-versus-candidate
30-file checks both pass (11 calls and ten graph checks each).

The 25,000-file common-name comparison passes all 15 calls and 14 complete
graph checks, including setup and warmup. Each arm performs five measured
single-file edits. Every candidate edit is faster than its paired control,
and all 25,000 files remain in the conservative resolution set.

| Measure | E5 control | E6 |
| --- | ---: | ---: |
| Median complete edit | 68.57 s | 61.56 s |
| Individual complete edits | 64.60–76.33 s | 58.90–63.95 s |
| Median source materialization/verification | 6.35 s | 2.63 s |
| Maximum process peak RSS | 2,245 MiB | 2,216 MiB |

Retain E6: total edit time improves 10.2% and verification 58.7%, with similar
peak memory and unchanged graph contents. This reuses an existing helper and
adds no cache format or option. It does not establish a cold-build improvement
or a measured E6 improvement at 100,000 files, and does not remove the remaining
whole-corpus source and resolution costs. Candidate and test provenance are in
[`evidence/large-repository-e6-2026-11-06.json`](evidence/large-repository-e6-2026-11-06.json).

### E7 retained: community grouping positions

`graphicle::serve::communities` groups nodes for `GraphContext`, which navigation
constructs after clustering. Its growing vector was searched for each node.
The candidate records each community ID's position in a standard hash map,
while retaining the vector's first-encounter order and each group's node order.
Numeric/string ID conversion and invalid-ID errors are unchanged. No navigation
format, clustering policy or persisted graph changes.

Focused checks cover ID coercion, missing/null IDs, invalid IDs and 20,000
interleaved communities. All three pass, along with eight engine serving
compatibility tests and 23 runtime navigation/server tests. Formatting checks
also pass. The release build is complete; its source manifest differs from E6
only in `serve.rs`. Baseline-versus-baseline and baseline-versus-candidate
navigation smoke checks each pass all eight complete packet comparisons and
independent caller/callee span checks.

The large comparison passes all 12 complete packet checks on the retained
98,999-file graph, including one excluded warmup per arm. Every candidate sample
is faster across five measured pairs. Median fresh-process navigation falls
from 12.37 to 8.68 seconds (29.8%); maximum peak RSS rises from 3,262 to 3,284 MiB
(0.7%). Retain the change: it removes repeated community scans while preserving
all text, citations, IDs and ordering. Whole-graph loading still accounts for
substantial time and memory. This does not measure an already warm server or
claim a graph-build speedup. E3 used E6, before this navigation-only change.
Provenance is in
[`evidence/large-repository-e7-2026-11-06.json`](evidence/large-repository-e7-2026-11-06.json).

### E8 retained: export symbols grouped by file

`cross_repo_imports::symbols_of` previously scanned every graph node for each
exported file and repeatedly normalized its path. The candidate extends the
existing ordered file grouping with node indices in graph encounter order.
Symbol extraction visits only the members of that file. The preferred module
node, `relpath`/`source_file` precedence, path normalization, external-node
exclusions, language filters and first-symbol tie winner remain unchanged.
There is no new public API, persisted format or runtime switch.

All 22 export/import unit tests pass, including three new tests for those
invariants and 2,000 interleaved TypeScript files. Eight runtime cross-repository
tests and 49 workspace-related tests pass. One unrelated distributed-consumer
test remains ignored because it requires an isolated Redis/Valkey server.
These tests include incremental/full-link equality, revocation, freshness,
scratch-project ownership and HTTP-route linkage.

The controlled comparison uses `bench/compare_export_linking.py` on disposable
copies of the completed E3 workspace. SQLite backup captures committed WAL
pages. Each arm restores the same seed at the same path and removes only
derived `gf_cross_link_state` rows during untimed setup. The real `graph.link`
call then rebuilds export indexes and relinks imports; an unchanged cached
link follows. Full canonical export indexes, all cross rows except creation
timestamps, complete response packets and source/graph/access rows are checked.
Arm order alternates, and one warmup per arm is excluded. Retained graph caches
belong to the older E3 build, so both binaries fall back to the recorded graph.

Baseline-versus-baseline and E7/E8 smoke comparisons each pass all 16 calls.
The large comparison passes all 24 calls on ten repositories with 100,000
generated files and 20 remaining manifests/probes. Every export index, response,
cross edge and unresolved import matches the qualified fixture. The source,
episode, graph and access tables remain unchanged, and the original fixture's
final integrity check passes. No build or test ran alongside measurements.

| Measure | E7 control | E8 |
| --- | ---: | ---: |
| Median export-index rebuild and relink | 100.89 s | 3.67 s |
| Individual measured rebuilds | 99.60–218.60 s | 3.44–7.61 s |
| Maximum rebuild process peak RSS | 2,011 MiB | 2,011 MiB |
| Median unchanged cached link | 0.790 s | 0.783 s |

Retain E8: the rebuild/relink median falls 96.4%, and every one of the five
candidate samples is faster than its paired control. Rebuild peak memory is
essentially unchanged (16 KiB difference in the sampled maxima). Cached linking
has no demonstrated performance change. Both arms' early calls are slower than
their later calls; all samples are preserved, and these times are not SLOs.
The fixture retains the prior Python-library deletion, including its three
unresolved imports and four live cross edges. This comparison preserves the
existing language-linking semantics; it does not independently qualify all
language exports, parsing, real repositories, concurrent writers or warm-server
latency. Evidence, build provenance and raw samples are in
[`evidence/large-repository-e8-2026-11-06.json`](evidence/large-repository-e8-2026-11-06.json).

### E9 retained: verified navigation snapshot reuse

Navigation now tries the existing `query_graph::read` before reconstructing an
episode from SQLite. That helper validates the project, episode, source-build
keys, snapshot format and checksum, and falls back for missing, disabled,
outdated or damaged caches. Its cached search index is passed to the existing
`GraphContext::with_index`; clustering only adds a top-level community field,
which that search index does not read. The serialized resident build, access
checks on every call, clustering and source/citation validation are unchanged.
No new persisted format, public API, switch or cache writer is introduced.

Two new CLI tests pass: all 11 tools that use the resident graph produce the
same complete packets with a valid snapshot and with snapshots disabled;
missing, corrupt, truncated, wrong-build and restored snapshots preserve
responses. Damaged files are discarded by the existing helper, while a
wrong-build file is left alone. Historical and stale-source requests keep their
episode lag, unchecked-relation and citation diagnostics. The existing 23
server tests and 22 workspace-scope tests also pass. The changed resident
function and the full query-file test file pass formatting; unrelated existing
formatting in `graph_tools.rs` is preserved.

`bench/compare_navigation_cache.py` prepares each binary's cache through the
existing `pack` operation in separate disposable copies of a qualified store.
Stores are parked by directory rename so requests use the same absolute path.
It compares search and caller/callee packets and independently checks citation
spans against Python ASTs. Setup and full source/graph/access integrity checks
are excluded from timing. The self-check and E8/E9 smoke comparison each pass
16 navigation calls plus two cache-preparation calls.

The 98,999-file comparison passes all 24 navigation calls plus both preparation
calls. It excludes one warmup per arm/operation and alternates arm and operation
order across five measured samples. Every candidate sample is faster than its
paired control. All packets, independent AST spans, source/graph/access rows
and saved graph files pass verification; the original fixture is unchanged.

| Operation | E8 median | E9 median | E8 max RSS | E9 max RSS |
| --- | ---: | ---: | ---: | ---: |
| Caller/callee navigation | 8.76 s | 6.60 s | 3,294 MiB | 3,095 MiB |
| Symbol search | 9.21 s | 7.10 s | 4,187 MiB | 3,664 MiB |

Retain E9: wall time improves 24.6% and 22.9%, respectively, with 6.1% and
12.5% lower maximum peak RSS. Cache preparation takes 6.47 seconds for E8 and
6.63 seconds for E9 in this run and is excluded from navigation timings; each
saved graph is approximately 136 MiB. These caches already belong to the normal
sync/pack workflow. The change adds no cache preparation to navigation.

This is a separate paired comparison against E8; prior absolute timings must
not be combined into a cumulative speedup claim. It measures first-request
resident loading with a warm OS cache and a compatible saved graph. Already
warm server requests, cold OS caches, parsing, graph builds and source updates
are outside the performance claim. Whole-graph memory remains substantial.
Provenance and every sample are in
[`evidence/large-repository-e9-2026-11-06.json`](evidence/large-repository-e9-2026-11-06.json).

### E10: real Babel repository qualification

`bench/qualify_babel.py` uses Babel commit
[`992afd97a0535c1fa8a3d9eb33e8b5fde4ad2c55`](https://github.com/babel/babel/tree/992afd97a0535c1fa8a3d9eb33e8b5fde4ad2c55).
The shared checkout is read-only. Each run creates a new source copy and a
local Git index to retain tracked-file admission semantics, with no commits,
hooks, remote configuration or package execution. All 29,280 tracked files
were copied: 56,552,075 bytes, including 15,233 JavaScript, 1,724 TypeScript,
1,573 MJS and 9,897 JSON files. This is a real monorepository dominated by
parser and transformation fixtures, including intentionally invalid syntax;
it is not a representative application-only corpus or a 100,000-file test.

Default admission indexes 29,217 files and 47,619,441 bytes. The complete
inventory comparison explains all 63 omissions:

- 61 tracked fixture files are under `node_modules`, which normal folder
  discovery excludes. They are not included in the sync's `excluded` counter.
- An 8,928,146-byte TypeScript benchmark text file exceeds the default 8 MiB
  file limit and is explicitly reported as skipped.
- One parser fixture contains a NUL byte and is explicitly excluded as binary.

The existing code projection also omits 22 YAML files while retaining their
text for retrieval. Of the remaining extraction inputs, 506 have no extractor.
The final graph records 11,179 unresolved calls. Passing these probes does not
mean every language construct, test fixture or dependency resolves.

The 123-file `babel-types` smoke run and full run each pass 16 CLI calls.
Each performs admission, a first graph, three unchanged refreshes, three
single-file function renames, deletion of `isValidES3Identifier.ts`, a
same-path fresh-cache rebuild, three context requests and three navigation
requests. Source-reviewed local and imported calls remain present, renamed
declarations replace their old identities, and deleted declarations disappear.
All admitted source hashes match the files and each graph's source heads.
The final graph's complete canonical attributes, nodes and edges equal the
fresh rebuild, as do all unresolved-call records. Only extraction work
counters are excluded from the graph comparison.

The first graph has 59,415 nodes and 72,570 edges; after deletion it has 59,414
nodes and 72,565 edges. Updates use the existing
`language_or_external_context_requires_full_resolution` fallback. Each rename
rewrites one source file but resolves 29,195 inputs, re-extracting 18,562 files
because the full extractor bypasses JavaScript/TypeScript record caches. Only
one persistence group is dirty. This qualifies graph updates and persistence
under that fallback, not selective incremental resolution for this mixed
corpus. Broadening its language gate without proving dependency safety would
not be a valid optimization.

| Operation | Observed wall time | Maximum process peak RSS |
| --- | ---: | ---: |
| Source admission | 28.50 s | 914 MiB |
| First graph after admission | 71.45 s | 2,810 MiB |
| Unchanged refresh, median of three | 9.32 s | 234 MiB |
| Single-file rename update, median of three | 56.57 s | 2,624 MiB |
| Source deletion | 55.79 s | 2,563 MiB |
| Fresh-cache rebuild of final sources | 84.77 s | 2,848 MiB |
| Two distinct unfiltered graph-context queries | 3.74–6.31 s | 2,312 MiB |
| File-filtered graph-context query | 0.96 s | 809 MiB |
| Three distinct caller/callee navigation requests | 1.48–2.32 s | 802 MiB |

Rename samples range from 56.41 to 67.76 seconds; unchanged refreshes range
from 9.10 to 9.36 seconds. Other operations have one sample per request.
Every call uses a fresh CLI process, four engine workers and a warm OS cache;
source copying, hashing and verification are outside timing. No build, test,
other fixture preparation or benchmark overlaps the measurements. These are
observed operating costs, not an A/B speedup or production SLO. Navigation
includes first-request graph loading rather than an already warm server.

All context citations match exact source bytes and stay below 4,000 tokens
with no model calls. The returned passages contain the reviewed clone defaults
and the `traverseFast(tree, removeProperties, opts)` call. Navigation checks
current episodes, source hashes, valid byte ranges and six expected function
definition spans with correct paths and starting lines. No navigation node
is omitted or unmatched in these three probes. An untimed final audit rehashes
every original file, confirms only the intended disposable edits/deletion,
and passes SQLite `quick_check` on both retained stores.

Context coverage is bounded even at this file count: the two unfiltered
requests take the full retrieval path, reach 40,000 passages and report 5,096
and 4,778 omitted documents respectively. The expected answer sources are
included. The file-filtered request uses the passage index and covers both
passages of the selected file without omissions. This is useful source-grounded
retrieval evidence, not independent general relevance or complete corpus recall.

No runtime feature is added by E10. Keep its qualification harness and evidence;
the next update experiment must address the measured full-resolution cost
without removing source coverage or dependency correctness. Larger real
repositories, combined large source/memory workloads, concurrent writers and
warm server latency remain unqualified. E11 separately measures an isolated
memory catalogue. Build identity, raw measurements, omissions,
hashes and verification details are in
[`evidence/large-repository-e10-2026-11-06.json`](evidence/large-repository-e10-2026-11-06.json).

### E11: 25,000 reviewed notes, recall bounds and freshness

`bench/qualify_memory_catalogue.py` creates 40 tiny Python files, then authors
25,000 distinct reviewed decisions through the public project-scoped `note`
API. Every response has revision 1, the expected title/body and the original
file's dependency hash. No memory records are inserted directly into SQLite.
The setup server is stopped before timed retrieval. This isolates memory
catalogue size; it does not combine 25,000 notes with the 100,000-file graph.

An 80-note smoke run and the 25,000-note run each complete 50 context checks
plus three source-sync calls. Three probes target the oldest, middle and newest
notes. Ordinary memory, file-filtered, graph-assisted and memory-disabled
requests each get one excluded warmup and three measured samples, with workload
order reversed on alternating rounds. All repeated packets match exactly.
Citations, complete delivered note bodies, revisions, dependency data, filters,
3,000-token budgets and zero model calls are checked. A final audit confirms
all 25,000 API records exactly match their stored fields, with 25,000 history
rows and 25,000 matching file-dependency edges. SQLite `quick_check` passes.

The listing/dependency limit is 100,000 records, but ordinary recall admits
only the newest 20,000 current reviewed memories. With this catalogue, both
ordinary and graph-assisted requests explicitly report 5,000 omitted memories
and fail to retrieve the oldest probe. The middle and newest probes are
delivered. Filtering to the oldest note's source file reduces the eligible
catalogue to 625 notes, retrieves that note and reports no recall omissions.
These are observed consequences of the configured selection policy, not
independent general relevance measurements or a complete-catalogue guarantee.

| Request | Range of the three probe medians | Maximum measured peak RSS |
| --- | ---: | ---: |
| Ordinary memory recall | 1.05–1.20 s | 658 MiB |
| File-filtered memory recall | 0.68–0.90 s | 329 MiB |
| Graph-assisted memory recall | 1.51–1.77 s | 815 MiB |
| Memory and graph disabled | 0.67–0.72 s | 329 MiB |

Each probe median uses three fresh CLI processes, excluding its warmup. Peak
RSS is the maximum across the nine measured calls of that request type.
The OS cache is warm; authoring, verification and fixture preparation are
outside these measurements. No builds, tests, other fixture preparation or
benchmarks run alongside them. These feature costs are not an A/B speedup.
Memory-disabled requests still pay catalogue-loading costs; the current
snapshot and indexed-query paths load the memory catalogue for state and
dependency checks before selection. Any optimization must preserve those
bindings and authorization semantics.

The final two context checks exercise a source edit and restoration through
normal sync. The edit withholds exactly 625 dependent notes, including the
newest probe. Restoring the original bytes makes the same authored revisions
eligible again, and the response matches the original in every field except
`freshness`. That diagnostic still reports 625 stale memories, while coverage
reports zero stale memories and the returned note is current. The 80-note
smoke run reproduces the same discrepancy with two dependent notes.

**Open finding E11-F1:** source-byte reversion restores canonical memory
eligibility but leaves memory entries in `gf_stale`. `memory::list_of` checks
current dependency hashes; `invalidation::repository_freshness` reports the
event-driven ledger, whose memory markers are cleared by `record_memory` on
save. Graph-node markers already have a source-reversion reconciliation path.
No stale note was delivered during the changed-source check, but the two
freshness signals disagree after restoration. This is not a clean freshness
qualification, and no runtime fix is included in E11.

The first smoke attempt exposed a harness request error: a project-scoped URL
rejects a duplicate `repository_id` in its body, before any notes are written.
The second completed the retrieval calls but failed its full restoration-packet
equality assertion, exposing E11-F1. The final harness retains and reports that
diagnostic difference explicitly, while requiring all other restored fields
to match and all unchanged reads to match in full. Both earlier logs are kept.

Keep the harness and evidence, with the recall bound and E11-F1 unresolved.
There is no new runtime feature. Independent user questions, larger real
memory collections, combined source/graph/catalogue scale, concurrent writers
and cross-project permissions at this catalogue size remain unqualified.
Every measured sample, API-journal hash and the known finding are recorded in
[`evidence/large-repository-e11-2026-11-06.json`](evidence/large-repository-e11-2026-11-06.json).

### E12: memory catalogue borrowing

`memory::list_of` previously cloned every decoded memory record so its lookup
could borrow unchanged rows while the response rows received annotations. E12
borrows the original catalogue, collects dependency reasons, symbol-link
summaries and visibility, then releases those borrows before moving the rows
into the response. Dependency resolution and transitive authorization still
see the complete unchanged catalogue. Ordering, retired-row behavior, decoding
errors and source/symbol freshness checks are preserved. There is no new
cache, API, schema, limit or selection policy.

The candidate changes only `crates/graphicle/src/memory.rs` relative to retained
E9. Its 99 distinct focused tests cover memory, permissions, revocation,
workspace scope and passage-index behavior. Formatting, Python syntax and
`git diff --check` pass. The release build completed successfully; all recorded
runtime source hashes still match the build inputs.

`bench/compare_memory_catalogue.py` compares saved E9 and E12 binaries on the
completed E11 fixtures. Each arm receives an isolated store and prepares its
own graph cache through `pack`. Stores are parked by rename so both arms use
the same request path. Each of three probes and four workloads receives one
excluded warmup plus five measured calls per arm, alternating execution order.
All 144 calls on 80 notes and all 144 calls on 25,000 notes return identical
complete packets. Exact citations, note bodies/revisions/dependencies, filters,
token budgets, freshness and omission diagnostics are checked. Complete
source, graph, access, memory, revision, dependency and stale-ledger rows remain
unchanged in the original fixture and both disposable stores.

The following table pools 15 measured calls per workload and arm across the
three fixed probes on 25,000 notes. Times are medians; memory is the maximum
observed process peak RSS. Per-probe medians and every raw sample are retained
in the evidence.

| Request | E9 seconds | E12 seconds | E9 MiB | E12 MiB |
| --- | ---: | ---: | ---: | ---: |
| Ordinary memory recall | 1.022 | 1.003 | 655 | 673 |
| File-filtered memory recall | 0.661 | 0.643 | 329 | 255 |
| Graph-assisted memory recall | 1.458 | 1.447 | 821 | 826 |
| Memory and graph disabled | 0.637 | 0.616 | 329 | 244 |

Retain for the targeted memory benefit: filtered and disabled requests reduce
peak RSS by 22.5% and 25.9%, with lower memory in all 15 paired samples of each
workload. Ordinary recall increases peak RSS by 2.8%, and graph-assisted recall
by 0.6%; this is not a universal peak-memory improvement. Pooled median latency
falls 0.8–3.4%, too little to claim a substantial general speedup. The 80-note
control shows no substantial performance change.

Every measurement starts a fresh CLI and measurement process with a warm OS
cache. No builds, tests or other benchmark workloads were deliberately run
alongside the timed calls. Host-wide process listing was unavailable, so
unrelated host load was not verified. This remains an isolated synthetic
catalogue over 40 tiny source files, not combined source/graph/memory scale or
independent retrieval-quality qualification. Full runtime and quality suites
were not rerun for E12 after its focused checks and exact-response comparisons.

The oldest 5,000 notes remain omitted by ordinary and graph-assisted recall,
and E11-F1 remains reproducible in the identical restored freshness report.
The optimization changes neither finding. Build identity, source hashes,
focused-test logs, comparisons and limitations are recorded in
[`evidence/large-repository-e12-2026-11-06.json`](evidence/large-repository-e12-2026-11-06.json).

### E13: larger parser inputs first (rejected)

A diagnostic run on a disposable copy of the qualified Babel corpus measures
15.365 seconds in per-file parsing, out of 26.366 seconds of extraction.
The 51 CommonJS files contribute 15.780 seconds of summed worker time. This
suggests testing the scheduling order; it does not establish that a different
order will improve throughput. Identity annotation takes another 7.080 seconds
and remains a separate possible experiment.

The candidate sorts the existing parallel work queue by descending source
size. It keeps the worker count, minimum parallel batch size, complete input
set, parser/cache policy and resolution scope. Each record and deferred cache
update still carries its original input position, and the report keeps input
order. A failed metadata lookup leaves the file in the queue for normal error
handling. No cache, API, schema or configuration is added.

All 22 focused extraction/incremental tests pass, including a new comparison
of complete serial and parallel output on uneven files, cache hits and missing
inputs. The modified lines introduce no formatting differences; the file has
the same five pre-existing formatting hunks as its control. The new
`bench/compare_babel_edits.py` harness passes 11 complete graph comparisons and
a fresh-cache equality check with E12 against itself on the 123-file smoke
corpus. That self-check ran alongside compilation and supplies correctness
evidence only.

The candidate release build also passes, and the candidate smoke comparison
passes all 11 complete graph checks, including a fresh rebuild. The full Babel
comparison completes 20 calls and all 19 complete graph checks, including
unchanged refreshes, deletion and final fresh-cache equality. All admitted
source heads, unresolved calls and independently reviewed call edges match.

| Measured edit pair | E12 seconds | E13 seconds | End-to-end reduction |
| --- | ---: | ---: | ---: |
| 1 | 59.239 | 57.526 | 2.9% |
| 2 | 59.479 | 58.658 | 1.4% |
| 3 | 66.526 | 51.375 | 22.8% |
| 4 | 130.102 | 138.308 | -6.3% |
| 5 | 121.636 | 123.536 | -1.6% |

Reject this experiment: the median improvement within each pair is only 1.4%,
and two of five pairs regress. Extraction alone improves in three pairs and
regresses in two, with a median paired reduction of 5.2%. Maximum measured peak
RSS rises from 2,605 to 2,646 MiB (1.6%). Substantial drift also affects source
scanning and identity annotation, which the change does not alter. The separate
arm medians (66.526 versus 58.658 seconds) therefore overstate the consistency
of the end-to-end benefit. No competing workload was deliberately run; unrelated
host load could not be verified. These measurements do not establish a useful,
repeatable benefit, or prove that the heuristic is universally harmful.

The scheduling code and its experiment-specific test have been removed by
restoring the exact E12 source, verified against its saved hash. Keep the
comparison harness, candidate patch and results for reproducibility. E12 was
the retained build used for the first million-file qualification. The profile, every raw
sample, build/source hashes and restoration evidence are recorded in
[`evidence/large-repository-e13-2026-11-06.json`](evidence/large-repository-e13-2026-11-06.json).

### E14: one million files across repositories (failed qualification)

The confirmed acceptance target is one million initial files. The existing
workspace harness now accepts `--files-per-project`, mutually exclusive with
its existing weighted `--units` mode. Manifests and probes count toward the
specified size: ten projects of 100,000 files contain 999,979 generated code
files and 21 manifests/probes. No Graphicle admission limit changes.

Syntax and CLI validation pass, including invalid sizes and conflicting sizing
options. A complete 400-file smoke run passes all 73 calls and checks, with
40 initial files in every project. The retained E12 binary hash and all 408
recorded runtime/build source hashes match. E13's rejected scheduling change
is absent.

The full run stopped with a failed correctness check in
`/private/tmp/graphicle-million-workspace-full-20261106`, using three context
samples per workload. It checks every generated local call and source hash,
unchanged graph equality, cross-project episode bindings, repeated packets,
exact citations and token budgets, default fanout omissions, revoked access,
and changed/deleted exporters. Each CLI operation retains the existing
1,800-second timeout. No coverage is removed to make the workload fit.

First completed milestone: one Python repository with exactly 100,000 sources,
99,997 generated local calls, 299,995 graph nodes and 299,992 edges passes cold
and unchanged checks. Admission takes 188.95 seconds and 1,410 MiB peak RSS;
cold graph construction takes 436.42 seconds and 7,873 MiB; unchanged refresh
takes 25.10 seconds and 444 MiB. All source hashes and generated calls are
checked, and the unchanged graph matches the cold graph. These are single
observations on the first repository, not results for the full target.
The second, TypeScript repository also passes on 100,000 sources and 99,998
generated calls (300,001 nodes and 299,999 edges). Admission takes 194.86
seconds and 945 MiB; cold construction takes 387.17 seconds and 7,850 MiB;
unchanged refresh takes 28.71 seconds and 462 MiB. Two repositories and six
calls pass that milestone. Go admission then passes in 209.14 seconds and
972 MiB peak RSS, bringing the admitted total to 300,000 files.

**Finding E14-F1:** the Go cold build returns successfully after 547.38 seconds
and 8,540 MiB peak RSS, but its graph contains no call edges. All 100,000 call
sites are explicitly unresolved with `go_analysis_timeout`, including all
99,998 generated local calls. The helper in `extractors/go_types.rs` limits
compiler analysis to 60 seconds; that limit was reached. The harness stops at
its first missing-call assertion. Seven calls are verified, the eighth CLI call
fails correctness verification, and no completed `results.json` is produced.
The remaining seven repositories and workspace checks never run. Package
discovery and compiler analysis need profiling before selecting a fix; raising
a timeout or accepting missing calls would not demonstrate a speed improvement.

**E14 does not qualify one million files.** The E15 follow-up below completes
all 73 calls and checks, beyond source admission and graph construction. This synthetic
equal-size workload will not by itself qualify uneven sizes, combined reviewed
memory scale, concurrent operation, warm-server latency or independent general
retrieval quality. Harness identity, smoke results and run details are recorded
in [`evidence/large-repository-e14-2026-11-06.json`](evidence/large-repository-e14-2026-11-06.json).

### E15: bounded large Go analysis (retained)

E14's Go failure was a time-budget failure, not missing source admission. A
temporary diagnostic helper on the exact materialized snapshot completes in
115.92 seconds and returns all 99,998 generated local calls correctly. Package
discovery takes 92.87 seconds, parsing 21.09 seconds, and type checking 0.46
seconds. Those diagnostics alone do not qualify the public CLI. E12 retains the
original one-minute analysis allowance as the saved control.

Two discovery-only prototypes preserve all 10,001 packages and 99,999 Go source
files: four workers with batches of 512 package patterns take 74.89 seconds;
requesting only the complete metadata schema through native `go list -json=`
takes 70.69 seconds. Neither fits even discovery into one minute. No batching,
new concurrency, field selection or cache is added to runtime. These single
diagnostics do not establish a general performance verdict.

E15 changes only graph compiler analysis from 60 to 300 seconds. Existing
request deadlines still shorten that allowance through `process::capture_bounded`.
Helper compilation remains bounded at 60 seconds, stdout/stderr at 64 MiB each,
and qualification CLI operations at 1,800 seconds. Background projections
already renew their lease and recheck authorization and source heads before
publishing. This is a completeness fix, not a speed optimization: complete
analysis may take longer than the previous incomplete result.

Three subprocess tests pass for deadlines, output bounds and timed-out child
cleanup. Five Go regressions pass for offline/no-home operation, receiver/build
identity, tests and standard imports. The release build and all 408 recorded
runtime/build source hashes verify. Its exact-size smoke passes 73 calls on 400
initial files. The changed source introduces no new formatting drift; its nine
pre-existing formatting hunks remain.

The fresh million-file rerun completes all 73 calls and checks, preserving the
failed E14 evidence. All three 100,000-file Go cold graphs verify every source
hash and 99,998 generated local calls each; all unchanged graphs match. Their
cold calls take 793.2, 687.0 and 744.9 seconds. This resolves E14-F1 on the tested
workload without dropping calls or raising its CLI timeout. Full source/call
coverage, workspace bindings, repeated packets, citations, budgets, default
fanout omissions, revoked access, edits and deletions all pass. Retain the
correction after the final runtime and quality gates also pass.

**E15-F1 — context remains bounded:** every main workspace context sample reports
600,000 omitted documents out of the one-million-document inventory. The
property-graph arm degrades; the code-graph arm still returns the reviewed answer
sources. The measured envelope and overlapping omission counters are explained
above. Passing the harness qualifies these explicit bounds and checks; it does
not qualify complete context coverage or general production readiness.

The same E15 sources pass 1,341 runtime tests (8 ignored), and its release
binary passes the 536-case quality gate without regressions or integrity errors.
Build identity, diagnostics, every scale sample, compact source-head hashes,
raw-result hash, coverage findings and final-check results are recorded in
[`evidence/large-repository-e15-2026-11-06.json`](evidence/large-repository-e15-2026-11-06.json).

### E16: indexed recall beyond query chunking bounds (retained)

E15-F1 traces to `indexed_query::packet`: graph-enabled and filtered requests
discard the usable passage index when the corpus crosses the full path's
50,000-document or 40,000-passage bounds. The fallback then selects a bounded
subset, despite the index already holding the complete retained file corpus.

E16 removes that fallback for repository-file retrieval. Filters still narrow
the indexed corpus and its BM25 statistics; reviewed-memory graph seeds use
the retained index too. Property-graph node/edge/traversal bounds, the code-graph
arm, authorization, source hashes and output budgets are unchanged. Connector
rows are still materialized for fusion, so a mixed connector/file catalogue
that reaches the source-candidate bound keeps the bounded full path and its
explicit omission report. Missing or ineligible indexes also keep their
existing fallbacks. No limit is raised and no requested graph arm is disabled.

The saved E15 executable matches all 408 baseline build/runtime source hashes
at starting HEAD `9a73af0`. Only `crates/graphicle/src/indexed_query.rs` changes
among those sources. The candidate release and source hashes are checked again
after measurement. The original qualification fixture is preserved; its final
state contains **999,999 files** after E15's deliberate exporter deletion.

[`bench/compare_indexed_coverage.py`](../bench/compare_indexed_coverage.py) backs
up that store separately for each arm, prepares all ten projects' build-matched
graph/query caches through `pack`, and parks stores at one shared path. It runs
one authored TypeScript invoice question in both workspace cross-repository
modes, explicitly searching ten projects. Each mode/arm has one excluded warmup
and three measured fresh-CLI calls, with arm and workload order alternated.
There are 16 context calls and 20 cache-preparation calls. Preparation is outside
timing; no competing build, test or fixture preparation is deliberately run.
OS caches are warm; unrelated host load is not controlled.

| Workspace context mode | E15 median | E16 median | Latency reduction | E15 maximum RSS | E16 maximum RSS | RSS reduction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cross-repository off | 31.60 s | 20.93 s | 33.8% | 19.41 GiB | 18.27 GiB | 5.9% |
| Cross-repository on | 37.13 s | 23.63 s | 36.4% | 19.64 GiB | 18.82 GiB | 4.2% |

Both modes report **599,999 → 0 omitted documents** and **400,000 → 999,998
searchable source passages**. One file is empty. The control's 499,999
document-limit omissions overlap its passage omissions and are not added.
The candidate keeps `complete_evidence_guaranteed=false`: corpus participation
does not guarantee recall or that all relevant evidence fits the answer budget.
The bounded property graph still reports degradation; the code-graph arm runs.

All 16 context calls find the labelled invoice source; cross-repository calls
also find its library implementation. Citation byte ranges and hashes match
admitted sources, deleted evidence is absent, token budgets pass, and complete
responses repeat exactly within each arm. Source/graph row hashes match before
and after in both copies and the original store. Prepared graph archive hashes
and executable hashes also remain unchanged. Packet equality across builds is
not the oracle because the searched corpus intentionally changes.

The 70 distinct targeted tests pass (55 integration, 15 query unit tests),
including existing small-corpus full-packet parity, filters, claims, permissions,
workspace scope and graph switches. New regressions cover:

- 50,001 indexed files: `retrying` retrieves a source containing `retry` that
  bounded raw-word preselection omits, while graph degradation stays explicit.
- A filtered 40,001-file corpus: the active property graph uses the indexed seed
  and reaches another dependency through reviewed memory.
- Exact citation bytes, a 1,000-token answer budget, lazy body loads, a missing
  index entry, corrupt-source rejection and deleted-source/memory exclusion.
- 40,001 unindexed connector documents: omission reporting remains accurate;
  filtering them out restores indexed retrieval.

An initial timing attempt copied E15-specific graph caches without preparing
the candidate's caches. Context reads do not repair these archives, so the
candidate repeatedly reconstructed graphs from SQLite. That run was stopped
and excluded from performance claims. The final comparison above prepares each
build through the existing `pack` path, as earlier cache comparisons do.

Retain E16 for the demonstrated coverage and resource improvements. Its paired
control is measured here; the earlier E15 32–44-second/15.8-GiB envelope is a
different run and is not used to compute these reductions. The full E15 runtime
suite and 536-case quality gate were not rerun for E16. This replay does not
requalify graph construction or updates, independent retrieval quality, large
connector catalogues, combined large source/memory volume, warm-server latency
or concurrency. The production checklist stays open.

The next focused optimization identified by E16 is full source-graph loading when the document
count already guarantees property-graph degradation. E16 phase logs spend
6.6–7.7 seconds there across ten projects, and peak context RSS remains high.
Saved file relationships and the code-graph arm must keep their current
behavior. Graph-update costs and memory-restoration freshness are separate
remaining items.

E17 below implements that source-graph optimization; the current next action is
recorded in the production checklist.

All samples, build identities, checks, phase totals, limitations and the excluded
run are recorded in
[`evidence/large-repository-e16-2026-11-06.json`](evidence/large-repository-e16-2026-11-06.json).

### E17: avoid unused source-graph loading (retained)

E16 retains indexed retrieval at large scale, but `indexed_query::packet`
still loads each project's full source graph whenever property-graph retrieval
is requested. Above 50,000 authorized documents, the property-graph builder
already returns its degradation report before reading any source edges.

E17 shares that document-budget predicate with the property-graph builder and
defers source-graph selection until after filters and indexed-path eligibility
checks. When the retained document count exceeds the budget and the current
episode's saved file relationships are valid, it uses the existing lightweight
episode view. The static lift keeps those relationships, and the code-graph
arm keeps its independent query plan. The property-graph builder still emits
the same degradation report. No budget or requested feature changes.

The filtered count matters: a large repository narrowed to exactly 50,000
documents can still use the property graph. Without valid saved relationships,
the source graph is still loaded. Unrecorded episodes retain the legacy path.
Only `graph.rs` and `indexed_query.rs` change among E16's 408 recorded
build/runtime source files. The staged E16 work present at task start is
preserved; E17 is a separate working-tree change.

**Correctness:** 79 distinct targeted tests pass (64 integration, 15 query unit
tests). The new 50,001-document regression checks that a healthy query-file and
file-relationship cache leave the resident graph unopened, while a filter to
exactly 50,000 documents opens it and runs the property graph. Missing,
truncated and corrupt saved relationships each fall back to the full graph
and return the same complete packet. Existing tests cover source and memory
filters, legacy graphs, graph switches, byte-exact citations, permission
revocation, query-cache recovery, historical/stale sources and workspace scope.
The earlier focused run and nested subprocess test are not double-counted.

**Comparison:** the same E15 fixture ends at 999,999 files after its deliberate
deletion. E16 and E17 each receive an isolated SQLite backup at the same parked
path. All ten build-matched graph/query caches are prepared through `pack`
outside timing. The existing harness adds `--require-identical`, requiring
complete packets to match across builds as well as repeat within each arm.
One authored TypeScript invoice question runs with cross-repository retrieval
off/on, one excluded warmup and three measured samples per mode/arm. There are
16 context calls and 20 setup calls; builds and tests finish before measurement.

| Workspace context mode | Paired E16 median | E17 median | Latency reduction | E16 maximum RSS | E17 maximum RSS | RSS reduction |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cross-repository off | 29.05 s | 14.64 s | 49.6% | 18.55 GiB | 3.70 GiB | 80.0% |
| Cross-repository on | 31.09 s | 16.77 s | 46.1% | 18.31 GiB | 13.18 GiB | 28.0% |

All 16 context packets match across builds, with 160 citation checks, the same
labelled sources, unchanged output budgets and zero corpus omissions. Both
builds search 999,998 source passages; one file is empty. Source and graph row
hashes remain identical in both copies and the original fixture, as do prepared
graph archive and executable hashes. All candidate source hashes verify after
measurement.

The measured candidate performs **zero `snapshot_graph` loads**, down from ten
per control call. With cross-repository retrieval off, saved resident-graph
reads also fall from ten to zero. With it on, graph archive reads fall from
thirteen to six: cross-repository endpoint lookup still loads resident graphs
through `workspace::Graphs`. That remaining path explains why the cross-repository
memory reduction is smaller and identified the next focused investigation.
E18 below removes these reads for healthy saved query files. Remaining
indexed-catalogue processing also contributes substantial query time.

Retain E17 for its unchanged responses and measured resource savings. These are
fresh CLI processes with warm OS caches and prepared build-specific caches;
unrelated host load is not controlled. Absolute E16 timings differ from the
previous experiment, so do not combine the two experiments' percentages. The
full E15 runtime suite and 536-case quality gate were not rerun. Real corpus
size, independent quality, large reviewed-memory volume, concurrency, server
latency and production budgets remain unqualified. Property-graph degradation,
connector bounds, graph-update costs and memory-restoration freshness remain
separate open items.

Samples, build identities, complete-packet checks, phase counts and outstanding
work are recorded in
[`evidence/large-repository-e17-2026-11-06.json`](evidence/large-repository-e17-2026-11-06.json).


### E18: saved cross-repository endpoint lookup (retained)

E17's cross-repository step still opens resident graphs to find endpoint paths,
locate citation ranges and select the first symbol that uses an imported name.
E18 lets `workspace::Graphs` read the existing saved query file first. Node-id
lookup reads only endpoint records; incoming module endpoints use a per-file
node index instead of scanning the whole graph.

The query-file format advances from v3 to v4. Each existing start-line record
now also stores its non-file nodes with start lines, in graph order. This
includes unnamed and document nodes, matching `code_graph_arm::symbols_in`;
the narrower named-code-symbol table would change behavior. A shared predicate
owns membership, and the identifier route and endpoint lookup share the saved
node-location helper. Citation construction still uses `locate_record`.
Stable line/byte sorting keeps graph order for equal ranges.

Missing, incompatible or corrupt caches fall back to the resident graph. A
failure discovered after locating a module retries the entire endpoint lookup,
so partial symbol results cannot silently replace the correct citation with a
module header. The reader validates counts, node bounds, ascending node order,
file membership and existing page checksums. Damaged caches are removed by the
existing cache-recovery path and rebuilt on the next preparation/sync. Old
caches need rebuilding to gain this optimization; source data needs no migration.
The extra file-to-node index adds derived-cache bytes and preparation work.

Workspace authorization, scope filters, current source visibility, historical
source-hash checks, one-hop limits, lifting and annotation order are unchanged.
Only `code_graph_arm.rs`, `identifier_route.rs`, `query_file.rs` and
`workspace.rs` change among E17's 408 recorded build/runtime source files.
Prior staged E16 work and unstaged E17 changes are preserved.

**Correctness:** 108 distinct targeted tests pass: 43 unit tests and 65
integration tests. New tests compare saved/resident endpoint ranges, equal-line
ordering, unnamed nodes, UTF-8 citations, absent/unauthorized sources, and
module-header fallback. Cache regressions cover missing, truncated,
incompatible, checksum-damaged and semantically invalid node indexes, including
corruption reached after node location succeeds. Healthy endpoint lookup leaves
the resident registry unopened; damaged lookup retries the whole operation.
Three workspace questions produce identical complete packets through saved
indexes and cache fallbacks, covering both directions and already-ranked
endpoints. Existing permission, freshness, HTTP linking, graph-switch,
query-cache recovery and scale-bound tests run with prepared saved caches.
The nested subprocess test is not double-counted.

**Comparison:** the same E15 fixture retains 999,999 files across ten
repositories after its deliberate deletion. Each binary receives an isolated
SQLite backup at the same parked path, with ten build-matched caches prepared
through `pack` before timing. The existing harness alternates arms and workload
order. One authored invoice question runs with cross-repository retrieval
off/on, one excluded warmup and three measured samples per mode/arm: 16 context
calls and 20 preparation calls. No builds, tests or fixture preparation overlap
the timed queries.

| Workspace context mode | Paired E17 median | E18 median | Latency change | E17 maximum RSS | E18 maximum RSS | RSS change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Cross-repository off | 8.98 s | 8.86 s | 1.4% lower | 3.69 GiB | 3.70 GiB | 0.4% higher |
| Cross-repository on | 12.58 s | 10.10 s | 19.7% lower | 13.14 GiB | 3.80 GiB | 71.1% lower |

All 16 complete packets match across builds and within each arm, with 160
byte-exact citation checks, identical labelled sources and output budgets,
999,998 searched source passages (one file is empty) and zero corpus omissions.
Source/graph row hashes match in both copies and the original fixture;
prepared graph archive, executable and harness hashes are unchanged. All 408
candidate build/runtime source hashes and recorded integration-test hashes
verify after measurement. Prior staging is unchanged.

Every measured E17 cross-repository call opens six resident graphs, taking
1.97–2.61 seconds in `graph_saved_read`. E18 opens **zero**. Its 24 endpoint
operations per cross-repository query use saved query-file records, with no
reported corruption. `snapshot_graph` remains zero in both modes and builds.
The endpoint diagnostic's `ms=0` is a note, not a latency measurement. Prepared
query files total 1,889,637,972 bytes versus 1,882,789,777 bytes in E17: an increase
of 6.53 MiB (0.36%) across ten projects.

Retain E18 for unchanged responses, eliminated resident endpoint loads and
measured cross-repository savings. The off mode is a control with no endpoint
work; its small median/RSS changes do not establish an improvement. The measured
samples vary: E17 cross-repository latency spans 12.53–18.22 seconds and E18
9.56–12.05 seconds; E18's off-mode samples span 8.74–15.04 seconds. These are
fresh CLI processes with warm OS caches on a host whose unrelated load is not
controlled. Do not combine percentages or absolute timings with earlier runs.

The remaining ten `context_indexed` phases total 7.9–9.8 seconds in measured
E18 cross-repository calls. This identified catalogue materialization, state
construction and retained workspace metadata as the next profiling target.
E19 below provides that breakdown and selects the next change.
The full E15 runtime suite and 536-case quality gate were not rerun. Real corpus
size, independent quality, reviewed-memory volume, warm-server latency,
concurrency and production budgets remain unqualified. Property-graph degradation,
connector bounds, graph-update costs and memory-restoration freshness remain
open; the readiness decision stays controlled pilot.

Samples, build identities, tests, phases and outstanding work are recorded in
[`evidence/large-repository-e18-2026-11-06.json`](evidence/large-repository-e18-2026-11-06.json).


### E19: indexed catalogue profile (complete)

E19 adds opt-in diagnostics to the retained E18 implementation; it changes no
retrieval policy, query limits, source authorization or result format. Existing
`GRAPHICLE_PHASE_TIMINGS` output on stderr now separates catalogue preparation,
search and finalization. Nested phases identify database-row materialization,
full document construction, lookup maps, reviewed-memory visibility, state
hashing, episode lookup, property-graph snapshot cloning, selected rows and
assembly snapshot cloning. Before/after-return checkpoints include the work
spent releasing local values, which earlier timers left unattributed.

Workspace diagnostics record retained project/document counts and process RSS
after each project, the cross-repository step, fusion and return. These are
process-residency observations; mappings, caches and allocator retention prevent
interpreting them as exact live-heap ownership. Counts and timings are logged
only when diagnostics are enabled; response payloads remain unchanged.

[`profile_indexed_catalogue.py`](../bench/profile_indexed_catalogue.py) consumes
a completed `compare_indexed_coverage.py --require-identical` replay. It rejects
unexpected project/seed-search counts, excludes warmups, checks phase sums,
reports nonoverlapping parent partitions and keeps nested details separate.
Derived cleanup remainders include timing/logging overhead, rounding and other
unmeasured work as well as destruction. No diagnostic `ms=0` marker is treated
as a measured zero-cost operation.

**Validation:** 56 distinct targeted tests pass (23 unit and 33 integration).
They cover query semantics, indexed filters, source integrity, memory visibility,
scale bounds, workspace permissions, stale endpoints and saved/resident packet
parity. An earlier focused run and the nested subprocess are not double-counted.
The full E15 runtime suite and 536-case quality gate have not been rerun.
**Method and parity:** the retained 999,999-file E15 fixture is copied into
isolated E18/control and instrumented stores. Each receives ten build-matched
caches via `pack` outside timing. The same invoice question runs with
cross-repository retrieval off/on, one excluded warmup and three measured
samples per mode/arm. All 16 complete packets match, with 160 byte-exact
citation checks, unchanged budgets and labelled sources, and zero omissions.
Both copies and the original retain identical source/graph row hashes, and
prepared graph archives and binaries stay unchanged. All 408 candidate source
hashes verify after measurement; prior staged work is preserved.

Two additional candidate calls with `GRAPHICLE_PHASE_TIMINGS=0` produce
byte-identical stdout to their instrumented counterparts, with no phase output.
The candidate's source/graph hashes and graph archives are checked again after
those calls. Builds, tests and cache preparation finish before timed queries.

**Measured costs:** the following nonoverlapping buckets cover indexed work
across all ten projects in a cross-repository query. Times are arithmetic means
of three measured query totals; percentages divide aggregate bucket time by
aggregate `context_indexed` time. Nested details are excluded from this sum.
The mean indexed total is **8.147 seconds** (median 7.897 seconds).

| Indexed work | Mean per query | Share of indexed time |
| --- | ---: | ---: |
| Repository metadata construction | 2.490 s | 30.6% |
| State fingerprint construction and hashing | 1.176 s | 14.4% |
| Source-head loading and current-episode lookup | 0.962 s | 11.8% |
| Code-graph plan, source loads and anchoring | 0.869 s | 10.7% |
| Property and assembly catalogue clones | 0.541 s | 6.6% |
| Response assembly | 0.415 s | 5.1% |
| Passage-index opening | 0.400 s | 4.9% |
| Remaining indexed work, including cleanup | 1.295 s | 15.9% |

Metadata construction plus state hashing account for **45.0%** of indexed
work. Inside metadata construction, SQL execution and intermediate JSON-row
materialization take 1.607 seconds on average; building the final document
values takes another 0.876 seconds. The row phase does not distinguish database
I/O from row decoding/allocation. Lexical and speaker scoring together average
only 0.020 seconds (0.24%); the current workload's main cost is outside scoring.
The off-mode profile agrees: metadata 30.9%, state hashing 14.3%.

The two snapshot clones process 1,999,998 document records per query in total,
derived from the catalogue counts and ten calls to each clone site. Their
construction averages 0.298/0.243 seconds for property/assembly snapshots.
The over-budget property-graph call itself reports below one millisecond per
project; the surrounding property block also includes clone destruction.
Search return cleanup averages 0.284 seconds, but includes other local values
and diagnostic overhead, so it cannot all be assigned to the assembly clone.

**Workspace retention:** every query retains 999,999 document records across
ten project snapshots until fusion finishes. In measured cross-repository
calls, process RSS rises from 936.8–940.5 MiB after the first project to
3,616.8–3,618.4 MiB after the tenth (about 0.92 → 3.53 GiB). After workspace
return it remains 3,415.7–3,417.2 MiB. This includes allocator retention and
mapped query caches; it is not an exact measurement of catalogue-owned bytes
or evidence of a memory leak. Project queries account for 87.8% of workspace
time, the cross-repository step 8.5%, return cleanup 3.5%, and fusion 0.1%.
The latter averages 10 ms, while endpoint work averages 790 ms with no resident
graph reads. Phase partitions pass for every measured candidate query.

**Action selected by E19 (completed in E20 below):** stream rows from the
existing SQLite query directly into final document records in
[`memory::repository_documents`](../crates/graphicle/src/memory.rs), eliminating
the temporary `Vec<Value>` of generic row objects. This is the largest measured
stage and has a local implementation boundary. Preserve the existing SQL joins,
text-only predicate, path order, metadata fields and types, both `content` modes,
source availability/error behavior, state fingerprints and authorization.
Re-run exact-packet and source-integrity checks and measure time/RSS before
retaining the change. **The 2.490-second stage is not an estimated saving:**
necessary SQLite access and final-record construction remain.

After that, investigate streaming state serialization with byte-identical
fingerprints, then unnecessary catalogue copies and retained workspace metadata.
Removing the property clone alone targets a smaller measured cost than the
metadata path. These are proposed optimizations, not work completed in E19.

The fresh E18/instrumented medians are 8.68/8.92 seconds with cross-repository
retrieval off and 10.43/9.48 seconds with it on. No speedup or isolated telemetry
overhead is inferred from those differences: timing, code generation, allocator
behavior and unrelated host load vary. This experiment retains diagnostics and
a reproducible profile, not a retrieval optimization. The fixture has tiny
synthetic files, one authored question and no reviewed memories; real-corpus
quality, combined source/memory workloads, warm-server latency, concurrency and
production budgets remain unqualified. The production checklist stays open.

Raw samples, phase partitions, RSS checkpoints, source/build hashes, tests and
pending work are recorded in
[`evidence/large-repository-e19-2026-11-06.json`](evidence/large-repository-e19-2026-11-06.json).


### E20: streamed repository document construction (retained)

`memory::repository_documents` now consumes its SQLite cursor directly into
final document records. It no longer collects generic JSON row objects before
building the catalogue. Decoded field values move into their final slots;
only values required in two response fields, such as path/title and SHA/version,
are duplicated. `store::json_value` owns the existing conversion rules for both
this path and generic row reads, including nulls, numbers, blobs and lossy UTF-8.

The SQL joins, text-only predicate and path order are unchanged, as are the
complete JSON fields, both content modes and current-source binary metadata.
Source availability/integrity checks, authorization, freshness, filters and
citation validation remain in their existing layers. The combined
`repository_document_stream` diagnostic replaces the old row/value phases;
`indexed_documents` remains the comparable outer timer. The profiling script
now accepts `--arm baseline` or `--arm candidate` for the same replay.

**Validation:** 59 distinct targeted tests pass (26 unit, 33 integration).
Three new regressions check complete document values, ordering, repository
scope, missing/non-text objects, current/stale parser metadata, both content
modes, lazy content loading, state parity, unusual SQLite values, invalid UTF-8,
SQL failures and tampered/unavailable sources. Existing indexed-query,
permissions, freshness, graph fallback and workspace tests pass. The nested
test subprocess is not double-counted. The full E15 runtime suite and 536-case
quality gate have not been rerun.

**Paired measurement:** isolated E19 and final E20 stores use the same retained
999,999-file, ten-repository fixture. Twenty build-matched cache preparations
finish before timing. One warmup per mode/arm is excluded; three measured
samples alternate build and workload order. No builds or tests run during the
measurements. All 16 complete responses match, with 160 exact citation-byte
checks, zero corpus omissions and unchanged budgets. Source/graph rows match
in both copies and the original; prepared archives and binaries are unchanged.
Two diagnostics-off calls also produce byte-identical stdout with no phase
logs. All 408 source hashes, build configuration and test hashes verify; prior
staged work is preserved.

The table uses mean metadata time across all ten projects, median end-to-end
query latency, and maximum observed peak RSS over each arm's three measured
samples. Positive reductions mean lower cost; these are paired observations,
not production guarantees.

| Measurement | E19 control | Final E20 | Reduction |
| --- | ---: | ---: | ---: |
| Metadata construction, cross-repository off | 2.247 s | 1.847 s | 17.8% |
| Total query latency, cross-repository off | 8.80 s | 8.26 s | 6.1% |
| Peak RSS, cross-repository off | 3.53 GiB | 3.78 GiB | -7.0% |
| Metadata construction, cross-repository on | 2.518 s | 1.852 s | 26.5% |
| Total query latency, cross-repository on | 9.75 s | 9.06 s | 7.0% |
| Peak RSS, cross-repository on | 3.64 GiB | 3.84 GiB | -5.3% |

**Tradeoff:** mean metadata-construction time falls 17.8% with cross-repository
retrieval off and 26.5% with it on. Median total latency falls 6.1%/7.0%.
Maximum RSS rises 7.0%/5.3%, so removing intermediate objects has not
demonstrated lower process memory. All final catalogue records remain live
through fusion; allocator retention and mappings also affect RSS. This retains
a measured latency improvement and leaves memory reduction open.

**Initial replay retained in the evidence:** simply streaming rows through the
JSON macro still copied decoded values. Its local-query median was 1.4% slower,
its cross-repository median 2.1% faster, and maximum RSS rose 9.5%/5.4%.
That mixed result prompted the owned-field construction above. The final table
uses a new E19 control from the final replay; no samples from separate builds
or runs are pooled, and the initial run is not presented as a success.

**Remaining work:** every final document record is still retained through
workspace fusion. This change removes intermediate allocations; it does not
make the final catalogue bounded or qualify concurrent production use. Next,
stream state fingerprint serialization with byte-identical digests, then assess
avoidable property/assembly catalogue clones and retained workspace metadata.
The property-graph budget, connector bounds, graph-update costs and
source-restoration freshness work remain open.

The fixture contains tiny synthetic files, one authored question and no
reviewed memories. Three samples, warm OS caches, fresh CLI processes and
uncontrolled host load limit performance conclusions. RSS includes mappings,
allocator retention and caches, not just live document allocations. Real-corpus
quality, combined source/memory volume, warm-server latency, concurrency and
production budgets still need qualification; readiness remains controlled pilot.

Build identities, both replay records, raw samples, phase partitions, tests and
remaining work are recorded in
[`evidence/large-repository-e20-2026-11-06.json`](evidence/large-repository-e20-2026-11-06.json).
