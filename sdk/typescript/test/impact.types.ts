import {Project, type ReviewPacket, type SnapshotPin, type ReviewProfile, type ServiceBoundaryManifest, type ServiceTransport} from "../index.js";
const boundaries: ServiceBoundaryManifest = {format: "graphicle.service-boundaries.v1", bindings: []};
const event: ServiceTransport = {protocol: "event", broker: null, topic: null, schema: "Order/v1", group: null};
// @ts-expect-error HTTP requires an explicit method and route path.
const invalidBoundary: ServiceTransport = {protocol: "http", authority: "https://orders.example"};
void boundaries; void event; void invalidBoundary;
declare const project: Project;
const pin: SnapshotPin = {repository_id: "repo", directory: "service", commit: "a".repeat(40), snapshot: "b".repeat(64), policy_sha256: "c".repeat(64)};
const profile: ReviewProfile = {goos: "linux", goarch: "amd64", tests: true};
const report: Promise<ReviewPacket> = project.impact([pin], [pin], {profile, max_tokens: 16000, previous_packet_id: "d".repeat(64)}, {timeoutMs: 120000});
project.impact([pin], [pin], {profile, business_context: {history: [{
  format: "graphicle.business-history.v1", pin, shallow: false,
  evolution: {format: "graphicle.history-analysis.v1", anchor_time: 1700000000, units: [], functions: [], omissions: {}},
  files: [{path: "main.go", file_sha256: "e".repeat(64), line_count: 2,
    omitted_ranges: 0, attributions: [{commit: pin.commit, line_start: 2,
      line_end: 2, subject: "Fix tenant isolation"}]}],
}]}});
// @ts-expect-error History must name an exact pin, not a loose commit.
project.impact([pin], [pin], {profile, business_context: {history: [{commit: "a"}]}});
void report;
// @ts-expect-error Episode IDs cannot represent pinned snapshot sets.
project.impact("base-episode", "head-episode", {profile});
// @ts-expect-error A build profile is mandatory.
project.impact([pin], [pin], {});
// @ts-expect-error The pinned review budget is named max_tokens.
project.impact([pin], [pin], {profile, budget_tokens: 4000});
// @ts-expect-error Pins require their exact commit, snapshot and admission policy.
project.impact([{repository_id: "repo", directory: "service"}], [pin], {profile});
project.searchSymbols({parameter_name: "tenant", parameter_type: "Tenant", annotation: "Override"});
project.executionFlows("node", {max_depth: 8, limit: 4});
// @ts-expect-error structured filters use exact strings
project.searchSymbols({parameter_type: 42});
// @ts-expect-error execution flows require an exact node identifier
project.executionFlows();
project.statementFlow("node", {goos:"linux",tags:["enterprise"]});
project.architecturePreview({remove_edges:[{source:"a",target:"b",relation:"calls"}],modules:{a:"billing"}});
project.secondaryContext("primary", "question", {enabled:true,expectedSnapshot:"a".repeat(64)});
// @ts-expect-error semantic flow requires an exact node ID
project.statementFlow();
// @ts-expect-error a dependency removal must identify its relation
project.architecturePreview({remove_edges:[{source:"a",target:"b"}]});
