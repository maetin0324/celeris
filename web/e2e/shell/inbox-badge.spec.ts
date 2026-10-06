import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, inboxItemsFixture } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// nav の受信箱の件数は GET /inbox/items の counts.total（ADR-0133）。SSE の inbox_changed で 15 s の
// poll を待たずに更新する。偽 daemon と gateway は loopback の空き port。
test("nav の受信箱件数は /inbox/items の counts から出て、inbox_changed で更新される", async ({ page }) => {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-inbox-badge-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/tasks`);
    const nav = page.getByRole("navigation", { name: "主要", exact: true });
    await expect(nav.getByRole("img", { name: "受信箱 5 件" })).toBeVisible();
    expect(daemon.requests.some((request) => request.path === "/api/v1/inbox/items")).toBe(true);
    await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);

    daemon.setInboxItems(inboxItemsFixture().slice(1));
    await expect(nav.getByRole("img", { name: "受信箱 4 件" })).toBeVisible({ timeout: 5_000 });
    daemon.setInboxItems([]);
    await expect(nav.locator('a[href="/inbox"] [data-badge]')).toHaveCount(0, { timeout: 5_000 });
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
