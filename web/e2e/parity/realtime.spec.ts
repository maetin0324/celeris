/// <reference types="node" />
// api/ を import するので tsconfig.app.json で型検査する（node の型は上の参照で足す）。
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { hashKey, QueryClient, type QueryKey, QueryObserver } from "@tanstack/react-query";
import { daemonKeys, projectKeys, taskKeys } from "../../api/queries/keys";
import { createConnectionStore } from "../../api/realtime/connection-state";
import { createRealtime, type Realtime } from "../../api/realtime/realtime";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { NodeEventSource } from "../support/node-event-source";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

// realtime（P2-04・P2-05）。実 gateway と偽 daemon（loopback の空き port）の間で transport と invalidate を動かす。
// 切断は gateway の接続を実際に切って起こす。外部ネットワークには出ない。
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-realtime-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });

let server: http.Server;
let base: string;

test.beforeAll(async () => {
  const daemonUrl = await daemon.start();
  server = createApp({ daemonUrl, daemonTokenFile: tokenFile, log: () => {} }).listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
  base = `http://127.0.0.1:${address.port}`;
});

test.afterAll(async () => {
  server.closeAllConnections();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

const row = (id: number, type: string, extra: Record<string, unknown> = {}) => ({
  id,
  seq: id,
  task_id: "T1",
  ts: "2026-09-30T00:00:00Z",
  event: { type, ...extra },
});

/** gateway 経由で daemon の JSON を取る query。key ごとの取得回数と同時実行数を数える。 */
function harness() {
  const calls = new Map<string, number>();
  const inflight = new Map<string, number>();
  const peak = new Map<string, number>();
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const queryFn = async ({ queryKey }: { queryKey: QueryKey }) => {
    const id = hashKey(queryKey);
    calls.set(id, (calls.get(id) ?? 0) + 1);
    inflight.set(id, (inflight.get(id) ?? 0) + 1);
    peak.set(id, Math.max(peak.get(id) ?? 0, inflight.get(id) ?? 0));
    try {
      const res = await fetch(`${base}/api/health`);
      return res.status;
    } finally {
      inflight.set(id, (inflight.get(id) ?? 1) - 1);
    }
  };
  const observers = [taskKeys.detail("T1"), taskKeys.timeline("T1"), taskKeys.list(), projectKeys.list()].map(
    (queryKey) => new QueryObserver(queryClient, { queryKey, queryFn }),
  );
  const unsubscribe = observers.map((o) => o.subscribe(() => {}));
  const count = (key: QueryKey) => calls.get(hashKey(key)) ?? 0;
  const maxInflight = (key: QueryKey) => peak.get(hashKey(key)) ?? 0;
  return {
    queryClient,
    queryFn,
    count,
    maxInflight,
    reset() {
      calls.clear();
      peak.clear();
    },
    dispose() {
      for (const u of unsubscribe) u();
      queryClient.clear();
    },
  };
}

function start(
  queryClient: QueryClient,
  backoffBaseMs = 50,
): { realtime: Realtime; store: ReturnType<typeof createConnectionStore> } {
  const store = createConnectionStore();
  const realtime = createRealtime({
    queryClient,
    store,
    url: `${base}/events`,
    createEventSource: (url) => new NodeEventSource(url),
    probe: async () => {
      const res = await fetch(`${base}/events`);
      await res.body?.cancel();
      return res.status === 401 ? "unauthorized" : "ok";
    },
    backoffBaseMs,
    backoffMaxMs: 60_000,
  });
  realtime.start();
  return { realtime, store };
}

const streamRequests = () => daemon.requests.filter((r) => r.path === "/api/v1/stream");

test("parity: /events 中継・再接続・Last-Event-ID・reset", async () => {
  test.setTimeout(60_000);
  const h = harness();
  await expect.poll(() => h.count(taskKeys.detail("T1"))).toBe(1);
  h.queryClient.setQueryData(daemonKeys.rest(), { source: "rest" });
  const { realtime, store } = start(h.queryClient);
  await expect.poll(() => store.get()).toBe("open");
  await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);

  // hello・heartbeat は invalidate しない。daemon は ['daemon','stream'] だけ。
  h.reset();
  daemon.sendEvent("hello", { cursor: 10, now: "2026-09-30T00:00:00Z", daemon: { source: "hello" } });
  daemon.sendEvent("heartbeat", {});
  daemon.sendEvent("daemon", { source: "stream" });
  await expect.poll(() => h.queryClient.getQueryData(daemonKeys.stream())).toEqual({ source: "stream" });
  expect(h.queryClient.getQueryData(daemonKeys.rest())).toEqual({ source: "rest" });
  expect(realtime.cursor()).toBe(10);

  // task.event は cursor を進め、不正 payload は捨てる。
  daemon.sendEvent("task.event", row(11, "transitioned"));
  daemon.sendEvent("task.event", { id: 99, event: { type: "no_such_kind" } });
  await expect.poll(() => realtime.cursor()).toBe(11);
  await expect.poll(() => h.count(taskKeys.detail("T1"))).toBe(1);

  // 実際に接続を切る → cursor を after_id に載せて張り直す（gateway は Last-Event-ID・after_id を daemon へ転送する）。
  const before = streamRequests().length;
  server.closeAllConnections();
  await expect.poll(() => streamRequests().some((r, i) => i >= before && r.query === "?after_id=11")).toBe(true);
  await expect.poll(() => store.get()).toBe("open");
  const resumed = new Promise<void>((resolve) => {
    // 張り直し後の購読が frame を受けるまで待つ
    const timer = setInterval(() => {
      if (realtime.cursor() === 12) {
        clearInterval(timer);
        resolve();
      }
    }, 20);
  });
  await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
  daemon.sendEvent("task.event", row(12, "worker_progress"));
  await resumed;

  // reset: 全 server query を stale に、active だけ取り直す。['daemon','stream'] は残す。
  h.reset();
  const inactive: QueryKey = ["tasks", "detail", "T-inactive"];
  h.queryClient.setQueryData(inactive, 1);
  daemon.sendEvent("reset", { cursor: 30, reason: "lagged" });
  await expect.poll(() => realtime.cursor()).toBe(30);
  await expect.poll(() => h.count(taskKeys.detail("T1"))).toBe(1);
  await expect.poll(() => h.count(projectKeys.list())).toBe(1);
  expect(h.queryClient.getQueryState(inactive)?.isInvalidated).toBe(true);
  expect(h.count(inactive)).toBe(0);
  expect(h.queryClient.getQueryData(daemonKeys.stream())).toEqual({ source: "stream" });

  // 401 で再接続を止める。
  daemon.setStreamStatus(401);
  server.closeAllConnections();
  await expect.poll(() => store.get()).toBe("unauthorized");
  const afterDenied = streamRequests().length;
  await new Promise((r) => setTimeout(r, 1_000));
  expect(streamRequests().length).toBe(afterDenied);
  daemon.setStreamStatus(200);

  realtime.dispose();
  h.dispose();
});

test("parity-x: SSE 再接続・reset・復帰・burst", async () => {
  test.setTimeout(90_000);
  const h = harness();
  await expect.poll(() => h.count(taskKeys.list())).toBe(1);
  const { realtime, store } = start(h.queryClient, 60_000);
  await expect.poll(() => store.get()).toBe("open");
  await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
  daemon.sendEvent("hello", { cursor: 100, now: "2026-09-30T00:00:00Z" });
  await expect.poll(() => realtime.cursor()).toBe(100);

  // worker_progress は project・一覧を取り直さない。
  h.reset();
  daemon.sendEvent("task.event", row(101, "worker_progress"));
  await expect.poll(() => h.count(taskKeys.timeline("T1"))).toBe(1);
  await new Promise((r) => setTimeout(r, 600));
  expect(h.count(taskKeys.detail("T1"))).toBe(0);
  expect(h.count(taskKeys.list())).toBe(0);
  expect(h.count(projectKeys.list())).toBe(0);

  // 1 s に 20 件の burst（重複・逆順を含む）+ daemon の応答 10 s 遅延: key ごとの進行中取得は 1 本。
  h.reset();
  daemon.setDelay(10_000);
  for (let i = 0; i < 20; i++) {
    const id = i === 10 ? 105 : i === 11 ? 105 : 102 + i; // 重複と逆順
    daemon.sendEvent("task.event", row(id, "transitioned", { from: "running", to: "done" }));
    await new Promise((r) => setTimeout(r, 50));
  }
  await expect.poll(() => realtime.cursor()).toBe(121);
  await new Promise((r) => setTimeout(r, 24_000));
  for (const key of [taskKeys.detail("T1"), taskKeys.list()]) {
    expect(h.maxInflight(key), JSON.stringify(key)).toBe(1);
    expect(h.count(key), JSON.stringify(key)).toBeGreaterThanOrEqual(1);
    expect(h.count(key), JSON.stringify(key)).toBeLessThanOrEqual(2);
  }
  daemon.setDelay(0);

  // 復帰: 接続中は stale で active な query だけ取り直す。
  h.reset();
  await h.queryClient.invalidateQueries({ queryKey: taskKeys.all, refetchType: "none" });
  realtime.resume();
  await expect.poll(() => h.count(taskKeys.detail("T1"))).toBe(1);
  await expect.poll(() => h.count(taskKeys.list())).toBe(1);
  expect(h.count(projectKeys.list())).toBe(0);

  // 切断後の復帰は backoff（60 s）を待たずに張り直す。
  const before = streamRequests().length;
  server.closeAllConnections();
  await expect.poll(() => store.get()).toBe("reconnecting");
  await new Promise((r) => setTimeout(r, 500));
  realtime.resume();
  await expect
    .poll(
      () =>
        streamRequests()
          .slice(before)
          .some((r) => r.query === "?after_id=121"),
      { timeout: 5_000 },
    )
    .toBe(true);
  await expect.poll(() => store.get()).toBe("open");

  realtime.dispose();
  h.dispose();
});
