import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";
import { BROWSER_RAW_LIVE_VIEW_URL } from "../support/fake-daemon.mjs";

// 偽 browser backend と browser-gateway の helper の煙試験。本人（owner）は /browser/runs を読め、
// 本人でない session は 403 not_owner。raw の live_view_url はどの応答にも出ない（ADR D2.2・D2.4）。

test("owner reads /browser/runs; a non-owner session gets 403 not_owner", async ({ page, browser }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  const other = await browser.newContext();
  try {
    const anonymous = await page.request.get(`${gateway.base}/browser/runs`);
    expect(anonymous.status()).toBe(401);

    const { csrf } = await gateway.loginAsOwner(page);
    expect(csrf).toMatch(/^[0-9a-f]{64}$/);
    const owned = await page.request.get(`${gateway.base}/browser/runs`);
    expect(owned.status()).toBe(200);
    const text = await owned.text();
    expect(text).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    const { items } = JSON.parse(text) as {
      items: Array<{ task_id: string; run_id: string; state: string; live_path: string; live_view_url: null }>;
    };
    expect(items.map((r) => `${r.task_id}/${r.run_id}:${r.state}`)).toEqual([
      "T1/R0:COMPLETED",
      "T1/R1:RUNNING",
      "T2/R2:RUNNING",
    ]);
    expect(items.find((r) => r.run_id === "R1")).toMatchObject({
      live_path: "/browser/live/T1/R1",
      live_view_url: null,
    });

    // 本人の Live View は偽 dashboard へ固定の loopback Host で中継される。
    const entry = await page.request.get(`${gateway.base}/browser/live/T1/R1`);
    expect(entry.status()).toBe(200);
    expect(await entry.text()).toContain("偽 dashboard");
    expect(gateway.dashboard.requests.at(-1)).toMatchObject({ path: "/", host: gateway.dashboard.authority });

    const otherPage = await other.newPage();
    await gateway.loginAsOther(otherPage);
    for (const route of ["/browser/runs", "/browser/runs?task_id=T1", "/browser/live/T1/R1"]) {
      const denied = await otherPage.request.get(`${gateway.base}${route}`);
      expect(denied.status(), route).toBe(403);
      expect(await denied.json(), route).toEqual({ code: "not_owner" });
    }
  } finally {
    await other.close();
    await gateway.close();
  }
});
