// Invoked by the Python product integration against a fixture-only Core server.
// The capability arrives on stdin and is never written to output or an argument.
import assert from "node:assert/strict";
import {Graphicle} from "../index.js";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const {url, workspace, capability, project, base, head, profile} = JSON.parse(input);
const client = new Graphicle({url, workspace, capability, timeoutMs: 120000});
const p = client.project(project);
const packet = await p.impact(base, head, {profile, max_tokens: 16000});
assert.equal(packet.format, "graphicle.review.v1");
assert.equal(packet.coverage.complete, true);
assert.ok(packet.changes.count > 0);
assert.ok(packet.entries.some(e => e.name.endsWith("Caller")));
const repeated = await p.impact(base, head, {profile, max_tokens: 16000, previous_packet_id: packet.packet_id});
assert.equal(repeated.entries.length, 0);
assert.equal(repeated.diffs.length, 0);
assert.ok(repeated.unchanged.length > 0);
assert.equal((await client.usage()).model_usage.calls, 0);
console.log(JSON.stringify({packet_id: packet.packet_id, repeated_entries: repeated.entries.length, repeated_diffs: repeated.diffs.length}));
