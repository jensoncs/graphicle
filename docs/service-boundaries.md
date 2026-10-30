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
