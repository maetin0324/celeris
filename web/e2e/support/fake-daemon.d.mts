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
  };
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
  files?: Record<string, { body: string | (() => string); type?: string; disposition?: string }>;
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
