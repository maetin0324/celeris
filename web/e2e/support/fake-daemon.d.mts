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
export const routingCatalogFixture: import("../../api/generated/types").RoutingCatalogView;
export const routingAuditFixture: import("../../api/generated/types").TaskRoutingView;
export const routingTrajectoryFixture: import("../../api/generated/types").TaskRoutingView;
export const llmSourcesFixture: import("../../api/generated/types").LlmSourcesView;
export function createFakeDaemon(options?: {
  host?: string;
  port?: number;
  delayMs?: number;
  fixtures?: Record<string, unknown | ((url: URL) => unknown)>;
  token?: string | null;
  files?: Record<string, { body: string | (() => string); type?: string; disposition?: string }>;
}): {
  requests: FakeDaemonRequest[];
  sendEvent(event: string, data?: unknown): void;
  setDelay(value: number): void;
  setStreamStatus(value: number): void;
  sendConsoleBlock(block: unknown): void;
  dropConsoleClients(): void;
  setPostDelay(ms: number): void;
  readonly consoleClients: number;
  readonly streamClients: number;
  start(): Promise<string>;
  close(): Promise<void>;
};
