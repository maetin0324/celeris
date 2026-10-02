import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, abortSessionRequests, apiFetch, apiMutate, configureApiClient, kindForStatus } from "./client";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

/** signal が止まるまで返らない fetch。 */
function hangingFetch(seen: AbortSignal[] = []): typeof fetch {
  return (_input, init) =>
    new Promise<Response>((_resolve, reject) => {
      const signal = init?.signal as AbortSignal;
      seen.push(signal);
      signal.addEventListener("abort", () => reject(signal.reason), { once: true });
    });
}

afterEach(() => {
  vi.useRealTimers();
  configureApiClient({ fetcher: (i, init) => fetch(i, init), onUnauthorized: () => {} });
});

describe("apiFetch", () => {
  it("same-origin の /api/ 以外は呼ばない", async () => {
    const fetcher = vi.fn();
    configureApiClient({ fetcher });
    for (const path of ["https://evil.example/api/x", "//evil.example/api/x", "/health", "/api/../login"]) {
      await expect(apiFetch(path)).rejects.toThrow(TypeError);
    }
    expect(fetcher).not.toHaveBeenCalled();
  });

  it("same-origin の資格情報で JSON を取る", async () => {
    const fetcher = vi.fn<typeof fetch>(async () => json(200, { ok: 1 }));
    configureApiClient({ fetcher });
    await expect(apiFetch("/api/tasks")).resolves.toEqual({ ok: 1 });
    const init = fetcher.mock.calls[0]?.[1];
    expect(init?.credentials).toBe("same-origin");
    expect(init?.method).toBe("GET");
  });

  it("HTTP の状態をエラーの種類に分ける", () => {
    expect(kindForStatus(401)).toBe("unauthorized");
    expect(kindForStatus(403)).toBe("forbidden");
    expect(kindForStatus(404)).toBe("not_found");
    expect(kindForStatus(409)).toBe("conflict");
    expect(kindForStatus(400)).toBe("validation");
    expect(kindForStatus(422)).toBe("validation");
    expect(kindForStatus(429)).toBe("rate_limited");
    expect(kindForStatus(503)).toBe("server");
    expect(kindForStatus(418)).toBe("http");
  });

  it("422 の本文をエラーに残す", async () => {
    configureApiClient({ fetcher: async () => json(422, { error: "title is required" }) });
    const error = await apiMutate("POST", "/api/tasks", { title: "" }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).kind).toBe("validation");
    expect((error as ApiError).body).toEqual({ error: "title is required" });
  });

  it("401 で session 失効の処理を呼ぶ", async () => {
    const onUnauthorized = vi.fn();
    configureApiClient({ fetcher: async () => json(401, {}), onUnauthorized });
    await expect(apiFetch("/api/tasks")).rejects.toMatchObject({ kind: "unauthorized", status: 401 });
    expect(onUnauthorized).toHaveBeenCalledOnce();
  });

  it("15 s で timeout にする", async () => {
    vi.useFakeTimers();
    const seen: AbortSignal[] = [];
    configureApiClient({ fetcher: hangingFetch(seen) });
    const pending = apiFetch("/api/tasks").catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(14_999);
    expect(seen[0]?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await pending).toMatchObject({ kind: "timeout" });
  });

  it("呼び出し側の AbortSignal で止まる", async () => {
    configureApiClient({ fetcher: hangingFetch() });
    const controller = new AbortController();
    const pending = apiFetch("/api/tasks", { signal: controller.signal }).catch((e: unknown) => e);
    controller.abort();
    expect(await pending).toMatchObject({ kind: "aborted" });
  });

  it("session の終了で進行中の要求が止まる", async () => {
    const seen: AbortSignal[] = [];
    configureApiClient({ fetcher: hangingFetch(seen) });
    const get = apiFetch("/api/tasks").catch((e: unknown) => e);
    const post = apiMutate("POST", "/api/tasks/t1/cancel").catch((e: unknown) => e);
    abortSessionRequests();
    expect(await get).toMatchObject({ kind: "aborted" });
    expect(await post).toMatchObject({ kind: "aborted" });
    expect(seen.every((s) => s.aborted)).toBe(true);
  });

  it("client 自身は再試行しない（変更系も GET も 1 回だけ送る）", async () => {
    const fetcher = vi.fn<typeof fetch>(async () => json(503, {}));
    configureApiClient({ fetcher });
    await expect(apiMutate("POST", "/api/tasks", {})).rejects.toMatchObject({ kind: "server" });
    await expect(apiFetch("/api/tasks")).rejects.toMatchObject({ kind: "server" });
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
});
