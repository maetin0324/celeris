import { expect } from "@playwright/test";

// goto の直後は SPA の起動（session 確認 → shell と画面の描画 → data の到着と再描画）の long task が
// main thread を塞ぎ、最初の click の locator 解決がそれを待つ。遷移の latency を測る試験は、
// click の前にこの helper で起動の完了を出来事で待つ（固定 sleep は使わない。SSE が開いたままで
// JSON は遅延させるので networkidle も使えない）。
//
// 完了の条件: nav と main h1 が出た後、「2 frame + requestIdleCallback」の 1 巡の間に long task が
// 1 つも終わらない状態。巡の途中で long task が見えたら次の巡でやり直す。
// 本文は文字列で page に渡す。型は boot-idle.d.mts（tsconfig.app の file 一覧に足さずに
// parity/latency-gate.spec.ts から読めるよう、support/fake-daemon.mjs と同じ形にする）。
const QUIET_ROUND = `(async (timeoutMs) => {
  const deadline = performance.now() + timeoutMs;
  let lastEnd = 0;
  const take = (list) => {
    for (const e of list) lastEnd = Math.max(lastEnd, e.startTime + e.duration);
  };
  const observer = new PerformanceObserver((list) => take(list.getEntries()));
  observer.observe({ type: "longtask", buffered: true });
  const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve()));
  const idle = () => new Promise((resolve) => requestIdleCallback(resolve, { timeout: 1000 }));
  try {
    for (let rounds = 1; ; rounds++) {
      const since = performance.now();
      await frame();
      await frame();
      const deadlineInfo = await idle();
      take(observer.takeRecords());
      if (!deadlineInfo.didTimeout && lastEnd < since) return rounds;
      if (performance.now() > deadline) throw new Error("boot did not become idle");
    }
  } finally {
    observer.disconnect();
  }
})`;

/** @param {import("@playwright/test").Page} page */
export async function waitForBootIdle(page, timeoutMs = 15_000) {
  await expect(page.getByRole("navigation", { name: "主要" })).toBeAttached({ timeout: timeoutMs });
  await expect(page.locator("main h1")).toBeVisible({ timeout: timeoutMs });
  return page.evaluate(`${QUIET_ROUND}(${timeoutMs})`);
}
