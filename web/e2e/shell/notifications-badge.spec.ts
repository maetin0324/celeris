import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, noticesFixture } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// nav の通知の未読数は GET /notifications/unread-count の unread（未読の束数）。SSE の notifications_changed で
// 15 s の poll を待たずに更新する。偽 daemon と gateway は loopback の空き port。
test("nav の通知の未読数は /notifications/unread-count から出て、notifications_changed で更新される", async ({
  page,
}) => {
  const dir = makeTmpDir("celeris-notifications-badge-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/tasks`);
    const nav = page.getByRole("navigation", { name: "主要", exact: true });
    await expect(nav.getByRole("link", { name: /通知/ })).toHaveAttribute("href", "/notifications");
    await expect(nav.getByRole("img", { name: "未読の通知 3 件" })).toBeVisible();
    expect(daemon.requests.some((request) => request.path === "/api/v1/notifications/unread-count")).toBe(true);
    await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);

    const notices = noticesFixture();
    notices[0].read_at = "2026-10-04T00:00:00Z";
    daemon.setNotices(notices);
    await expect(nav.getByRole("img", { name: "未読の通知 2 件" })).toBeVisible({ timeout: 5_000 });
    daemon.setNotices(notices.map((notice) => ({ ...notice, read_at: notice.read_at ?? "2026-10-04T00:00:00Z" })));
    await expect(nav.locator('a[href="/notifications"] [data-badge]')).toHaveCount(0, { timeout: 5_000 });
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
