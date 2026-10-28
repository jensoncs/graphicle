# Graphicle for TypeScript and JavaScript

Node 22+, ESM, no runtime dependencies. This is a **source package**; @graphicle/sdk
is not published to a registry. From the root of a Graphicle checkout:

~~~sh
npm install ./sdk/typescript
~~~

~~~ts
import { Graphicle } from "@graphicle/sdk";

const client = new Graphicle({
  url: process.env.GRAPHICLE_URL!,
  workspace: process.env.GRAPHICLE_WORKSPACE!,
  capability: process.env.GRAPHICLE_KEY!
});
const project = client.project(process.env.GRAPHICLE_PROJECT!); // existing ID
const packet = await project.context("Where is authentication checked?", {max_tokens: 3000});
console.log(packet.evidence);
~~~

register(key, name) explicitly creates/reuses a project. project(id) never creates
one. The Python client's project(key) registers; use find_project for read-only
discovery.

The client exposes read, graphSource, projectGraph, episodes, graph, impact,
memories, history, enqueue, job, cancel, export, and a generic command.
client.usage() and client.setBudget() require an owner credential.
The service's /docs and /v1/openapi.json describe the versioned transport.

All calls accept a final {signal, timeoutMs} request option. Requests never follow
redirects and are never automatically retried. A timeout can occur after a
mutation commits: inspect current state before retrying. GraphicleError provides
status, parsed body, and retryAfterSeconds when the server sends it. HTTP is
restricted to loopback unless insecureHttp: true is explicitly chosen.
Capabilities remain private client fields; never ship them in public browser code.

Run `npm ci` to install the pinned development compiler, then `npm test` to run
the runtime tests and strict TypeScript contract checks. TypeScript declarations
are shipped alongside the JavaScript runtime; the client has no runtime
dependencies or build step.

## Pinned change review

`Project.impact(base, head, args, requestOptions?)` accepts arrays of `SnapshotPin`
and requires `args.profile: ReviewProfile`. It returns `Promise<ReviewPacket>`.
For Core, `build.info.change_impact_scope` is `pinned-go-snapshot-sets-v1`.
The capability boolean alone does not describe an extension's request format.

First admit both source snapshots using `sync.prepare`, `sync.upload`, and
`sync.commit`. Prepare metadata must include the full Git `commit`,
`policy_sha256`, and an accurate `scope_complete` attestation. Each pin uses the
source-manifest snapshot digest returned by sync:

```ts
import type {SnapshotPin, ReviewProfile} from "@graphicle/sdk";

const basePin: SnapshotPin = {
  repository_id: project.id, directory: "service",
  commit: baseCommit, snapshot: baseSnapshot, policy_sha256: policySha256,
  modules: ["."],
};
const headPin: SnapshotPin = {...basePin, commit: headCommit, snapshot: headSnapshot};
const profile: ReviewProfile = {goos: "linux", goarch: "amd64", tags: [], tests: true};
const packet = await project.impact([basePin], [headPin], {profile, max_tokens: 16000});
const delta = await project.impact([basePin], [nextHeadPin], {
  profile, max_tokens: 16000, previous_packet_id: packet.packet_id,
}, {timeoutMs: 120000});
```

Read `changes.count`, `entries`, `diffs`, and `coverage.complete`. Apply deltas
across both entries and diffs using their `update` fields, `invalidated`, and
`unchanged`. Incomplete coverage never means no impact. Core's first review
implementation is Go-only; unsupported languages produce explicit limitations.
No model is called.

Migration from 0.1.8 to 0.2.0: episode strings and `budget_tokens` are not Core review arguments.
Use pinned arrays, a build profile, and `max_tokens`. Graph episodes remain valid
for `graphSource` and `episodes`. An extension that explicitly retains the old
contract can still be called with
`project.command("impact.report", {base: baseEpisode, head: headEpisode, budget_tokens: 4000})`.
See [the pinned review guide](../../docs/pinned-review.md) for source admission
and the complete response contract.
