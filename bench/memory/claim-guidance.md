# Evidence-scoped interpretation guidance

The shared `interpretation.contract` now asks the proposer to keep code claims
within visible evidence and preserve the conditions on branches and SQL
predicates. The change extends `semantic::INSTRUCTIONS`, which is consumed by
connected agents and configured proposers.

Passing a value to an omitted helper supports a claim about the call, but does
not establish the helper's side effects. Comments and documentation need
explicit attribution in the claim. For optional SQL predicates, an empty or
NULL argument can leave `x = x`, which still excludes NULL values in a `WHERE`
clause. The proposer should preserve those limits and narrow unsupported claims.

The claim schema, exact-quote validation and approval requirement are unchanged.
Native validation still does not prove semantic entailment. This change adds no
model calls, retrieval expansion or automatic acceptance gate.

## Measurements, 9–10 November 2026

These are sequential paired experiments, not one cumulative accuracy estimate.
Within each pair, source windows, topics, model and schema were fixed; only the
instructions changed. Generation used local, tool-free Claude sessions, with
one first output per arm per case. The fresh-file experiments used
`claude-sonnet-5-5` for generation and a separate `claude-opus-5-5` review session.

| Comparison | Evaluation | Baseline | Candidate |
|---|---|---|---|
| Original instructions → helper/attribution guidance | Owner-scored support, 12 known cases | 17/36 claims | 33/36 claims |
| Helper guidance → helper + predicate guidance | Separate blind review, 12 fresh-file cases | 22/24 claims | 24/24 claims |
| Same fresh-file outputs, after owner adjudication | Supported claims | 20/24 | 23/24 |
| Same fresh-file outputs, after owner adjudication | Owner-scored complete supported topics | 11/24 | 16/24 |
| Both accepted paragraphs → extra consistency paragraph | Separate blind support and topic-completeness review, six further cases | 12/12 for both metrics | 12/12 for both metrics |

The first experiment also introduced one contradicted candidate claim; its
large development gain is not an error-free result. In the fresh-file evaluation,
24 generation calls produced 48 claims from 14 files excluded from earlier
inputs, split evenly between gateway and other service cases. Every proposal
passed native validation. Owner adjudication addressed early-return guarantees
and unproven SQL row cardinality, preserving the original blind labels. Three
paired support improvements remained, with no paired support regressions.

The fresh-file result is a **qualified positive**, suitable for reviewed use.
Topic completeness was scored by the owner, so the preregistered requirement
for independent completeness confirmation was not established. Cases were
selected, the sample is small, and generator and reviewer belong to the same
model family. Separate sessions do not establish human or model-family
independence. These results do not estimate production accuracy or validate
deployed behavior. No new model calls were made to prepare this change.

The additional consistency paragraph failed its coverage-improvement gate and
is not included. An earlier evidence-expansion experiment also introduced a
contradicted claim and did not justify enabling default expansion. Jev was not
the judge for these guidance measurements.

## Local audit record

Frozen inputs, first outputs, reviewer labels, adjudications and replay scripts
remain local under the benchmark kit's ignored `runs/` directory. They include
private source excerpts and are not distributed with this summary. These
SHA-256 digests identify the preserved aggregate reports; they are provenance
references, not a substitute for independently reproducing the evaluation.

| Report | SHA-256 |
|---|---|
| `claim-guidance-20261109.json` | `61ee130bebebf25ba11b1b15bb95908f23b762cfed02fc8cce6f64100456d193` |
| `fresh-claims-adjudicated-20261110.json` | `650d74a4813f037c213f834934a5db78fd988dff4c3571a074268591e7aff561` |
| `claim-consistency-review-20261110.json` | `db7c7fc812994bf9061a9c056e47118dd0775b6b336bd0d2de40d0bd8616b6d4` |

Future guidance changes should compare fixed first outputs on fresh excerpts,
with blind support **and** topic-completeness labels. Score wording changes
only when they are part of the generated output; keep live checks and proposed
rewrites separate from frozen-window judgments.
