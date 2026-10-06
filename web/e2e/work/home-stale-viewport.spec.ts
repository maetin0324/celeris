/// <reference lib="dom" />
import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";
import { applyStateRoute, stateByKey } from "../support/states";

// fix-r7 home: stale の /console（stream が 503 を返し続ける）を電話幅で開いた初期 viewport（scroll 前）で、
// 固定の送信欄の上端より上に会話本文が 150px 以上見える（narrow-r6.spec の (6) と同じ基準を scroll 前で測る）。
// stale では接続状態が「確認中」と「再接続中」を行き来する。その切替で document の高さが変わらない。

async function settle(page: Page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

for (const width of [360, 390, 412]) {
  test(`stale の /console ${width}: scroll 前の viewport で送信欄の上に会話本文が 150px 以上見え、接続状態の切替で高さが跳ねない`, async ({
    page,
  }) => {
    const stale = stateByKey("stale");
    const gateway = await startFixtureGateway(stale.daemon);
    try {
      await page.setViewportSize({ width, height: 800 });
      await applyStateRoute(page, stale);
      await page.goto(`${gateway.base}/console`);
      await expect(page.locator("[data-connection]")).toHaveAttribute("data-connection", "reconnecting", {
        timeout: 15_000,
      });
      await expect(page.getByRole("status").filter({ hasText: "最新の状態は未確認です" })).toBeVisible();
      await expect(page.getByRole("list", { name: "Console の会話" }).getByRole("listitem").first()).toBeAttached();
      await settle(page);
      expect(await page.evaluate(() => window.scrollY), "scroll していない").toBe(0);
      // 会話の list の箱を、会話枠の見える範囲・宛先行の下端・送信欄の上端・viewport で切り取った高さ。
      const visible = await page.evaluate(() => {
        const region = document.querySelector("[data-home-console]") as HTMLElement;
        const toolbar = region.querySelector("[data-console-toolbar]") as HTMLElement;
        const composer = region.querySelector('[data-testid="console-composer"]') as HTMLElement;
        const list = region.querySelector('ol[aria-label="Console の会話"]') as HTMLElement;
        const box = list.getBoundingClientRect();
        const top = Math.max(box.top, region.getBoundingClientRect().top, toolbar.getBoundingClientRect().bottom, 0);
        const bottom = Math.min(
          box.bottom,
          region.getBoundingClientRect().bottom,
          composer.getBoundingClientRect().top,
          window.innerHeight,
        );
        return bottom - top;
      });
      expect(visible, "送信欄の上に見える会話本文の高さ").toBeGreaterThanOrEqual(150);
      // 接続状態の切替（確認中 → 再接続中）を出来事として待ち、その間の document の高さを記録する。
      const initial = await page.evaluate(() => {
        const w = window as unknown as { __r7: { state: string | null; height: number }[] };
        w.__r7 = [];
        const status = document.querySelector("[data-connection]") as HTMLElement;
        new MutationObserver(() => {
          w.__r7.push({
            state: status.getAttribute("data-connection"),
            height: document.documentElement.scrollHeight,
          });
        }).observe(status, { attributes: true, attributeFilter: ["data-connection"] });
        return document.documentElement.scrollHeight;
      });
      await page.waitForFunction(
        () => {
          const log = (window as unknown as { __r7: { state: string | null }[] }).__r7;
          const i = log.findIndex((entry) => entry.state === "connecting");
          return i >= 0 && log.slice(i + 1).some((entry) => entry.state === "reconnecting");
        },
        undefined,
        { timeout: 20_000 },
      );
      await settle(page);
      const log = await page.evaluate(
        () => (window as unknown as { __r7: { state: string | null; height: number }[] }).__r7,
      );
      for (const entry of log) {
        expect(entry.height, `接続状態 ${entry.state} でも document の高さが同じ`).toBe(initial);
      }
      expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBe(initial);
      expect(await page.evaluate(() => window.scrollY), "切替で scroll が動かない").toBe(0);
    } finally {
      await gateway.close();
    }
  });
}
