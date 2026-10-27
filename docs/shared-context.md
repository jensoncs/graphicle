# Shared context in GCS

`graphicle shared-context` publishes immutable repository-source snapshots and
queries an exact snapshot from another worker. It reuses Graphicle's existing
source manifests, graph projection and context packer. It does not export the
live SQLite store, workspace credentials, connector documents, sessions or
reviewed memories. These source-only packets are not replacements for a full
reviewer's existing context inputs.

The proposed pilot prefix is `gs://code-review-transcripts/graphicle/`. Configure
it in the worker environment; changing buckets requires no code change:

```sh
export GRAPHICLE_SHARED_CONTEXT_PREFIX=gs://code-review-transcripts/graphicle/
```

An explicit `--prefix gs://another-bucket/path/` overrides this variable for one
command. With neither set, the command fails. There is no hardcoded destination;
installing Graphicle enables no uploads.

## Publish

The trusted review materializer first syncs the admitted source files into an
isolated project using the existing sync API or CLI. Export uses the existing
materialization and extraction cache to build with the running engine, avoiding
stale projections from an older executable. It writes an identity file containing:

```json
{
  "repository": "ShareChat/argus",
  "commit": "<full lowercase Git commit ID>",
  "source_snapshot": "<snapshot returned by sync>",
  "dependencies_sha256": "<SHA-256 of the selected dependency content manifest>",
  "profile_sha256": "<SHA-256 of the build and retrieval profile>",
  "exclusions_sha256": "<SHA-256 of the admitted-source policy>"
}
```

The source snapshot is Graphicle's canonical manifest digest, not a Git tree
ID. Graphicle independently checks all source bytes against this digest.
The materializer is responsible for proving the repository/commit binding,
dependency completeness and exclusions before supplying this identity. These
fields are attestations from the caller, not proofs created by uploading them.
Unknown or incomplete dependencies must not be labelled complete by the harness.

Supply a short-lived Google OAuth access token in
`GRAPHICLE_GCS_ACCESS_TOKEN` through the worker's existing credential provider.
Do not put tokens in arguments, identity files, snapshots or logs. This version
does not discover credentials, invoke `gcloud`, or refresh an expired token.

```sh
graphicle --data-dir /private/worker-store shared-context publish \
  --project argus-candidate \
  --identity /private/identity.json \
  --expires-at <explicit-expiry-in-Unix-seconds>
```

Only a local workspace owner can export. The export reads one transaction and
refuses a changed source manifest, duplicate paths, excluded paths or an empty
source set. Current built-in source exclusions are rechecked. Custom policy and
content-sensitive exclusions remain the materializer's responsibility; a hash
of a policy does not apply that policy. A configured prefix must have the correct
audience for all admitted source in it.

Each artifact includes the full executable SHA-256, expiry, sources and graph,
including unresolved diagnostics. Its exact serialized bytes determine its key:

```text
graphicle/v1/<repository-identity-sha256>/<artifact-sha256>.json
```

Uploads use GCS `ifGenerationMatch=0`. Concurrent identical uploads can create
only one object. An existing key counts as reuse only after a byte-for-byte
readback matches. New uploads are read back too. Retry an interrupted upload
with the same identity, build and expiry; it cannot overwrite another artifact.
The result contains `uri`, `snapshot`, `verified`, `reused` and `expires_at`.
There is a 64 MiB serialized artifact limit and a 120-second HTTP timeout.

## Query from another worker

```sh
graphicle shared-context context \
  --identity /private/identity.json \
  --snapshot <artifact-sha256-from-publish> \
  --max-tokens 8400 \
  'Where is the request validated?'
```

Use the identical Graphicle executable and expected identity. The command
downloads the exact artifact, verifies its size, SHA-256, source manifest,
identity, executable and expiry, then uses the existing local context packer.
It neither syncs a working directory nor builds a new graph, and calls no model.
Each query performs an authenticated GCS read; it never serves a stale local
copy after GCS denies access. No persistent download cache is created.

Changing the commit, dependencies, build profile, exclusion policy or executable
requires a new binding. An expired, missing, corrupt, incompatible or inaccessible
artifact produces an error. The harness decides whether to rebuild from currently
authorized source; the command never substitutes a different commit silently.

The packet identifies its scope as `repository_sources_only`, its commit binding
as `caller_attested`, and its semantic authority as `unverified_candidate`.
It explicitly reports `complete_review_context: false`, the excluded context
sources, source-file count and graph limitations. Normal retrieval coverage and
token-budget reports remain present. A bounded ranked packet is not a guarantee
that every relevant excerpt has been retrieved.
Sharing does not turn syntax-derived facts into qualified Go facts. The existing
qualified-reader prerequisite in issue #303 is still required for that authority.

## Baselines, permissions and cleanup

The harness owns sibling/stacked PR isolation and authenticated target-commit
observations. These commands create candidate artifacts only: they never update a
`latest` pointer, inspect GitHub, promote an unmerged PR, or publish a baseline.
If a harness adds a target pointer, update it with GCS generation preconditions
and continue requiring exact commit identities on reads. GCS and GitHub do not
form a distributed transaction.

GCS IAM controls remote access; local Graphicle credentials are never sent to GCS.
The reader's service account must be authorized for the repository at query time.
An artifact's repository name is not an access-control list. Enforce that check in
the harness and restrict IAM to the appropriate prefix/audience. Publisher access
requires object create and read; readers require object read. These commands do
not change IAM, create buckets, list objects or delete objects.

Expiry is mandatory and is enforced on publish and query. It stops serving data;
it does **not** delete the GCS object. Physical retention, bucket soft-delete or
versioning behavior, aggregate storage quotas and lifecycle cleanup must be
selected and configured separately by the bucket owner. Do not use the evidence
bundle or an entire store backup as this source-only artifact format.

## Validation and delivery boundary

Local tests exercise serialization, source-manifest integrity, all identity
fields, expiry, exclusions, owner-only export, concurrent create-only uploads,
isolated sibling snapshots, missing sources/objects, truncated downloads,
rename/deletion, repeat-export parity, corrupt existing objects and denied reads through an HTTP mock
of the GCS JSON API. They do not certify live GCS IAM/lifecycle configuration or
the complete shared-graph pilot. Live upload, remote checksum verification,
Ubuntu acceptance, full dependency loading and the frozen semantic corpus remain
separate deployment/integration checks.

For importing verified source snapshots and comparing pinned Go base/head sets, see [pinned review](pinned-review.md).
