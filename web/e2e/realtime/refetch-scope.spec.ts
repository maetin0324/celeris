import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { recordLatency } from "../support/latency-results";
import { installRealtimeProbe, readRealtimeProbe, waitFetchesSettled, waitHandled } from "../support/realtime-probe";
import { v3Screens } from "../support/screens";

// S2: 関係のない SSE で画面の query を取り直さない（nfr。全画面の掃引）。
// 固定時間は待たない（agent-docs/guides/testing.md の方法 1・2）。
// - page の時計は Playwright の clock に差し替え、250 ms の束ね窓・15 s の受信箱 poll・2 s の daemon tick を
//   runFor で進める（実時間 20 s の待ちを仮想時間 20 s に置き換える）。
// - 「app が SSE を処理し終えた」は試験側の観測点（support/realtime-probe.ts）の処理済み件数で待つ。
// - 画面が出て取得が落ち着いたら時計を止める。以後 timer は runFor で進めた分だけ発火する。
// - 最後に関係のある event（受信箱を取り直す created）を目印に流し、同じ経路（frame → 束ね窓 → invalidate →
//   fetch）で取得が呼ばれることを確かめる（観測が空振りしていない証拠）。目印を処理し終えた時点までの
//   記録に画面の取得が 0 本なら、関係のない SSE は取り直しを起こしていない。
const STREAM_ONLY = ["/api/v1/stream", "/api/v1/health", "/api/v1/inbox", "/api/v1/daemon"];
const CANARY_TASK = "e2e-canary";

for (const screen of v3Screens()) {
  test(`S2 ${screen.path}: unrelated SSE does not refetch screen queries`, async ({ page }) => {
    const dir = mkdtempSync(path.join(tmpdir(), "celeris-v3-refetch-"));
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
    const daemonUrl = await daemon.start();
    const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    try {
      await installRealtimeProbe(page);
      await page.clock.install();
      await page.goto(`${gateway.base}${screen.fixture}`);
      await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
      await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
      // The Phase 2 shell owns an active inbox query on both routes. Task lists are
      // still placeholders, so counting /tasks alone would make this gate vacuous.
      const count = (route: string) => daemon.requests.filter((request) => request.path === route).length;
      await expect.poll(() => count("/api/v1/inbox")).toBeGreaterThan(0);
      await waitFetchesSettled(page);
      // ここから page の時計を止める。束ね窓・poll の timer は runFor で進めたときだけ発火する。
      await page.clock.pauseAt((await page.evaluate(() => Date.now())) + 1000);
      await waitFetchesSettled(page);
      const before = await readRealtimeProbe(page);
      const screenPaths = new Set(
        before.fetches.filter((route) => route.startsWith("/api/v1/") && !STREAM_ONLY.includes(route)),
      );

      for (let i = 1; i <= 20; i++)
        daemon.sendEvent("task.event", {
          id: i,
          seq: i,
          task_id: "unrelated",
          ts: "2026-09-30T00:00:00Z",
          event: { type: "worker_progress" },
        });
      await waitHandled(page, "task.event", (before.handled["task.event"] ?? 0) + 20);
      // 10 回の daemon tick と 2 s ずつの仮想時間（計 20 s。受信箱の 15 s poll を 1 回含む）。
      for (let i = 0; i < 10; i++) {
        const ticks = (await readRealtimeProbe(page)).handled.daemon ?? 0;
        daemon.sendEvent("daemon", { cursor: i + 21 });
        await waitHandled(page, "daemon", ticks + 1);
        await page.clock.runFor(2000);
      }
      await waitFetchesSettled(page);

      // 目印: 受信箱（全画面で active）を取り直す関係のある event。時計が止まっているので、
      // 目印を処理し終えた時点までの記録は関係のない SSE と 20 s の仮想時間の分だけ。
      daemon.sendEvent("task.event", {
        id: 21,
        seq: 1,
        task_id: CANARY_TASK,
        ts: "2026-09-30T00:00:00Z",
        event: { type: "created" },
      });
      await waitHandled(page, "task.event", (before.handled["task.event"] ?? 0) + 21);
      const marker = (await readRealtimeProbe(page)).fetches.length;
      await page.clock.runFor(1000);
      await expect
        .poll(async () => (await readRealtimeProbe(page)).fetches.slice(marker).includes("/api/v1/inbox"), {
          message: "canary event refetches the inbox",
        })
        .toBe(true);
      await waitFetchesSettled(page);
      const after = await readRealtimeProbe(page);
      const unrelated = after.fetches.slice(before.fetches.length, marker);
      const refetched = (route: string) => unrelated.filter((item) => item === route).length;

      // One or two inbox refreshes may be its 15 s fallback poll. SSE must not
      // turn the 2 s daemon ticks or unrelated progress events into refetches.
      expect(refetched("/api/v1/inbox")).toBeLessThanOrEqual(2);
      expect(refetched("/api/v1/tasks")).toBe(0);
      const screenRefetches = Object.fromEntries([...screenPaths].map((route) => [route, refetched(route)]));
      recordLatency({
        kind: "S2",
        path: screen.path,
        fixture: screen.fixture,
        unrelatedRefetches: screenRefetches,
        inboxPolls: refetched("/api/v1/inbox"),
        healthPolls: count("/api/v1/health"),
        daemonRestPolls: count("/api/v1/daemon"),
      });
      for (const [route, refetches] of Object.entries(screenRefetches))
        expect(refetches, `${screen.path}: unrelated SSE refetched ${route}`).toBe(0);
      await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
