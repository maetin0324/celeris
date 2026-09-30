import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { v3Screens } from "../support/screens";

for (const screen of v3Screens()) {
  test(`S2 ${screen.path}: unrelated SSE does not refetch screen queries`, async ({ page }) => {
    const dir = mkdtempSync(path.join(tmpdir(), "celeris-v3-refetch-"));
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
    const daemonUrl = await daemon.start();
    const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    try {
      await page.goto(`${gateway.base}${screen.fixture}`);
      await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
      await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
      // The Phase 2 shell owns an active inbox query on both routes. Task lists are
      // still placeholders, so counting /tasks alone would make this gate vacuous.
      const count = (route: string) => daemon.requests.filter((request) => request.path === route).length;
      await expect.poll(() => count("/api/v1/inbox")).toBeGreaterThan(0);
      const inboxBefore = count("/api/v1/inbox");
      const tasksBefore = count("/api/v1/tasks");
      for (let i = 1; i <= 20; i++)
        daemon.sendEvent("task.event", {
          cursor: i,
          event: { id: i, task_id: "unrelated", event: { type: "worker_progress" } },
        });
      for (let i = 0; i < 10; i++) {
        daemon.sendEvent("daemon", { cursor: i + 21 });
        await page.waitForTimeout(2000);
      }
      // One or two inbox refreshes may be its 15 s fallback poll. SSE must not
      // turn the 2 s daemon ticks or unrelated progress events into refetches.
      expect(count("/api/v1/inbox") - inboxBefore).toBeLessThanOrEqual(2);
      expect(count("/api/v1/tasks") - tasksBefore).toBe(0);
      await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
