export type FakeDaemonRequest = {
  path: string;
  method: string;
  at: number;
  aborted: boolean;
  authorization: string | null;
  cookie: string | null;
};
export function fixtureFor(node: unknown): unknown;
export function validateFixture(value: unknown, node: unknown): string[];
export const defaultFixtures: Record<string, unknown>;
export function createFakeDaemon(options?: {
  host?: string;
  port?: number;
  delayMs?: number;
  fixtures?: Record<string, unknown>;
  token?: string | null;
  files?: Record<string, { body: string; type?: string; disposition?: string }>;
}): {
  requests: FakeDaemonRequest[];
  sendEvent(event: string, data?: unknown): void;
  setDelay(value: number): void;
  start(): Promise<string>;
  close(): Promise<void>;
};
