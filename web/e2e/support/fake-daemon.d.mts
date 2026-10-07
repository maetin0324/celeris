export type FakeDaemonRequest = {
  path: string;
  method: string;
  at: number;
  aborted: boolean;
  authorization: string | null;
  cookie: string | null;
  query?: string;
  lastEventId?: string | null;
  body?: string;
};
export function fixtureFor(node: unknown): unknown;
export function validateFixture(value: unknown, node: unknown): string[];
export const defaultFixtures: Record<string, unknown>;
export function chatInboxItemsFixture(): import("../../api/generated/types").InboxItem[];
export function chatSeedFixture(): Array<{
  thread: import("../../api/generated/types").ChatThread;
  messages: import("../../api/generated/types").ChatMessage[];
}>;
// Deterministic chat controls (direct daemon URL, with the fixture token when configured):
// POST /__fixture/chat/hold {thread_id, hold?: boolean} creates a held active run.
// POST /__fixture/chat/threads/{t}/emit accepts {type, data, run_id?, message_id?}
// and broadcasts a persisted ChatEvent. POST /__fixture/chat/threads/{t}/expire
// {before_id} makes older SSE cursors return 410. POST /__fixture/chat/disconnect
// tears down all chat SSE connections (to test reconnection).
// POST /__fixture/chat/override-state {state: "succeed" | "conflict"} controls
// POST /api/v1/cos/operations/{o}/override (409 while conflict).
// GET /__fixture/chat/override-log lists the override bodies that were received.
// POST /__fixture/chat/upload-state {state: "hold" | "succeed" | "fail"} applies to new uploads.
// GET /__fixture/chat/uploads lists pending {id, name}; POST /__fixture/chat/uploads/{id}/release
// {state: "succeed" | "fail"} resolves one held upload. Disconnected uploads leave the pending list.
export type ChatFixtureControl = {
  rows: Map<
    string,
    {
      thread: import("../../api/generated/types").ChatThread;
      messages: import("../../api/generated/types").ChatMessage[];
      events: import("../../api/generated/types").ChatEvent[];
    }
  >;
  emit(
    threadId: string,
    event: Pick<import("../../api/generated/types").ChatEvent, "type" | "data"> &
      Partial<Pick<import("../../api/generated/types").ChatEvent, "run_id" | "message_id">>,
  ): import("../../api/generated/types").ChatEvent;
};
export const routingCatalogFixture: import("../../api/generated/types").RoutingCatalogView;
export const routingAuditFixture: import("../../api/generated/types").TaskRoutingView;
export const routingTrajectoryFixture: import("../../api/generated/types").TaskRoutingView;
export const routingShadowFixture: import("../../api/generated/types").TaskRoutingView;
export const routingEstimatorShadowFixture: import("../../api/generated/types").TaskRoutingView;
export const llmSourcesFixture: import("../../api/generated/types").LlmSourcesView;
export function richFixtures(): Record<string, unknown>;
export function richFiles(): Record<string, { body: string; type?: string }>;
export function inboxItemsFixture(): import("../../api/generated/types").InboxItem[];
export function noticesFixture(): import("../../api/generated/types").Notice[];
export const BROWSER_RAW_LIVE_VIEW_URL: string;
export type BrowserBackendOptions = {
  /** T2 の credential 待ち（W2）を置く。既定 true。置くと namespace の認証区間で Live View は 409 auth_interval。 */
  credentialWait?: boolean;
  /** gateway の attestation 鍵の公開鍵。渡すと assertion の署名を確かめる。 */
  publicKey?: import("node:crypto").KeyObject | null;
  /** 時刻（ms）。lease・grant の期限を試験の時計で進める。 */
  now?: () => number;
};
export type BrowserControlState = {
  phase: "agent_running" | "pausing" | "paused" | "human_control" | "stopped";
  version: number;
  lease_holder: string | null;
  lease_expires_at: number | null;
  in_flight: number;
  auth_section: boolean;
};
export type BrowserBackend = {
  records: {
    control: Array<{ key: string; holder: string; body: Record<string, unknown> }>;
    disconnect: Array<{ key: string; holder: string }>;
    waits: Array<{ task_id: string; wait_id: string; kind: "decision" | "credential"; body: Record<string, unknown> }>;
    grants: Array<{ key: string; owner: string }>;
    checks: Array<{ key: string; grant_id: string }>;
    reads: Array<{ key: string; after: string | null }>;
    identities: Array<{ method: string; path: string; query: string; body: Record<string, unknown> }>;
    devices: Array<{ method: string; path: string; purpose: string | null; body: Record<string, unknown> }>;
  };
  /** 信頼端末（ADR 2026-10-07-browser-trusted-devices）。hash を持つ内部の行。時刻は UNIX 秒。 */
  devices: Array<{
    id: string;
    name: string;
    created_at: number;
    last_used_at: number | null;
    expires_at: number;
    revoked_at: number | null;
    revoked_reason: string | null;
    secret_hash: string;
    prev_secret_hash: string | null;
  }>;
  waits: import("../../api/generated/types").BrowserWait[];
  identities: Array<{
    identity_id: string;
    project_id: string;
    origin: string;
    demand_confirmed_by: string;
    generation: number;
    created_at: number;
    expires_at: number;
    state: "active" | "revoked" | "deleted";
  }>;
  control: Map<string, BrowserControlState>;
  live: Record<string, Array<{ seq: number; body: Record<string, unknown> }>>;
  clock: { now: () => number };
  inboxItems(): import("../../api/generated/types").InboxItem[];
  setControl(task: string, run: string, session: string, patch: Partial<BrowserControlState>): void;
  appendLive(task: string, run: string, session: string, body: Record<string, unknown>): void;
};
export function createBrowserBackend(options?: BrowserBackendOptions): BrowserBackend;
export type FakeDaemonOptions = {
  host?: string;
  port?: number;
  delayMs?: number;
  fixtures?: Record<string, unknown | ((url: URL) => unknown)>;
  token?: string | null;
  files?: Record<string, { body: string | Uint8Array | (() => string); type?: string; disposition?: string }>;
  profile?: "default" | "rich";
  fault?: { status: number; paths?: string[] } | null;
  hold?: { paths?: string[] } | null;
  streamStatus?: number;
  inboxItems?: import("../../api/generated/types").InboxItem[] | null;
  notices?: import("../../api/generated/types").Notice[] | null;
  browser?: boolean | BrowserBackendOptions | null;
};
export function createFakeDaemon(options?: FakeDaemonOptions): {
  requests: FakeDaemonRequest[];
  chat: ChatFixtureControl;
  sendEvent(event: string, data?: unknown): void;
  inbox: {
    items: import("../../api/generated/types").InboxItem[];
    notices: import("../../api/generated/types").Notice[];
    answers: Array<{ id: string; option?: string; note?: string; payload?: unknown }>;
  };
  browser: BrowserBackend | null;
  browserSettingsEvents: Array<{
    actor: string;
    ts: string;
    before: Record<string, unknown>;
    after: Record<string, unknown>;
  }>;
  setInboxItems(items: import("../../api/generated/types").InboxItem[]): void;
  setNotices(notices: import("../../api/generated/types").Notice[]): void;
  setDelay(value: number): void;
  setStreamStatus(value: number): void;
  sendConsoleBlock(block: unknown): void;
  dropConsoleClients(): void;
  setFault(rule: { status: number; paths?: string[] } | null): void;
  releaseHeld(): void;
  readonly heldCount: number;
  dropStreamClients(): void;
  setPostDelay(ms: number): void;
  readonly consoleClients: number;
  readonly streamClients: number;
  start(): Promise<string>;
  close(): Promise<void>;
};
