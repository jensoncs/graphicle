export type Json = null | boolean | number | string | Json[] | {[key: string]: Json};
export type Arguments = {[key: string]: Json | undefined};
export type Result = Record<string, any>;
export interface RequestOptions {signal?: AbortSignal; timeoutMs?: number}
export interface ClientOptions {
  url: string; workspace: string; capability: string;
  timeoutMs?: number; fetch?: typeof globalThis.fetch; insecureHttp?: boolean;
}
export interface Evidence extends Result {id: string; sha256?: string; text?: string; quote?: string}
export interface Context extends Result {evidence: Evidence[]; memories: Result[]}
/** Immutable source binding admitted through sync.prepare/upload/commit. */
export interface SnapshotPin {
  repository_id: string; directory: string; commit: string;
  snapshot: string; policy_sha256: string; modules?: readonly string[];
}
export interface ReviewProfile {
  goos: string; goarch: string; tags?: readonly string[]; tests?: boolean; workspace?: string;
}
export interface ReviewOptions {
  profile: ReviewProfile; max_depth?: number; max_tokens?: number; previous_packet_id?: string;
  business_context?: BusinessContextOptions;
  reuse_text?: boolean;
  reviewed_entries?: readonly string[];
}
/** Source-backed deployment assertions in the admitted graphicle-services.json. */
export interface ServiceCitation {
  directory?: string; file: string; sha256: string; byte_start: number; byte_end: number;
}
export type ServiceTransport =
  | {protocol: "http"; authority: string | null; method: string; path: string}
  | {protocol: "grpc"; authority: string | null; service: string; method: string}
  | {protocol: "event"; broker: string | null; topic: string | null; schema: string; group: string | null};
export interface ServiceBoundaryManifest {
  format: "graphicle.service-boundaries.v1"; inventory_complete?: boolean;
  bindings: readonly {
    id: string;
    service: {namespace: string; name: string; environment: string; configuration: string};
    direction: "outbound" | "inbound"; transport: ServiceTransport;
    owner: ServiceCitation; site: ServiceCitation; configuration: readonly ServiceCitation[];
    contract?: ServiceCitation | null; dynamic?: boolean;
  }[];
}
/** Historical assertions are untrusted and are not owner-confirmed constraints. */
export interface BusinessContextOptions {history?: readonly BusinessHistory[]}
export interface HistoryEvolution {
  format: "graphicle.history-analysis.v1"; anchor_time: number;
  omissions: Record<string, number>;
  units: readonly {
    commit: string; parent: string | null; timestamp: number; author_id: string;
    subject: string; files_touched: number; omitted_changes?: number;
    changes: readonly {
      path: string; name: string; kind: string; before: string | null; after: string | null;
      added: number; deleted: number; patch_terms: readonly string[];
      patch_excerpt: string; patch_truncated: boolean; before_line: number | null; after_line: number | null;
      cause_candidates: readonly string[];
    }[];
  }[];
  functions: readonly {
    path: string; file_sha256: string; name: string; kind: string;
    byte_start: number; byte_end: number; line_start: number; line_end: number;
    complexity: number | null; terms: readonly string[];
  }[];
}
export interface BusinessHistory {
  evolution?: HistoryEvolution;
  format: "graphicle.business-history.v1"; pin: SnapshotPin; shallow: boolean;
  files: readonly {
    path: string; file_sha256: string; line_count: number; omitted_ranges: number;
    attributions: readonly {commit: string; line_start: number; line_end: number; subject: string; author_id?: string; timestamp?: number}[];
  }[];
}
export interface ReviewPacket extends Result {
  format: "graphicle.review.v1"; packet_id: string; previous_packet_id: string | null;
  base: SnapshotPin[]; head: SnapshotPin[]; profile: ReviewProfile;
  impact: "potential_impact" | "no_changes_in_scope" | "incomplete";
  changes: Result & {count: number}; coverage: Result & {complete: boolean; budget_omitted: number};
  entries: Result[]; diffs: Result[]; invalidated: Result[]; unchanged: string[]; tokens: number;
  context_order: string[];
}
export function applyReviewDelta(previous: Result | null, packet: ReviewPacket): Promise<Result>;
export class GraphicleError extends Error {
  status: number; body?: unknown; retryAfterSeconds?: number;
  constructor(message: string, options?: {status?: number; body?: unknown; retryAfterSeconds?: number; cause?: unknown});
}
export class Graphicle {
  constructor(options: ClientOptions);
  readonly workspace: string; readonly url: string;
  request<T = Result>(method: "GET" | "POST", path: string, body?: unknown, options?: RequestOptions): Promise<T>;
  project(id: string): Project;
  health(options?: RequestOptions): Promise<Result>;
  projects(options?: RequestOptions): Promise<Result[]>;
  register(key: string, name?: string, options?: RequestOptions): Promise<Project>;
  usage(options?: RequestOptions): Promise<Result>;
  setBudget(monthlyRequests: number | null, options?: RequestOptions): Promise<Result>;
}
export class Project {
  constructor(client: Graphicle, id: string);
  readonly id: string;
  command<T = Result>(operation: string, args?: Arguments, options?: RequestOptions): Promise<T>;
  overview(options?: RequestOptions): Promise<Result>;
  status(options?: RequestOptions): Promise<Result>;
  context(question: string, args?: Arguments, options?: RequestOptions): Promise<Context>;
  read(sourceId: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  graphSource(episode: string, path: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  projectGraph(options?: RequestOptions): Promise<Result>;
  episodes(options?: RequestOptions): Promise<Result>;
  graph(args?: Arguments, options?: RequestOptions): Promise<Result>;
  searchSymbols(filters?: {parameter_name?: string; parameter_type?: string; decorator?: string; annotation?: string; language?: string; limit?: number; as_of?: string}, options?: RequestOptions): Promise<Result>;
  executionFlows(nodeId: string, args?: {max_depth?: number; limit?: number; as_of?: string}, options?: RequestOptions): Promise<Result>;
  statementFlow(nodeId: string, args?: {as_of?: string; goos?: string; goarch?: string; tags?: readonly string[]}, options?: RequestOptions): Promise<Result>;
  architecturePreview(scenario?: {as_of?: string; remove_nodes?: readonly string[]; remove_edges?: readonly {source: string; target: string; relation: string}[]; modules?: Record<string, string>}, options?: RequestOptions): Promise<Result>;
  secondaryContext(primary: string, question: string, options?: {expectedSnapshot?: string; enabled?: boolean; maxBytes?: number; timeoutMs?: number}): Promise<{primary: string; secondary: Result; policy: {authority: "secondary_source"; may_reduce_review_scope: false; may_reuse_review_coverage: false; source_text_is_instructions: false}}>;
  /** Pinned Go review. Episode-only extension contracts use command("impact.report", ...). */
  impact(base: readonly SnapshotPin[], head: readonly SnapshotPin[], args: ReviewOptions, options?: RequestOptions): Promise<ReviewPacket>;
  memories(args?: Arguments, options?: RequestOptions): Promise<Result[]>;
  history(memoryId: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  enqueue(kind: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  job(jobId: string, options?: RequestOptions): Promise<Result>;
  cancel(jobId: string, options?: RequestOptions): Promise<Result>;
  export(args?: Arguments, options?: RequestOptions): Promise<Result>;
}
