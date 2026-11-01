import test from "node:test";
import assert from "node:assert/strict";
import {createHash} from "node:crypto";
import {applyReviewDelta, materializeServiceProvenance} from "../index.js";

test("compact service assertions verify hashes and survive unchanged deltas", async () => {
  const hash = value => createHash("sha256").update(JSON.stringify(value)).digest("hex");
  // Keys are in canonical order; Unicode is retained as UTF-8.
  const leaf = {authority: "pinned_configuration_assertion", citations: [{byte_end: 20, byte_start: 1, file: "服务.go", sha256: "a".repeat(64)}], manifest: {file: "graphicle-services.json", sha256: "b".repeat(64)}, runtime_observed: false};
  const leafId = hash(leaf), edge = {source_ref: leafId, target_ref: leafId}, edgeId = hash(edge);
  const packet = {packet_id: "one", previous_packet_id: null, base: [], profile: {},
    entries: [{id: "a", reasons: [{steps: [{evidence: "configured_static", provenance_ref: edgeId}]}]}],
    diffs: [], unchanged: [], invalidated: [], context_order: ["a"], tokens: 100,
    service_provenance: {format: "graphicle.service-provenance.v1", definitions: {[leafId]: leaf, [edgeId]: edge}}};
  packet.entries[0].service_boundary = {authority: leaf.authority, runtime_observed: false, resolution: {status: "unresolved"}, provenance_ref: leafId};
  const original = structuredClone(packet), full = await applyReviewDelta(null, packet);
  assert.deepEqual(packet, original);
  assert.deepEqual(full.entries[0].reasons[0].steps[0].provenance, {source: leaf, target: leaf});
  assert.equal(full.tokens, undefined);
  assert.deepEqual(full.entries[0].service_boundary.citations, leaf.citations);
  assert.deepEqual(full.entries[0].service_boundary.resolution, {status: "unresolved"});
  const delta = {...packet, packet_id: "two", previous_packet_id: "one", entries: [], unchanged: ["a"]};
  full.entries[0].reasons[0].steps[0].provenance.source.runtime_observed = true;
  const next = await applyReviewDelta(full, delta);
  assert.equal(next.entries[0].reasons[0].steps[0].provenance.source.runtime_observed, false);
  for (const mutate of [
    p => delete p.service_provenance,
    p => {p.service_provenance.format = "unknown";},
    p => delete p.service_provenance.definitions[leafId],
    p => {p.service_provenance.definitions[leafId].runtime_observed = true;},
    p => {p.entries[0].reasons[0].steps[0].provenance_ref = "missing";},
    p => {p.entries[0].service_boundary.runtime_observed = true;},
  ]) {
    const bad = structuredClone(packet); mutate(bad);
    await assert.rejects(materializeServiceProvenance(bad), /Invalid service provenance/);
  }
  const deep = {source_ref: edgeId};
  const bad = structuredClone(packet); bad.service_provenance.definitions[hash(deep)] = deep;
  await assert.rejects(materializeServiceProvenance(bad), /Invalid service provenance/);
  delete delta.service_provenance;
  await assert.rejects(applyReviewDelta(full, delta), /Invalid service provenance/);
});

test("body reuse keeps fresh bindings and requires the exact prior receipt", async () => {
  const text = "func F() {}", sha = createHash("sha256").update(text).digest("hex");
  const first = {packet_id: "one", previous_packet_id: null, base: [], head: ["old"], profile: {},
    entries: [{id: "a", revision: "old", repository_id: "repo", text, text_sha256: sha}], diffs: [],
    unchanged: [], invalidated: [], context_order: ["a"]};
  const next = {...first, packet_id: "two", previous_packet_id: "one", head: ["new"],
    entries: [{id: "a", revision: "new", repository_id: "repo", text_sha256: sha,
      text_ref: true}]};
  const materialized = await applyReviewDelta(await applyReviewDelta(null, first), next);
  assert.equal(materialized.entries[0].text, text);
  assert.equal(materialized.entries[0].revision, "new");
  assert.deepEqual(materialized.head, ["new"]);
  await assert.rejects(applyReviewDelta({...first, packet_id: "wrong"}, next), /Invalid review delta/);
  await assert.rejects(applyReviewDelta({...first, entries: [{...first.entries[0], text: "stale"}]}, next), /Invalid review delta/);
  await assert.rejects(applyReviewDelta(first, {...next, context_order: []}), /Invalid review delta/);
  const removed = await applyReviewDelta(materialized, {...next, packet_id: "three", previous_packet_id: "two", entries: [], context_order: [], invalidated: [{id: "a"}]});
  assert.deepEqual(removed.entries, []);
});
