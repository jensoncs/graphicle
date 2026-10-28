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
}
export interface ReviewPacket extends Result {
  format: "graphicle.review.v1"; packet_id: string; previous_packet_id: string | null;
  base: SnapshotPin[]; head: SnapshotPin[]; profile: ReviewProfile;
  impact: "potential_impact" | "no_changes_in_scope" | "incomplete";
  changes: Result & {count: number}; coverage: Result & {complete: boolean; budget_omitted: number};
  entries: Result[]; diffs: Result[]; invalidated: Result[]; unchanged: string[]; tokens: number;
}
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
  /** Pinned Go review. Episode-only extension contracts use command("impact.report", ...). */
  impact(base: readonly SnapshotPin[], head: readonly SnapshotPin[], args: ReviewOptions, options?: RequestOptions): Promise<ReviewPacket>;
  memories(args?: Arguments, options?: RequestOptions): Promise<Result[]>;
  history(memoryId: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  enqueue(kind: string, args?: Arguments, options?: RequestOptions): Promise<Result>;
  job(jobId: string, options?: RequestOptions): Promise<Result>;
  cancel(jobId: string, options?: RequestOptions): Promise<Result>;
  export(args?: Arguments, options?: RequestOptions): Promise<Result>;
}
