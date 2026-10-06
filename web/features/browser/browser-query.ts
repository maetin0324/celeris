import type { BrowserRun, BrowserWaitList } from "../../api/generated/types";
import { getSessionQueryClient } from "../../api/query-client";

// Browser 操作は generic /api relay を通さず、本人確認を行う gateway の同一 origin 経路へ送る。
const ID = /^[0-9A-Za-z_-]{1,64}$/;
function id(value: string): string {
  if (!ID.test(value)) throw new TypeError("Invalid browser identifier");
  return value;
}

export const browserKeys = {
  all: ["browser"] as const,
  runs: (taskId?: string) => ["browser", "runs", taskId ?? "all"] as const,
  control: (taskId: string, runId: string, sessionId: string) =>
    ["browser", "control", taskId, runId, sessionId] as const,
  waits: (taskId: string) => ["browser", "waits", taskId] as const,
  identities: (projectId: string) => ["browser", "identities", projectId] as const,
  owner: ["browser", "owner-session"] as const,
};

export type BrowserRunItem = Omit<BrowserRun, "live_view_url"> & {
  live_path?: string;
  live?: { state: "link"; href: string } | { state: "disabled"; reason: string };
};
export type BrowserRunsResponse = { items: BrowserRunItem[] };
export type OwnerSession = { available: boolean; isOwner: boolean; csrfToken: string | null };
export type ControlStatus = {
  phase: "agent_running" | "pausing" | "paused" | "human_control" | "stopped";
  version: number;
  lease_expires_at: number | null;
  lease_holder?: string | null;
  in_flight: number;
  auth_section: boolean;
};
export type ControlCommand =
  | { kind: "pause" | "stop" }
  | { kind: "takeover" | "renew"; holder: string; ttl_secs: 60 }
  | { kind: "resume"; holder: string; fresh_snapshot: true; policy_origin_ok: true };
export type BrowserIdentity = {
  identity_id: string;
  project_id: string;
  origin: string;
  generation: number;
  expires_at: string;
  state: string;
};
export type BrowserIdentityList = { identities: BrowserIdentity[] };

export class BrowserGatewayError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
  ) {
    super(`Browser gateway: ${code} (${status})`);
  }
}

async function gateway<T>(path: string, method = "GET", body?: object, signal?: AbortSignal): Promise<T> {
  if (!path.startsWith("/browser/") || path.includes("..") || path.includes("\\"))
    throw new TypeError("Invalid browser gateway path");
  const response = await fetch(path, {
    method,
    credentials: "same-origin",
    cache: "no-store",
    headers: body ? { "Content-Type": "application/json" } : undefined,
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  const value = (await response.json()) as T & { code?: string };
  if (!response.ok) throw new BrowserGatewayError(response.status, value.code ?? "request_failed");
  return value;
}

export function browserRunsQuery(taskId?: string) {
  const path = taskId ? `/browser/runs?task_id=${id(taskId)}` : "/browser/runs";
  return {
    queryKey: browserKeys.runs(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => gateway<BrowserRunsResponse>(path, "GET", undefined, signal),
  };
}

export function ownerSessionQuery() {
  return {
    queryKey: browserKeys.owner,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      gateway<OwnerSession>("/browser/owner-session", "GET", undefined, signal),
  };
}

export function browserControlQuery(taskId: string, runId: string, sessionId: string) {
  const path = `/browser/control/${id(taskId)}/${id(runId)}/${id(sessionId)}`;
  return {
    queryKey: browserKeys.control(taskId, runId, sessionId),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      gateway<{ ok: true; status: ControlStatus }>(path, "GET", undefined, signal),
    refetchInterval: 2000,
  };
}

export function browserWaitsQuery(taskId: string) {
  const path = `/api/tasks/${encodeURIComponent(id(taskId))}/browser/waits`;
  return {
    queryKey: browserKeys.waits(taskId),
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const response = await fetch(path, { credentials: "same-origin", cache: "no-store", signal });
      const value = (await response.json()) as BrowserWaitList & { code?: string };
      if (!response.ok) throw new BrowserGatewayError(response.status, value.code ?? "request_failed");
      return value;
    },
  };
}

export function browserIdentitiesQuery(projectId: string) {
  const path = `/browser/identities?project_id=${id(projectId)}`;
  return {
    queryKey: browserKeys.identities(projectId),
    queryFn: ({ signal }: { signal: AbortSignal }) => gateway<BrowserIdentityList>(path, "GET", undefined, signal),
  };
}

/** 二重送信を拒否する。結果不明でも自動再送せず、利用者に再取得を促す。 */
export class BrowserActionGate {
  private pending = false;
  get busy(): boolean {
    return this.pending;
  }
  async run<T>(send: () => Promise<T>): Promise<T> {
    if (this.pending) throw new Error("browser_action_pending");
    this.pending = true;
    try {
      return await send();
    } finally {
      this.pending = false;
    }
  }
}

export const browserActionGate = new BrowserActionGate();

/** takeover から返却まで同じ値を保持する。UUID は lease holder の識別にも使う。 */
export function newControlHolder(): string {
  return crypto.randomUUID();
}

function invalidateBrowser(): void {
  void getSessionQueryClient().invalidateQueries({ queryKey: browserKeys.all });
}

async function mutate<T>(path: string, body: object, csrf: string, method = "POST"): Promise<T> {
  return browserActionGate.run(async () => {
    const result = await gateway<T>(path, method, { ...body, csrf });
    invalidateBrowser();
    return result;
  });
}

export function requestOwnerSession(): Promise<{ challenge: string }> {
  return browserActionGate.run(() => gateway<{ challenge: string }>("/browser/owner-session", "POST"));
}

export function sendControl(
  ids: { taskId: string; runId: string; sessionId: string },
  command: ControlCommand,
  expectedVersion: number,
  csrf: string,
): Promise<{ ok: true; status: ControlStatus }> {
  const path = `/browser/control/${id(ids.taskId)}/${id(ids.runId)}/${id(ids.sessionId)}`;
  return mutate(path, { command, expected_version: expectedVersion, idempotency_key: crypto.randomUUID() }, csrf);
}

export function releaseControl(ids: { taskId: string; runId: string; sessionId: string }, csrf: string) {
  const path = `/browser/control/${id(ids.taskId)}/${id(ids.runId)}/${id(ids.sessionId)}/release`;
  return mutate<{ ok: true }>(path, {}, csrf);
}

export function answerBrowserDecision(
  waitId: string,
  taskId: string,
  expectedVersion: number,
  decision: "approve_once" | "deny",
  csrf: string,
) {
  return mutate<{ ok: true; code: string }>(
    `/browser/waits/${id(waitId)}/decision`,
    { task_id: id(taskId), expected_version: expectedVersion, decision },
    csrf,
  );
}

export function answerBrowserCredential(
  waitId: string,
  taskId: string,
  expectedVersion: number,
  username: string,
  password: string,
  csrf: string,
) {
  return mutate<{ ok: true; code: string }>(
    `/browser/waits/${id(waitId)}/credential`,
    { task_id: id(taskId), expected_version: expectedVersion, username, password },
    csrf,
  );
}

export function createBrowserIdentity(
  input: { identity_id: string; project_id: string; origin: string; demand_confirmed_by: string; state: object },
  csrf: string,
) {
  return mutate<{ ok: true; identity: BrowserIdentity }>("/browser/identities", input, csrf);
}

export function revokeBrowserIdentity(identityId: string, projectId: string, csrf: string) {
  return mutate<{ ok: true; identity: BrowserIdentity }>(
    `/browser/identities/${id(identityId)}/revoke`,
    { project_id: id(projectId) },
    csrf,
  );
}

export function restoreBrowserIdentity(identityId: string, projectId: string, origin: string, csrf: string) {
  return mutate<{ ok: true }>(
    `/browser/identities/${id(identityId)}/restore`,
    { project_id: id(projectId), origin },
    csrf,
  );
}

export function deleteBrowserIdentity(identityId: string, projectId: string, csrf: string) {
  return mutate<{ ok: true }>(`/browser/identities/${id(identityId)}`, { project_id: id(projectId) }, csrf, "DELETE");
}
