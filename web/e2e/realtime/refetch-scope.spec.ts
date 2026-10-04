import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { recordLatency } from "../support/latency-results";
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
      // The shell owns an active inbox query (GET /inbox/items, ADR-0133) on every route. Task lists are
      // still placeholders, so counting /tasks alone would make this gate vacuous.
      const count = (route: string) => daemon.requests.filter((request) => request.path === route).length;
      await expect.poll(() => count("/api/v1/inbox/items")).toBeGreaterThan(0);
      const inboxBefore = count("/api/v1/inbox/items");
      const tasksBefore = count("/api/v1/tasks");
      const screenPaths = new Set(
        daemon.requests
          .map((request) => request.path)
          .filter(
            (route) =>
              route.startsWith("/api/v1/") &&
              ![
                "/api/v1/stream",
                "/api/v1/health",
                "/api/v1/inbox/items",
                "/api/v1/notifications/unread-count",
                "/api/v1/daemon",
              ].includes(route),
          ),
      );
      const screenBefore = Object.fromEntries([...screenPaths].map((route) => [route, count(route)]));
      for (let i = 1; i <= 20; i++)
        daemon.sendEvent("task.event", {
          id: i,
          seq: i,
          task_id: "unrelated",
          ts: "2026-09-30T00:00:00Z",
          event: { type: "worker_progress" },
        });
      for (let i = 0; i < 10; i++) {
        daemon.sendEvent("daemon", { cursor: i + 21 });
        await page.waitForTimeout(2000);
      }
      // One or two inbox refreshes may be its 15 s fallback poll. SSE must not
      // turn the 2 s daemon ticks or unrelated progress events into refetches.
      expect(count("/api/v1/inbox/items") - inboxBefore).toBeLessThanOrEqual(2);
      expect(count("/api/v1/tasks") - tasksBefore).toBe(0);
      const screenRefetches = Object.fromEntries(
        [...screenPaths].map((route) => [route, count(route) - screenBefore[route]]),
      );
      recordLatency({
        kind: "S2",
        path: screen.path,
        fixture: screen.fixture,
        unrelatedRefetches: screenRefetches,
        inboxPolls: count("/api/v1/inbox/items") - inboxBefore,
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
