# Pinned cross-service dependency paths

Pinned reviews join local calls with HTTP, gRPC and event relationships in one
graph. Paths can mix protocols, for example:

`checkout → HTTP endpoint → orders handler → gRPC method → pricing handler → publisher → subscriber → billing handler`.

This is static, configured wiring. It does not observe runtime traffic. It does
not infer deployment identity from a URL path, an RPC method name or a topic name
alone. The existing heuristic HTTP route extractor remains separate; its path
matches are not promoted into authoritative cross-service calls.

## Supplying wiring

Include a `graphicle-services.json` file at each service repository's root in
both admitted snapshot sets. The normal `impact.report`, `graph.impact`, CLI and
SDK review operations use it automatically. No external files, environment
variables, DNS, cloud APIs or network endpoints are read during resolution.

The file has this shape (replace illustrative hashes and offsets with exact
values from your pinned sources):

```json
{
  "format": "graphicle.service-boundaries.v1",
  "inventory_complete": true,
  "bindings": [{
    "id": "submit-order",
    "service": {
      "namespace": "commerce",
      "name": "checkout",
      "environment": "production",
      "configuration": "regional-routing-v3"
    },
    "direction": "outbound",
    "transport": {
      "protocol": "http",
      "authority": "https://orders.example",
      "method": "POST",
      "path": "/orders"
    },
    "owner": {
      "file": "checkout.go", "sha256": "FULL_FILE_SHA256",
      "byte_start": 100, "byte_end": 300
    },
    "site": {
      "file": "checkout.go", "sha256": "FULL_FILE_SHA256",
      "byte_start": 190, "byte_end": 240
    },
    "configuration": [{
      "directory": "deployments",
      "file": "production/orders.yaml", "sha256": "FULL_FILE_SHA256",
      "byte_start": 0, "byte_end": 120
    }],
    "contract": null,
    "dynamic": false
  }]
}
```

Declare the receiver separately with `direction: "inbound"`, its service
identity, the matching transport, its exact handler declaration in `owner`, and
the handler registration in `site`. `id` is stable within its repository.
Outbound sites must lie inside the cited owner. Byte ranges are half-open UTF-8
boundaries; hashes cover the entire file. `configuration` requires 1–8 citations.
`directory` is an optional pinned checkout name for configuration or contracts
in another repository. Owner and site stay in the declaring repository; the
owner must be in a pinned module. Configuration files can be outside modules.

Service identity includes namespace, service name, environment and configuration
identity. A connection additionally needs an exact transport authority or broker.
Namespaces and environments must agree across a link; configuration identity
belongs to each service and need not have the same name on both sides.

Other transport shapes:

```json
{"protocol":"grpc","authority":"grpcs://pricing.example","service":"pricing.v1.Price","method":"Quote"}
```

```json
{"protocol":"event","broker":"project-a/pubsub","topic":"orders","schema":"Order/v1","group":null}
```

gRPC and events require a `contract` citation to their IDL/schema. Both ends must
cite exactly the same complete contract-file hash. Compatible version ranges
are insufficient. Event subscribers supply a nonempty `group`. Separate groups
fan out; several handlers claiming one broker/topic/group are ambiguous even
when they belong to different services. Subscription-to-topic mappings must be
supplied explicitly and backed by configuration citations. Streaming RPCs use
the same fully qualified service/method identity as unary RPCs.

HTTP authorities include the scheme and listener port. Paths support literal
segments and `{parameter}`/`:parameter` segments. Multiple matching routes remain
ambiguous; framework route precedence is not guessed. Wildcard, query-string
and fragment-bearing route patterns are rejected. Supply the route path without
query parameters. A null authority, broker or topic, or `dynamic: true`, leaves
the target unresolved. Do not substitute environment-variable names as values.

## Evidence, safety and limits

These manifests are **source-bound configuration assertions**, not extracted
proof of deployment wiring. Hash checks establish which bytes were asserted;
they do not prove the mapping is semantically correct. Producers must review
their manifests against deployment configuration. `inventory_complete` is also
an assertion and defaults to false. Reports expose `inventory_verified: false`.
Missing inventories, source objects, configuration, ambiguous targets and parser
failures remain explicit coverage issues. An incomplete graph never returns an
unqualified no-impact result.

Boundary entries include service identity, transport, resolution targets and
citations containing repository, commit, snapshot, policy, full-file hash and
byte range. Relations are `invokes_boundary`, `handles_boundary`, `http_calls`,
`grpc_calls` and `event_delivers`. They participate in the same incoming/outgoing
traversal as compiler calls. `max_depth` counts every edge, including owner and
endpoint edges; use a sufficient depth for mixed-protocol paths. Depth and
token-budget omissions retain their existing coverage reporting.

Go owners must match a unique compiler declaration for the selected build.
Other supported source languages reuse the existing exact structural parser;
their unanalysed local calls remain reported as incomplete. No name-only or
nearest-function fallback is used. Native package review-coverage reuse is
conservatively disabled when service wiring is present; text reuse remains
revision-safe. Service-level review equivalence is not yet certified.

Only authorized admitted sources are read. A separate configuration repository
requires its own authorized pin. Existing final authorization, source-retention
and snapshot-binding checks apply. Compiler caches never cache authorization or
service relationships: wiring is relinked after every authorized input load.
Changed configuration hashes, contracts, owners, sites or removed subscriptions
update entries or invalidate old context through normal receipt deltas.

Limits: 512 KiB per manifest, 1,024 attempted bindings per snapshot side and
4,096 total service relationships including owner edges. Limit exhaustion is an
explicit coverage issue. Manifests are local source inputs; no separate mutable
service graph or unversioned registry is maintained.

## Accuracy evaluation

Use `crates/graphicle/tools/score_service_boundaries.py` with a materialized review packet and
independently labelled expected caller/target pairs. It measures exact-pair
precision/recall and false positives, counts unknown/unresolved or omitted
callers, and rejects a mismatched pinned revision set. It does not report edge
count growth as accuracy. Include duplicate operation names, other environments,
missing repositories, dynamic configuration and schema changes in each service
acceptance set. Evaluate extraction/inventory recall separately: perfect matching
of supplied assertions does not prove that all source call sites were collected.

## References and attribution

Implementation is independent of GitNexus; its noncommercial implementation was
not copied. CodeGraphContext's MIT regression
`test_find_callers_keeps_per_row_target_when_definitions_differ` from
`tests/unit/tools/test_query_correctness_batch.py` at commit
`93c9685bff9151f50a3121e9913e5ba2d27c1191` was adapted into the Rust per-caller
service-target test. Its copyright and permission notice are retained in
`crates/graphicle/licenses/CodeGraphContext-MIT.txt`.

## Literal environment inputs (v2)

Environment analysis runs by default in pinned review (`graph.impact` /
`impact.report`). Without a service manifest, Graphicle discovers Kubernetes YAML
or JSON within a repository containing Go or Python environment operations. It
links a single admitted application root (one Go module or Python project) to a
single workload with a single container. Python discovery also works without a
project manifest; multiple Python project roots remain ambiguous. The
association is labelled `same_repository_candidate`, with `service: null` and
no manifest assertion; it does not invent a production/staging identity or prove
runtime ownership. Existing literal-value and compiler completeness checks still
control whether selection can narrow.

Multiple modules, workloads or containers, and incomplete discovery retain
conservative scope with `environment_auto_scope_ambiguous`. Automatic discovery
never joins separate repositories. Use an explicit scope to resolve ownership
or join an admitted deployment repository. An existing service manifest is
authoritative: discovery does not override it, including invalid manifests or
an explicit empty `environment_scopes` list. This provides an explicit opt-out.

`graphicle.service-boundaries.v2` accepts the existing `bindings` plus optional
`environment_scopes`. Existing v1 manifests keep their behavior. Ordinary
question-based `context` retrieval does not acquire deployment bindings from
this extension. Application `config.json` keys are not automatically linked to
code readers: their edits retain existing conservative scope.

An explicit environment scope assigns one pinned Kubernetes container to one
admitted application root. Matching variable names alone never establishes ownership.
For example, add this manifest to the application repository:

```json
{
  "format": "graphicle.service-boundaries.v2",
  "inventory_complete": true,
  "bindings": [],
  "environment_scopes": [{
    "id": "checkout-env",
    "service": {
      "namespace": "commerce",
      "name": "checkout",
      "environment": "production",
      "configuration": "deployment-v1"
    },
    "module": ".",
    "deployment": {
      "directory": "deploy",
      "file": "checkout/deployment.yaml",
      "container": "checkout"
    }
  }]
}
```

`module` must be in that application's pin `modules` (normally `.` for Python). `deployment.directory` is
an optional **already admitted pin directory**, permitting a separate deployment
repository; omission means the application's own pin. No host files are read.
The parser checks the deployment namespace against `service.namespace` and
records the workload kind/name and container. Service and environment ownership
remain assertions made by this manifest, not discovered runtime facts. Include
only one scope per module and deployment file in a request; multiple competing
scopes (including production and staging for the same module) remain ambiguous.

The deployment must be one YAML or JSON document containing a `v1/Pod`, an
`apps/v1` Deployment/StatefulSet/DaemonSet, or a `batch/v1` Job/CronJob. Each env
entry must have a unique literal `name` (at most 200 characters, matching
`[A-Za-z_][A-Za-z0-9_]*`) and string `value`. Typed Go calls to
`os.Getenv("KEY")` and `os.LookupEnv("KEY")` produce `reads_environment` edges
from their containing declarations to the corresponding configuration input.
Import aliases are supported; comments, strings and local objects named `os`
do not create reads. Normal compiler call edges then bring in callers.

Python supports `os.getenv("KEY")`, `os.environ["KEY"]` and
`os.environ.get("KEY")`, including `import os as alias` and direct imports such
as `from os import getenv as read` / `from os import environ as env`. `getenv`
also accepts a literal `key=` argument. Reads use the existing Python file nodes
and incoming import edges, so context contains the reader file and its importing
files. Evidence is `python_import_binding`, not compiler resolution or proven
runtime calls.

Only unconditional module-level imports establish Python bindings. Rebinding
or shadowing an alias anywhere in the file withholds that alias conservatively;
conditional/wildcard imports, escaped bindings, dynamic execution, parse/operation
limits, and a pinned local `os.py` / `os/__init__.py` remain explicit uncertainty.
Unresolved non-`os` imports and unsupported `os` APIs also prevent narrowing;
known read links can still be supplied with these limitations. Interpolated,
escaped, concatenated and nonliteral keys are unresolved. Environment assignment,
deletion, `update`, `clear`, `pop`, `setdefault`, `putenv` and `unsetenv` prevent
narrowing. Python basic-analysis limitations still apply; import hooks, dynamic
execution and monkey-patching are not proved absent.

Environment entries carry `kind: "environment_input"`,
`analysis_level: "basic_file"`, and `environment_input` metadata with the key,
service (null for automatic candidates), workload, manifest identity when
supplied, and exact admitted declaration bytes.
Dependency steps carry the typed Go read location and configuration provenance.
No env values are added to counters; ordinary source citations remain subject
to existing admission policy. Secret files and live process environments are
not imported by this feature.

For a value-only edit, Graphicle can replace repository-wide fallback with the
mapped inputs and their consumers. Both snapshots must have the same valid
scope, env names/order, deployment structure and exact bytes outside the env
declarations. Added/removed keys, image/command changes, other-container edits,
namespace changes, embedded files, incomplete admissions or compiler inventories, unsupported
consumer languages, and ambiguous scopes retain conservative fallback.
`envFrom`, `valueFrom` (including ConfigMaps and Secrets), aliases, duplicate YAML
keys, multi-document YAML, Kubernetes `$(...)` expansion and unresolved Helm
expressions also retain fallback. Ambiguous inline Unicode citation positions
are refused; Unicode on other lines is supported. No Helm rendering or override
evaluation runs.

Dynamic Go keys, `os.Environ`, unbound keys, `Setenv`/`Unsetenv`/`Clearenv`, and
unresolved indirect/external call sites prevent narrowing. Even when narrowing
is eligible, `environment_analysis_limitations` remains explicit: image defaults,
indirect library behavior and actual deployment state are not proved. This
improves source selection; it does not make environment risk analysis complete.

Each side reports `service_graph.{base,head}.environment` with a stable model
identifier `literal_go_python_reads_and_kubernetes_values_v2`, `narrowable_files`, and
per-scope counters. Counts are `null` when the deployment could not be
validated/parsed, rather than claiming zero reads. Otherwise they count
discovered facts; `issues` and `narrowing_eligible` qualify their completeness:

| Field | Meaning |
| --- | --- |
| `declared_keys` | Distinct parsed literal env declarations |
| `literal_reads` | Typed Go or import-bound Python reads with a supported literal key |
| `resolved_reads` | Read sites linked to an admitted input and owner |
| `unresolved_reads` | Dynamic reads, unbound keys, missing owners or edge-limit omissions |
| `runtime_overrides` | Recognized Go or Python environment mutation sites |
| `narrowing_eligible` | This snapshot has no blocker for value-only selection; both snapshots must still match |
| `issues` | Machine-readable reasons for this scope's blockers |

Limits are 128 scopes per request side, 1 MiB / 20,000 YAML events / depth 64 per
source, 1,024 env declarations per container, and existing graph/packet bounds.
A malformed or bounded-out input remains explicit uncertainty. Default discovery
uses only admitted sources and never imports live environment values or secrets.

The regression fixture in `tests/it/environment_impact.rs` compares the same
value-only edit with explicit empty scopes (discovery disabled) and a supplied
scope, at a 64,000-token budget. A separate regression verifies automatic
selection without a manifest. With
100 unrelated functions it produces **213 → 7 entry/diff candidates (96.7%
fewer)** and **116–118 → 0 budget omissions** across recorded runs (packing
varies slightly with generated fixture identities). The affected reader and caller are
both retained; none of the 100 unrelated functions is retained. Candidate counts
include omitted entries, so the result is not an artifact of packet trimming.
These are controlled fixture measurements, not production PR results.

The Python regression uses a reader file, an importing file, an unrelated env
reader and 30 unrelated source files. Automatic mapping reduces **67 → 7
entry/diff candidates (89.6%)**, retains both affected files and excludes all 30
unrelated files. Python consumers remain file-granular. The environment model
marker is v2 to distinguish these measurements from the earlier Go-only model.
