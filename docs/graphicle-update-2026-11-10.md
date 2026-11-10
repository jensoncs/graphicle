# Graphicle feature and performance update

**As of:** 10 November 2026

**Code baseline:** `context-improvement-plan` at `88e315b9065841219ef4c538158733ae6dad4c72`

**Version:** Core, graph engine, memory library and Python/TypeScript SDKs are aligned at 0.2.0; the changelog marks 0.2.0 as unreleased.

**Companion document:** [Graphicle high level design](high-level-design.md).

Graphicle now combines code navigation, reviewed project memory and cited context with immutable change reviews, configured service dependencies, reusable compiler analysis and persistent review serving. The current branch includes the upstream 0.1.13 capabilities and the additional review, retrieval and scale work described below.

The strongest recent results are a **96.4% reduction in export-index rebuild/relink time**, **8.2–15.7% lower warm pinned-review latency** on two local repository archives, and successful bounded graph qualification across **one million synthetic files in ten repositories**. Later retrieval changes remove the indexed-file cutoff and reduce context costs while preserving the compared responses. These measurements have different workloads and baselines; their improvements are not cumulative.

Sentinel production use is recorded, with a running Graphicle 0.2.0 artifact observed on 6 November. That artifact predates the latest readiness and interpretation changes. The evidence supports controlled use within measured limits; general production qualification for very large repositories remains open.

## Integrated features

“Integrated” below means present in this code baseline. It does not imply that every feature is enabled in Sentinel or included in the last observed production binary.

### Code graph and context retrieval

- **Local code graph:** extract files, declarations, calls and imports; search symbols, inspect callers/callees, trace dependency paths, assess affected code and export an interactive graph.
- **Cited context:** retrieve code, documents, sessions and approved memories with file/line citations and a caller-selected token budget. Ordinary context retrieval and navigation require no model call.
- **Optional semantic retrieval:** configured CPU ONNX embeddings and cross-encoder reranking are available through the `local-inference` build feature. They require validated local assets and are disabled by default; their quality and CPU costs depend on the workload.
- **Workspace retrieval:** search across authorized repositories, with cross-repository imports and HTTP relationships, source provenance and explicit omissions.
- **Structured declaration search:** filter Python, JavaScript/TypeScript, Go and Java declarations by supported parameter names/types, decorators or annotations. Results report field coverage and exact source locations.
- **Static flow navigation:** return ordered invocation paths, including configured HTTP, gRPC and event boundaries. Go function-value analysis follows bounded assignments, arguments and literal storage, retaining uncertain targets as `may_call` candidates.
- **Long-context packs:** assemble architecture, focus code, dependencies, tests, documentation and a file manifest into a bounded pack, with budgets up to two million tokens. `ask --pack` and `plan` can pass these to a configured model.
- **Refresh and scale improvements:** incremental extraction, Git hooks, safe early stopping for unchanged graph updates, indexed file retrieval, compressed dependency representation, reusable navigation snapshots, saved endpoint lookup and streamed document construction.

Sources: [product overview](../README.md), [structured symbols and flows](symbol-search-and-flows.md), [packs](pack.md), [configuration](configuration.md), [scale work](large-repository-readiness.md).

### Project memory and documentation

- **Unified project memory:** search Markdown, PDF, Office and HTML documents, reviewed decisions/procedures and captured agent sessions alongside code. Agent-proposed memories enter an approval queue.
- **Connected sources:** import authorized content from GitHub, Slack, Notion, Google Drive, Jira and Confluence, plus JSONL snapshots/deltas. Provider-specific coverage and explicit sharing rules apply; Core does not provide provider-permission synchronization.
- **Code-linked freshness:** attach memories to declaration revisions, identify stale notes after relevant code changes and link documentation mentions to code. Complete declaration spans now protect revision checks against truncated symbol slices.
- **More selective memory retrieval:** use qualified symbol/dependency scope, question intent, rationale anchors, named configuration fields and language-specific extraction to reduce unrelated notes. Ambiguous cases retain broader fallback; these filters have configuration-specific limits.
- **Evidence-scoped interpretation:** the November 10 change asks proposers to preserve branch/SQL conditions, attribute comments and avoid asserting effects inside unseen helpers. Citation validation and approval remain required; they do not prove that a claim logically follows from its evidence.
- **Project wiki:** build cited articles and an index, update affected articles, and check stale documentation, memories and citations. Model use is opt-in and cost-capped; correction proposals remain reviewable. The 0.1.13 wiki release was validated with stand-in models, without a live-model release run.
- **Session continuity and browser app:** capture supported agent sessions, carry reviewed context into later sessions and inspect sources, graph and memory approvals in the app.

Sources: [product overview](../README.md), [retrieval quality](quality.md), [interpretation guidance](../bench/memory/claim-guidance.md), [wiki](wiki.md).

### Pinned change review and reuse

- **Immutable base/head review:** CLI, HTTP, MCP and both SDKs accept exact snapshot sets and a build profile. Go analysis uses compiler facts and reports callers, dependencies, changed source and exact provenance.
- **Basic impact for other inputs:** Terraform, Helm, Bash, Python and Rust have bounded file/module/chart impact. This tier reports incomplete semantic coverage and cannot certify no impact or authorize skipping review.
- **Separate analysis and delivery coverage:** risk schema v2 preserves the full-analysis impact verdict when the response budget omits context. `analysis_coverage` separately assesses changed Go components and resolved dependency APIs; it can be complete while global impact remains incomplete.
- **More useful review packets:** reserve current-head code, bounded diff hunks and delta controls before optional context; report canonical ordering, useful-head coverage, ranking decisions and omissions. Packing selection no longer depends on variable-length store identities.
- **Avoid repeated primary diff text:** optional `provided_diff` validates exact hashes and side-specific ranges before excluding fully covered entries/hunks from supplemental selection. Partial overlaps remain eligible.
- **Incremental review packets:** subsequent pushes return added, updated, unchanged and invalidated entries. Optional text references reuse identical bytes; Python and TypeScript materializers verify hashes and apply deltas without a second full request.
- **Conditional reuse of completed reviews:** component fingerprints cover sources, dependencies, build inputs, analyzer and toolchain. Reuse requires complete proofs, consumer-attested completion and an unchanged review contract. Missing evidence falls back to full review.
- **Compiler and dependency caches:** reuse exact compiler inputs and package exports, including eligible incomplete analyses without suppressing their diagnostics. Separate configuration pools retain up to 64 MiB each and 128 MiB per repository; cache diagnostics explain misses and refusals.
- **Go correctness and offline operation:** batch discovery by module, respect supplied vendor trees, isolate pinned-review caches, preserve binary compiler inputs, handle empty/build-excluded inventories, and resolve supported interface initializers while retaining aliasing and mutation uncertainty.
- **Execution observations:** accept revision-bound coverage observations with source checks, separately from static dependency evidence.

Sources: [pinned reviews](pinned-review.md), [transport and measurements](pilot-protocol.md), [cache capacity evidence](evidence/package-cache-capacity-20261105.json), [changelog](../CHANGELOG.md).

### Service boundaries and configuration

- **Pinned service wiring:** `graphicle-services.json` joins local calls with HTTP, gRPC and event relationships across supplied repositories. Links preserve service identity, exact configuration/contract citations and snapshot provenance; dynamic or ambiguous destinations stay unresolved.
- **Environment-aware selection:** connect literal Kubernetes environment values to supported Go and Python reads and their consumers. Automatic discovery supports unambiguous single-application/single-workload repositories; explicit scopes support separately pinned deployment repositories. Dynamic reads and overrides prevent unsafe narrowing.
- **HTTP request extraction:** recognize literal Fetch/Axios methods, default GET behavior, immutable URL bindings and simple same-file request helpers. Dynamic options no longer select an arbitrary method.
- **Framework endpoints:** discover supported Next.js App/Pages routes, Spring mappings, Actix/Axum routes and Firebase HTTP/callable/event facts. Firebase deployment identity remains unresolved. Inferred route matches are distinct from explicitly configured service wiring.

Sources: [service boundaries and environment inputs](service-boundaries.md), [extraction improvements](evidence/extraction-improvements.md).

### Historical context and semantic tools

- **Business and Git context:** attach evidence-bearing domain candidates and exact-commit history assertions to review entries. The bounded history collector supports function patches, co-change relationships, churn/decision hotspots, pseudonymous ownership, candidate fix-to-cause links and explainable review priority. These remain candidates, not measured business criticality or runtime hotness.
- **Snapshot-checked secondary context:** optional SDK helpers preserve primary review text and add only validated repository-file excerpts. Wrong snapshots, invalid citations or retrieval failures return explicit status without reducing the primary review scope.
- **Go statement analysis:** inspect bounded intraprocedural control flow, reads/writes, reaching definitions and control gates, with exact spans and unsupported constructs reported. This does not provide general taint or whole-program effect analysis.
- **Architectural previews:** simulate dependency/node removals and module assignments on a copy of the authorized graph, reporting coupling and cycle changes without changing stored data.
- **Go local rename:** preview and apply function-local variable/parameter renames after binding checks, re-typechecking and source/configuration revalidation; replacement is atomic for the affected file.

Sources: [business context](business-context.md), [secondary context and semantic previews](secondary-context-and-semantic-previews.md).

### Integration and operation

- **Persistent review service:** retain the compiled Go helper across requests. `serve --no-worker` separates review serving from built-in background workflow processing; a separate worker can process queued work.
- **Unix sockets and deadlines:** private Unix socket transport, SDK support, cooperative review deadlines, subprocess cleanup, structured timeout errors and server recovery.
- **Efficient source admission:** compact status/commit receipts, paginated missing-object checks, authorized reuse of admitted sources, hash-checked text/binary uploads and faster snapshot materialization.
- **Head-context reuse:** query the already admitted review head without a second sync; secondary-context helpers reject a stale snapshot.
- **Shared snapshots in GCS:** publish immutable, expiring source/graph artifacts and query the exact artifact from another worker, checking source hashes, identity, executable compatibility and access. Local/mock validation exists; live GCS policy and the complete remote pilot remain separate acceptance work.
- **Agent and team access:** one MCP server, Python/TypeScript SDKs, setup for supported clients, workspace/project authorization, revocable access keys, shared HTTPS deployment, Docker Compose and systemd recipes.
- **Optional backends and maintenance:** Neo4j graph projection, Qdrant vector search and Redis inference dispatch integrate with the authoritative SQLite store. Backup, retention, offboarding and external-copy cleanup support operation; these adapters are not required for a local install.
- **Model-assisted developer workflows:** cited branch reviews and a GitHub Action; issue fixing with tests; browser reproduction and screen-to-code lookup through `see`, `pixel` and `fix`. These workflows use configured models and have their own cost controls.

Sources: [pinned review serving](pinned-review.md), [pilot protocol](pilot-protocol.md), [shared context](shared-context.md), [team server](team.md), [MCP](mcp.md), [review](review.md), [screen-to-code workflow](flow.md), [connectors and optional backends](../crates/graphicle/README.md), [local inference](../crates/graphicle/LOCAL_INFERENCE.md).

## Performance testing results

The following are recorded measurements from the linked evidence. Latency reductions compare each candidate only with its paired control. Peak resident memory (RSS), retained payload bytes and token counts measure different things and are kept separate.

### Local pinned reviews

The 4 November comparison used release builds on macOS ARM64 with 18 logical CPUs, two runs per arm in before/after/after/before order, older Nexus #963 and platform-rbac #638 archives, and an 8,000-token budget. “Cold” means empty Graphicle analysis/package caches; “warm” repeats identical pins with full native-analysis cache hits. OS caches were not flushed. Dependency collection, model calls and service startup were excluded.

| Operation | Nexus before → after | Platform RBAC before → after |
| --- | ---: | ---: |
| Cold review median | 32.03 → 30.42 s; **5.0% lower** | 31.33 → 29.99 s; **4.3% lower** |
| Warm review median | 2.40 → 2.02 s; **15.7% lower** | 2.13 → 1.96 s; **8.2% lower** |
| Warm review peak RSS, median | 603.2 → 512.0 MiB; **15.1% lower** | 613.9 → 528.8 MiB; **13.9% lower** |
| Sync plus cold review stages | 47.18 → 45.70 s; **3.1% lower** | 47.60 → 46.44 s; **2.4% lower** |

All 16 packets matched within each case after excluding only cache diagnostics and token counters. Sync itself did not improve: its median increased approximately 0.9–1.1%. The stage sums above are not complete Sentinel review latency. [Recorded results](evidence/local-pinned-review-performance-20261104.json).

### Persistent serving and concurrency

| Experiment | Measured result | Interpretation |
| --- | --- | --- |
| Process reuse, 4 November; two samples per arm/case | Fresh HTTP → warm HTTP: **19.14 → 13.45 s** for Nexus and **17.42 → 11.70 s** for platform-rbac; **29.7% and 32.8% lower** | New heads still required analysis. All 12 packets matched apart from cache diagnostics/token counts. Warm service RSS was higher. |
| Worker-isolated soak, 4–5 November | Initial sequential medians: **16.83 → 13.24 s** and **18.04 → 13.53 s** versus CLI; **21.3% and 25.0% lower** | 40 measured reviews and 20 CLI/server packet pairs; all pairs matched and stayed within budget. |
| Two concurrent requests in that soak | Server medians reached **25.08 s** for Nexus and **18.93 s** for platform-rbac; sampled server RSS reached **1,205.5 MiB** and **1,157.2 MiB**, respectively | Request latency and retained memory increased. Initial client concurrency recommendation remains **one**. |

These are finite local tests on two archives, excluding admission and collection. They do not establish production throughput or long-term memory stability. Sources: [process reuse](evidence/persistent-review-service-20261104.json), [worker isolation and soak](evidence/review-worker-isolation-soak-20261105.json), [longer retained-memory probes](evidence/retained-review-memory-20261105.json).

### Large repositories and workspaces

| Change and workload | Paired result | Correctness and scope |
| --- | --- | --- |
| Export-index rebuild/relink; ten repositories, 100,000 generated files plus manifests/probes | **100.89 → 3.67 s**, **96.4% lower** median over five samples per arm; essentially unchanged peak RSS | Complete indexes, links and compared packets matched. This improves rebuilding/relinking, not parsing or individual repository updates. |
| Indexed coverage fix; retained 999,999-file workspace | Reported omitted documents **599,999 → 0**; median **31.6 → 20.9 s** with cross-repository retrieval off, **37.1 → 23.6 s** with it on | Searches 999,998 passages; one file is empty. Output/ranking budgets and property-graph limits still apply. |
| Avoid unnecessary full graph loads; same retained fixture | Cross-repository off: **29.05 → 14.64 s**, RSS **18.55 → 3.70 GiB**. On: **31.09 → 16.77 s**, RSS **18.31 → 13.18 GiB** | Complete packets matched; **79 targeted tests** and 16 scale queries passed. |
| Saved cross-repository endpoint lookup | **12.58 → 10.10 s**, **19.7% lower**; RSS **13.14 → 3.80 GiB**, **71.1% lower** | Resident endpoint graph loads fell from six to zero. **108 targeted tests**, 16 matched packets and 160 exact citation checks passed. |
| Streamed document construction | Cross-repository off: **8.80 → 8.26 s**, **6.1% lower**. On: **9.75 → 9.06 s**, **7.0% lower** | **59 targeted tests** and 16 matched scale queries passed. Maximum RSS **increased 7.0% and 5.3%**, respectively. |

The million-file fixture initially held about 124.5 MB of tiny synthetic sources across ten repositories; a deliberate deletion left 999,999 files for later comparisons. Retrieval timings use one authored question, three measured samples per mode/arm, fresh CLI processes, warm OS caches and prepared build-specific caches on macOS ARM64. Reviewed-memory retrieval was disabled. Each row has a separately measured baseline; do not chain the percentages or compare different runs as a single speedup.

Sources: [export linking](evidence/large-repository-e8-2026-11-06.json), [indexed coverage](evidence/large-repository-e16-2026-11-06.json), [graph loading](evidence/large-repository-e17-2026-11-06.json), [endpoint lookup](evidence/large-repository-e18-2026-11-06.json), [streamed documents](evidence/large-repository-e20-2026-11-06.json).

Graph update costs remain material. The earlier 100,000-file Python experiment recorded a **494.9 s cold graph**, **28.5 s median unchanged refresh** and **308.7 s median single-file edit**, with up to **5.71 GiB** peak RSS. These are separate synthetic graph-operation measurements, not the optimized workspace retrieval timings above. [Scale assessment](large-repository-readiness.md).

### Component benchmarks

| Component | Recorded result | Measurement boundary |
| --- | --- | --- |
| Batched missing-object checks | **34.7–41.6% lower** median time at 5,000/20,000/50,000 files; 50,000-file SQL statement executions **9,800,000 → 196** | In-memory metadata microbenchmark; two samples per arm/size; excludes admission/upload/projection. |
| Snapshot materialization | **59.1–62.4% lower creation time** and **43.7–45.3% lower creation-plus-cleanup time** | Synthetic 1 KiB files; 5,000/20,000/50,000 files; two samples per arm/size. |
| Shared verified source payloads | At 20,000 files per side and 1% changed content: load **1,204 → 643 ms**, retained payload bytes **49.5% lower** | Exact retained allocations, not peak RSS. With 100% changed content there is no payload saving and loading is slower. |
| Cache refresh persistence | 16 MiB analysis refresh **837 → 15 ms**; 32 MiB package refresh **845 → 47 ms** | Synthetic persistence fixtures; 14 samples per arm. These are component savings, not end-to-end review reductions. |

All four used local macOS unoptimized test binaries. Sources: [pagination](evidence/sync-pagination-benchmark.json), [materialization](evidence/snapshot-materialization-benchmark.json), [source loading](evidence/review-source-io-benchmark.json), [cache refresh](evidence/review-cache-refresh-benchmark.json).

### Earlier released engine benchmarks

- **Unchanged updates, 0.1.12 → 0.1.13:** Kubernetes **164.8 → 15.7 s**, React **22.6 → 1.02 s**, Redis **3.40 → 0.25 s**, with the same outputs. One cloud machine, eight CPUs/eight workers; three baseline and five candidate runs. [Release record](../CHANGELOG.md#0113---2026-11-05).
- **Graphicle 0.1.11 versus Graphify 0.9.100:** Graphicle took less time in **34/39 measurements across 13 codebases**, with reported ratios of **3.4–23.9×**; Graphify was faster in five unchanged updates. One Linux machine, three CPUs/eight workers, mostly three measured runs; Kubernetes and VS Code used one. Graph counts differed slightly, so these are not graph-equivalence claims. [Method and tables](benchmarks.md#graphicle-0111-against-graphify-09100).

These historical results describe their named releases, not a new benchmark of the current 0.2.0 branch.

## Quality and correctness results

| Area | Recorded validation | Limit on the claim |
| --- | --- | --- |
| Million-file qualification | **73 public CLI checks** passed across ten repositories; initial graph **2,999,990 nodes / 2,999,973 edges**. All three 100,000-file Go graphs recovered the generated local calls. | Synthetic graph and bounded retrieval qualification. |
| Broad regression run at the bounded Go scale milestone | **1,341 tests passed, eight ignored**, in `crates/graphicle`; **536 context calls** passed the quality gate with no regression findings or input-integrity errors. | Historical run; not an entire engine/Python suite rerun. Full suites were not rerun for the later scale changes or for this document. |
| Retrieval quality | On the authored 195-question suite, links-enabled memory precision reached **93.64%**, pooled recall **100%**, and both source variants retained **88/88 required spans**. | Eleven false-positive note occurrences remain. Default counted-mode pooled recall is **50%**; the links-enabled result does not describe the default mode. |
| HTTP/framework extraction | **90 synthetic acceptance cases**. Expected method links **1/12 → 12/12**; URL/helper links **1/11 → 11/11**; incorrect local method-control links **8 → 0**. Added Next.js, Spring, Actix/Axum and Firebase fact coverage. | Development fixtures with successive baselines, not production recall or runtime benchmarks. |
| Environment-aware review selection | Go fixture **213 → 7** entry/diff candidates; Python fixture **67 → 7**, retaining the affected reader and consumer. | Controlled fixtures, not production PR token or reviewer-cost savings. |
| Interpretation guidance, 9–10 November | Fresh-file owner-adjudicated supported claims **20/24 → 23/24**; complete supported topics **11/24 → 16/24**. | Selected small sample; generator/reviewer from the same model family. Independent completeness confirmation and production accuracy remain unestablished. |

Sources: [million-file checks and regression receipts](evidence/large-repository-e15-2026-11-06.json), [retrieval quality](quality.md), [extraction](evidence/extraction-improvements.md), [environment fixtures](service-boundaries.md), [claim guidance](../bench/memory/claim-guidance.md).

## Sentinel deployment status

The recorded read-only observation on **6 November 2026, 23:03–23:06 IST** found the persistent service active, running `graphicle 0.2.0 (core)` from an artifact labelled `0.2.0-9a73af09`. The label is not a source-build attestation, and the observation did not deploy the later readiness changes or the November 10 interpretation update.

For the running binary, six selected records represented **three unique attempts**. All delivered context without recorded errors, and all skipped pinned impact analysis with `no_go_change`. Context-query p50/p95 was **1.07/2.33 s**; the broader Graphicle section, including source preparation, was **17.13/52.62 s**. With only three attempts, the reported p95 is the sample maximum and is not a reliable tail-latency estimate.

Nine unique pinned-impact attempts from older binaries delivered evidence but all reported incomplete impact and coverage. This establishes delivery, not complete analysis. The proposed observation period runs through 27 November, with reviews on 13, 20 and 27 November; the repository records a proposal, not scheduled automation or a completed observation window. [Deployment baseline and observation plan](large-repository-readiness.md#production-observation-through-sentinel).

## Remaining work

1. **Qualify the current release candidate:** run the full relevant regression/quality suites on the exact deployment build, then complete staging and rollback validation. Later targeted checks do not replace this gate.
2. **Resolve memory freshness after source restoration:** restored source can make a note eligible again while the freshness report still marks it stale; the reproduced catalogue reports 625 such stale markers.
3. **Address large memory catalogues:** ordinary and graph-assisted recall consider the newest 20,000 eligible notes and omit the oldest 5,000 in the 25,000-note fixture. Combined large-source/large-memory workloads remain unqualified.
4. **Set and validate operating budgets:** use representative real repositories and production request configurations to establish latency, peak memory, freshness, concurrency, recovery and retention requirements. The current graph-update costs and concurrent-review memory growth matter here.
5. **Measure reviewer outcomes separately:** real-query relevance, generated-answer correctness, completed coding tasks and end-to-end reviewer token/cost savings require their own comparisons. The quality fixtures and component speedups do not establish those outcomes.

The detailed acceptance criteria and retained experiment receipts are in the [production readiness assessment](large-repository-readiness.md#pending-work-for-full-production).
