import {Project, type ReviewPacket, type SnapshotPin, type ReviewProfile} from "../index.js";
declare const project: Project;
const pin: SnapshotPin = {repository_id: "repo", directory: "service", commit: "a".repeat(40), snapshot: "b".repeat(64), policy_sha256: "c".repeat(64)};
const profile: ReviewProfile = {goos: "linux", goarch: "amd64", tests: true};
const report: Promise<ReviewPacket> = project.impact([pin], [pin], {profile, max_tokens: 16000, previous_packet_id: "d".repeat(64)}, {timeoutMs: 120000});
void report;
// @ts-expect-error Episode IDs cannot represent pinned snapshot sets.
project.impact("base-episode", "head-episode", {profile});
// @ts-expect-error A build profile is mandatory.
project.impact([pin], [pin], {});
// @ts-expect-error The pinned review budget is named max_tokens.
project.impact([pin], [pin], {profile, budget_tokens: 4000});
// @ts-expect-error Pins require their exact commit, snapshot and admission policy.
project.impact([{repository_id: "repo", directory: "service"}], [pin], {profile});
