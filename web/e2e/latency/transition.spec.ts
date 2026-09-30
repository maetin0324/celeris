import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { v3Screens } from "../support/screens";

// baseline と同じくクリックを起点に URL、見出し、画面データ（現 Phase は準備中の枠）を別々に測る。
for (const screen of v3Screens()) {
  test(`S1 ${screen.path}: JSON 0/5/10s でも shell 遷移は独立`, async ({ page }) => {
    test.setTimeout(120_000);
    const dir = mkdtempSync(path.join(tmpdir(), "celeris-v3-latency-"));
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
    const daemonUrl = await daemon.start();
    const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    try {
      const measures: Array<{ delay: number; url: number; heading: number; data: number }> = [];
      for (const delay of [0, 5000, 10000]) {
        daemon.setDelay(delay);
        await page.goto(gateway.base);
        const nav = page.getByRole("navigation", { name: "主要" });
        await expect(nav).toBeAttached();
        const link = nav.getByRole("link", { name: screen.heading });
        const start = performance.now();
        if (await link.count()) await link.click();
        else await page.goto(`${gateway.base}${screen.fixture}`, { waitUntil: "commit" });
        await expect(page).toHaveURL(`${gateway.base}${screen.fixture}`);
        const url = performance.now() - start;
        const headingNode = page.getByRole("heading", { level: 1, name: screen.heading });
        await expect(headingNode).toBeVisible();
        const heading = performance.now() - start;
        await expect(headingNode.locator("..").locator(":scope > :not(h1)").first()).toBeVisible();
        const data = performance.now() - start;
        measures.push({ delay, url, heading, data });
        expect(url, `${screen.path} URL @${delay}`).toBeLessThanOrEqual(300);
        expect(heading, `${screen.path} heading @${delay}`).toBeLessThanOrEqual(300);
      }
      process.stdout.write(`S1 ${screen.path} ${JSON.stringify(measures)}\n`);
      expect(Math.abs(measures[2].url - measures[0].url), "URL 10s-0s").toBeLessThanOrEqual(100);
      expect(Math.abs(measures[2].heading - measures[0].heading), "heading 10s-0s").toBeLessThanOrEqual(100);
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
