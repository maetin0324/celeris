import { connect } from "node:net";
import { expect, it } from "vitest";
import { startFakeDashboard } from "./browser-gateway";
import { createFakeDaemon } from "./fake-daemon.mjs";

function exchange(base: string, raw: string): Promise<void> {
  const url = new URL(base);
  return new Promise((resolve, reject) => {
    const socket = connect(Number(url.port), url.hostname, () => socket.end(raw));
    socket.setTimeout(5000, () => socket.destroy(new Error("fixture request timed out")));
    socket.on("data", () => {});
    socket.on("error", reject);
    socket.on("close", () => resolve());
  });
}

it("fake daemon ignores empty, foreign and incomplete requests before a real API request", async () => {
  const daemon = createFakeDaemon({ fixtures: { "/api/v1/tasks": { items: [] } } });
  const base = await daemon.start();
  try {
    for (const raw of [
      "",
      "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
      "POST /api/v1/tasks HTTP/1.1\r\nContent-Length: 8\r\n\r\nx",
    ]) {
      await exchange(base, raw);
    }
    expect(daemon.requests).toHaveLength(0);
    const response = await fetch(`${base}/api/v1/tasks`);
    expect(response.status).toBe(200);
    await response.text();
    expect(daemon.requests).toHaveLength(1);
  } finally {
    await daemon.close();
  }
});

// 迷い込み接続の filter は試験の制御の endpoint を落とさない。/__fake/releases が 404 だと parity/ops.spec.ts の
// 昇格の台本（失敗の mode・pending の時間）が設定されず、失敗の表示の試験が成功の表示で進んでしまう。
it("fake daemon lets the /__fake/ control endpoint through the probe filter", async () => {
  const daemon = createFakeDaemon({});
  const base = await daemon.start();
  try {
    const set = await fetch(`${base}/__fake/releases`, {
      method: "POST",
      body: JSON.stringify({ mode: "fail", pendingMs: 1200 }),
    });
    expect(set.status).toBe(200);
    expect(await set.json()).toEqual({ mode: "fail", pendingMs: 1200 });
    const promote = await fetch(`${base}/api/v1/releases/bbbbbbbbbbbb/promote`, { method: "POST" });
    expect(promote.status).toBe(202);
    await promote.text();
    const list = await fetch(`${base}/api/v1/releases`);
    const body = JSON.stringify(await list.json());
    expect(body).toContain('"promoting":true');
  } finally {
    await daemon.close();
  }
});

it("fake dashboard ignores host probes and still records gateway traffic", async () => {
  const dashboard = await startFakeDashboard();
  const base = `http://${dashboard.authority}`;
  try {
    for (const raw of [
      "",
      "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
      "GET /api/sessions HTTP/1.1\r\nOrigin: http://fixture\r\nContent-Length: 8\r\n\r\nx",
    ]) {
      await exchange(base, raw);
    }
    const missing = await fetch(`${base}/unknown`, { headers: { Origin: base } });
    expect(missing.status).toBe(404);
    await missing.text();
    expect(dashboard.requests).toHaveLength(0);
    const response = await fetch(base, { headers: { Origin: base } });
    expect(response.status).toBe(200);
    await response.text();
    expect(dashboard.requests).toHaveLength(1);
  } finally {
    await dashboard.close();
  }
});
