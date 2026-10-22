import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  abortSessionRequests,
  apiFetch,
  apiMutate,
  apiPathFromDaemon,
  configureApiClient,
  kindForStatus,
} from "./client";

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

  it("daemon の前置 /api/v1 を付けた path は二重前置として投げる（gateway が前置する）", async () => {
    const fetcher = vi.fn();
    configureApiClient({ fetcher });
    for (const path of ["/api/v1/config/fields", "/api/v1", "/api/v1?x=1", "/api/v1/routing/stages"]) {
      await expect(apiFetch(path)).rejects.toThrow(/daemon prefix/);
      await expect(apiMutate("PATCH", path, {})).rejects.toThrow(TypeError);
    }
    expect(fetcher).not.toHaveBeenCalled();
    // 前置の無い同名の segment は通す。
    fetcher.mockImplementation(async () => json(200, {}));
    await expect(apiFetch("/api/v1x")).resolves.toEqual({});
    await expect(apiFetch("/api/tasks/v1")).resolves.toEqual({});
  });

  it("daemon が返す /api/v1/x を client の /api/x に直す", () => {
    expect(apiPathFromDaemon("/api/v1/tasks/T%201/integrations/web/merge")).toBe(
      "/api/tasks/T%201/integrations/web/merge",
    );
    expect(apiPathFromDaemon("/api/v1/inbox/items?kind=decision")).toBe("/api/inbox/items?kind=decision");
    expect(apiPathFromDaemon("/api/tasks/T1")).toBe("/api/tasks/T1");
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

// web の client code（試験・生成物・e2e 以外）に daemon の `/api/v1` を前置した文字列 literal が無いこと。
// gateway が `/api/x` を `/api/v1/x` へ前置するので、client が `/api/v1/...` を書くと 404 になる（2026-10-22 の設定・routing 画面）。
describe("source scan", () => {
  const root = join(import.meta.dirname, "..");
  const sources = (dir: string): string[] =>
    readdirSync(join(root, dir), { withFileTypes: true }).flatMap((entry) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return entry.name === "generated" || entry.name === "fixtures" ? [] : sources(path);
      return /\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name) ? [path] : [];
    });

  it("client code does not hard-code the daemon /api/v1 prefix", () => {
    const files = ["api", "features", "components", "lib", "routes"].flatMap(sources);
    expect(files.length).toBeGreaterThan(50);
    const offenders = files.flatMap((file) =>
      readFileSync(join(root, file), "utf8")
        .split("\n")
        .flatMap((line, i) => (/["'`]\/api\/v1(\/|["'`?])/.test(line) ? [`${file}:${i + 1}`] : [])),
    );
    expect(offenders).toEqual([]);
  });
});
