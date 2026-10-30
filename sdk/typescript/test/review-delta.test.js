import test from "node:test";
import assert from "node:assert/strict";
import {createHash} from "node:crypto";
import {applyReviewDelta} from "../index.js";

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
