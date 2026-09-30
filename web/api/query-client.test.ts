import { MutationObserver, onlineManager, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearProtectedCaches } from "../lib/session";
import { ApiError, apiGet, apiMutate, configureApiClient } from "./client";
import { taskKeys } from "./queries/keys";
import {
  bindQueryClientToSession,
  createQueryClient,
  endQuerySession,
  getSessionQueryClient,
  MAX_QUERY_RETRIES,
  shouldRetryQuery,
} from "./query-client";

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });
const err = (kind: ApiError["kind"], method = "GET") => new ApiError(kind, { method, path: "/api/x" });

afterEach(() => {
  vi.useRealTimers();
  onlineManager.setOnline(true);
  endQuerySession();
  configureApiClient({ fetcher: (i, init) => fetch(i, init), onUnauthorized: () => {} });
});

describe("shouldRetryQuery", () => {
  it("GET の一時的な失敗だけ、上限まで再試行する", () => {
    for (const kind of ["timeout", "network", "server", "rate_limited"] as const) {
      expect(shouldRetryQuery(0, err(kind))).toBe(true);
      expect(shouldRetryQuery(MAX_QUERY_RETRIES, err(kind))).toBe(false);
    }
  });

  it("401 / 403 / 検証エラー / 404 / 409 / 中断は再試行しない", () => {
    for (const kind of [
      "unauthorized",
      "forbidden",
      "validation",
      "not_found",
      "conflict",
      "aborted",
      "parse",
    ] as const) {
      expect(shouldRetryQuery(0, err(kind))).toBe(false);
    }
  });

  it("GET 以外と ApiError 以外は再試行しない", () => {
    expect(shouldRetryQuery(0, err("server", "POST"))).toBe(false);
    expect(shouldRetryQuery(0, new Error("boom"))).toBe(false);
  });
});

describe("QueryClient", () => {
  it("GET の query は上限付きで再試行して止まる", async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn<typeof fetch>(async () => json(503, {}));
    configureApiClient({ fetcher });
    const client = createQueryClient();
    const result = client
      .fetchQuery({ queryKey: taskKeys.list(), queryFn: ({ signal }) => apiGet("/api/tasks", signal) })
      .catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(await result).toMatchObject({ kind: "server" });
    expect(fetcher).toHaveBeenCalledTimes(1 + MAX_QUERY_RETRIES);
  });

  it.each([401, 403, 422])("GET の %i は再試行しない", async (status) => {
    const fetcher = vi.fn<typeof fetch>(async () => json(status, {}));
    configureApiClient({ fetcher });
    const client = createQueryClient();
    await expect(
      client.fetchQuery({ queryKey: taskKeys.detail("t1"), queryFn: ({ signal }) => apiGet("/api/tasks/t1", signal) }),
    ).rejects.toBeInstanceOf(ApiError);
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it("変更系は失敗しても再送しない", async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn<typeof fetch>(async () => json(503, {}));
    configureApiClient({ fetcher });
    const observer = new MutationObserver(createQueryClient(), {
      mutationFn: () => apiMutate("POST", "/api/tasks/t1/cancel"),
    });
    const result = observer.mutate().catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(await result).toMatchObject({ kind: "server" });
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it("offline で止めて復帰時に送り直すことをしない", async () => {
    onlineManager.setOnline(false);
    const fetcher = vi.fn<typeof fetch>(async () => json(503, {}));
    configureApiClient({ fetcher });
    const observer = new MutationObserver(createQueryClient(), {
      mutationFn: () => apiMutate("POST", "/api/tasks/t1/cancel"),
    });
    await observer.mutate().catch(() => undefined);
    expect(observer.getCurrentResult().isPaused).toBe(false);
    onlineManager.setOnline(true);
    await new Promise((r) => setTimeout(r, 10));
    expect(fetcher).toHaveBeenCalledOnce();
  });

  it("staleTime を D5 の表から入れる", () => {
    const client = createQueryClient();
    expect(client.getQueryDefaults(taskKeys.detail("t1")).staleTime).toBe(5_000);
    expect(client.getQueryDefaults(taskKeys.timeline("t1")).staleTime).toBe(1_000);
    expect(client.getQueryDefaults(["projects", "docs", "p1", "a.md"]).staleTime).toBe(60_000);
    expect(client.getQueryDefaults(["daemon", "stream"]).staleTime).toBe(0);
  });
});

describe("session の終了", () => {
  it("session に 1 つの QueryClient を使い、終了後は新しい client にする", () => {
    const first = getSessionQueryClient();
    expect(getSessionQueryClient()).toBe(first);
    endQuerySession();
    expect(getSessionQueryClient()).not.toBe(first);
  });

  it("session 失効（clearProtectedCaches）で cache を捨て、進行中の fetch を止める", async () => {
    const unbind = bindQueryClientToSession();
    const seen: AbortSignal[] = [];
    configureApiClient({
      fetcher: (input, init) => {
        if (String(input) === "/api/tasks/t1") return Promise.resolve(json(200, { id: "t1" }));
        return new Promise<Response>((_resolve, reject) => {
          const signal = init?.signal as AbortSignal;
          seen.push(signal);
          signal.addEventListener("abort", () => reject(signal.reason), { once: true });
        });
      },
    });
    const client = getSessionQueryClient();
    await client.fetchQuery({
      queryKey: taskKeys.detail("t1"),
      queryFn: ({ signal }) => apiGet("/api/tasks/t1", signal),
    });
    const observer = new QueryObserver(client, {
      queryKey: taskKeys.list(),
      queryFn: ({ signal }) => apiGet("/api/tasks", signal),
    });
    const unsubscribe = observer.subscribe(() => {});
    const mutation = new MutationObserver(client, { mutationFn: () => apiMutate("POST", "/api/tasks/t1/cancel") });
    const mutating = mutation.mutate().catch((e: unknown) => e);
    await vi.waitFor(() => expect(seen).toHaveLength(2));
    expect(client.getQueryCache().getAll()).toHaveLength(2);
    expect(client.getMutationCache().getAll()).toHaveLength(1);

    clearProtectedCaches();

    expect(seen.every((s) => s.aborted)).toBe(true);
    expect(await mutating).toMatchObject({ kind: "aborted" });
    expect(client.getQueryCache().getAll()).toHaveLength(0);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
    expect(getSessionQueryClient()).not.toBe(client);
    unsubscribe();
    unbind();
  });

  it("401（session 失効）で失効の処理が走り、その query も止めて cache が空になる", async () => {
    const unbind = bindQueryClientToSession();
    configureApiClient({
      fetcher: async (input) => (String(input) === "/api/tasks/t1" ? json(200, { id: "t1" }) : json(401, {})),
      onUnauthorized: clearProtectedCaches,
    });
    const client = getSessionQueryClient();
    await client.fetchQuery({
      queryKey: taskKeys.detail("t1"),
      queryFn: ({ signal }) => apiGet("/api/tasks/t1", signal),
    });
    await expect(
      client.fetchQuery({ queryKey: taskKeys.list(), queryFn: ({ signal }) => apiGet("/api/tasks", signal) }),
    ).rejects.toBeDefined();
    expect(client.getQueryCache().getAll()).toHaveLength(0);
    expect(client.getQueryCache().find({ queryKey: taskKeys.detail("t1") })).toBeUndefined();
    unbind();
  });
});
