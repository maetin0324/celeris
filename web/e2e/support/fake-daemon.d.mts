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
