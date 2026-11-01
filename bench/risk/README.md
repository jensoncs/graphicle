# Local risk-evidence pilot

Run Graphicle's real source admission, history collector, compiler analysis and
pinned review locally, without models or production services. This is a pilot
of **evidence delivery for Sentinel**, not an implementation or evaluation of
Sentinel's risk verdict. Passing means the harness and safety controls passed;
inspect `gaps` and recall separately. A passing run can have missing evidence.

```sh
python3 -B bench/risk/pilot.py \
  --binary /absolute/path/to/graphicle \
  --output /tmp/graphicle-risk-pilot
python3 -B -m unittest discover -s bench/risk -p 'test_*.py' -v
```

The output directory must be new. Python 3.10+, Git, Go, and a built Graphicle
binary are required. The runner uses the standard library and the repository's
Python SDK. Sources are authored inline fixtures; it does not scan local
repositories or load credentials. An isolated temporary server binds to loopback
for source admission and a synthetic CI coverage upload, then stops. Review calls
run through the CLI over the same private store. There are no configured models;
Go dependency downloads and the update notifier are disabled. This is not
Sentinel's Linux sandbox or a network-isolation test.

The six cases cover a tenant guard regression with earlier fix history, a
discarded error and its callee contract, unresolved callback dispatch, a
documentation-only negative control, changed-code budget pressure, and a
configured HTTP → gRPC → event chain. The fixture labels are authored evidence
requirements, not independent real-world risk labels. They never enter Graphicle
requests. The synthetic coverage observation is not production traffic.

## Measurements and artifacts

Default budgets are 4,000 and 32,000 tokens. At each budget the same pins are
reviewed with static evidence and with collected history. Each case also gets a
same-request warm replay and a phase-instrumentation-off control at the largest
budget. The service chain adds a depth-zero request. The default run has 37
requests. The runner records cache state; “first” means first in that cell, not
necessarily cold. It does not clear caches between cells. Compare quality at
equal budgets, and compare latency only for matched cache states.

Artifacts include:

- `fixtures.json`: complete authored sources and evaluator-only evidence labels.
- `CASE/setup.json`: exact base/head pins, repository IDs and setup wall time.
- `CASE/*-history.json`: actual history-collector output and separate collection
  timing files. Collection and source admission are outside review latency.
- `CASE/VARIANT-BUDGET-SAMPLE/request.json` and `packet.json`: exact review input
  and output; no gold evidence labels are sent.
- `diagnostics.json`: per-label delivery, explicit unavailable/not-requested
  states, source/flow/history omissions, observed coverage issues, cache state,
  selected-entry reasons and request-correlated phase samples.
- `stderr.log`: the opt-in phase trace from exactly that CLI process.
- `measurement.json`: request timing, token count and quality counters, or a
  failed-call record. CLI startup is included; failed requests remain in recall
  denominators. Undefined recall/precision is not converted to a perfect score.
- `summary.json`: binary/runner/fixture hashes, platform, settings, quality gaps,
  grouped measurements, control checks and optional paired regressions.

Timers reuse `GRAPHICLE_PHASE_TIMINGS=1`. Review now separately times service
linking on each side, execution-coverage joins and historical candidate selection,
in addition to source loading, compiler analysis, comparison, business annotation,
reuse decisions and packing. Nested phase durations may overlap. RSS is process
resident memory and process high-water memory, not allocations caused by a phase.
The control requires identical packets with timing enabled and disabled, ignoring
only cache diagnostics and the corresponding wire token count.

Source excerpts are checked against admitted fixture bytes, full-file hashes and
the exact request pins/profile. Negative controls detect irrelevant evidence on
specific labelled paths; the suite does not claim exhaustive precision over all
unlabelled entries. Aggregate omission counters do not locate a particular miss:
unattributable misses remain `not_delivered_stage_unknown`. An empty flow or
missing observation cannot lower risk. Diagnostic business criticality and
production hotness remain `unassessed`; the risk verdict is `unmeasured`.

For an optimization, retain a baseline and run the same fixture contract:

```sh
python3 -B bench/risk/pilot.py --binary /path/to/candidate/graphicle \
  --output /tmp/graphicle-risk-candidate \
  --baseline /tmp/graphicle-risk-pilot/summary.json
```

Comparison refuses changed fixture hashes, budgets, profiles or protocol versions.
Lost previously delivered labels, new failures, missing runs and additional
negative-control hits fail the gate. Timing is descriptive and never gates on a
single local run. Changing labels is a new experiment, not an optimization win.

## Sentinel integration boundary

The indexed `ShareChat/agent-poc-space` implementation inspected on 1 November
2026 has two separate paths:

- [`cra/context/graphicle.py`](https://github.com/ShareChat/agent-poc-space/blob/main/cra/context/graphicle.py)
  retrieves supplemental head-source excerpts, enabled only for live reviews
  with `GRAPHICLE_CONTEXT=1`. Its frozen prompt block and sandbox remain owned by
  Sentinel. It currently calls `context`, not `impact.report`.
- [`cra/context/risk.py`](https://github.com/ShareChat/agent-poc-space/blob/main/cra/context/risk.py)
  computes deterministic R0–R3 routing from its own facts. Unavailable facts
  escalate; lowering risk needs approved context. It separately decides human
  approval and reviewer depth. The Graphicle excerpt adapter does not currently
  feed these pinned packets into that classifier.

This pilot does not modify either path or claim compatibility with the existing
excerpt renderer. The proposed integration should consume pinned review packets
as supplemental facts, preserve all uncertainty/provenance, and leave Sentinel's
existing tier, approval and reviewer-depth policies authoritative. Diagnostics
must stay outside reviewer evidence, and labels outside both retrieval and prompts.
An empty/failed response must not erase primary context or permit reduced review.

Next, replay independently labelled real changes through the actual Sentinel
adapter. Capture baseline and enriched facts, tier, human-routing and pair-count
decisions, exact evidence delivered to the assessor, omissions, model usage and
wall time. Evaluate missed high-risk changes and false escalations. Use only
information available at the reviewed revision; later fixes belong in evaluator
labels, not input history. This local fixture pilot supplies instrumentation and
an evidence regression gate, not that end-to-end prediction evaluation.
