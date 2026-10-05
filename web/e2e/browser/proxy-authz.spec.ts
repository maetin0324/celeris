import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";
import { BROWSER_RAW_LIVE_VIEW_URL } from "../support/fake-daemon.mjs";

const paths = ["/browser/live/T1/R1", "/browser/control/T1/R1/S1"];

test("live and control reject a different cookie session and a session without owner grant", async ({
  page,
  browser,
}) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  const other = await browser.newContext();
  try {
    await gateway.loginAsOwner(page);
    const otherPage = await other.newPage();
    await gateway.loginAsOther(otherPage);
    for (const route of paths) {
      const response = await otherPage.request.get(`${gateway.base}${route}`);
      expect(response.status(), route).toBe(403);
      expect(await response.json(), route).toEqual({ code: "not_owner" });
      expect(await response.text()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    }
    const mutation = await otherPage.request.post(`${gateway.base}/browser/control/T1/R1/S1`, {
      headers: { Origin: gateway.base },
      data: { command: { kind: "pause" } },
    });
    expect(mutation.status()).toBe(403);
    expect(await mutation.json()).toEqual({ code: "not_owner" });
    await otherPage.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(otherPage.getByTestId("browser-owner-request").first()).toBeVisible();
    expect(await otherPage.content()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    expect(gateway.dashboard.requests).toEqual([]);
  } finally {
    await other.close();
    await gateway.close();
  }
});

test("a run under another task and a missing run fail closed for live and control", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    const { csrf } = await gateway.loginAsOwner(page);
    for (const route of [
      "/browser/live/T1/R2",
      "/browser/control/T1/R2/S2",
      "/browser/live/T1/NO_RUN",
      "/browser/control/T1/NO_RUN/S1",
    ]) {
      const response = await page.request.get(`${gateway.base}${route}`);
      expect(response.status(), route).toBe(404);
      expect(await response.json(), route).toEqual({ code: "not_found" });
      expect(await response.text()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    }
    const mutation = await page.request.post(`${gateway.base}/browser/control/T1/R2/S2`, {
      headers: { Origin: gateway.base },
      data: { csrf, command: { kind: "pause" }, expected_version: 0, idempotency_key: "cross-task" },
    });
    expect(mutation.status()).toBe(404);
    expect(await mutation.json()).toEqual({ code: "not_found" });
    expect(gateway.dashboard.requests).toEqual([]);
  } finally {
    await gateway.close();
  }
});

test("generic API relay rejects browser mutations and removes raw live URLs from JSON and SSE", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    for (const route of ["/api/tasks/T1/browser/control/R1/S1", "/api/browser/identities"]) {
      const response = await page.request.get(`${gateway.base}${route}`);
      expect(response.status(), route).toBe(403);
      expect(await response.json(), route).toEqual({ error: "browser_route_required" });
    }
    const events = await page.request.get(`${gateway.base}/api/tasks/T1/events?types=browser_updated`);
    expect(events.status()).toBe(200);
    const json = await events.text();
    expect(json).toContain("browser_updated");
    expect(json).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);

    await page.goto(`${gateway.base}/browser`);
    const priorStreams = gateway.daemon.requests.filter((r) => r.path === "/api/v1/stream").length;
    const stream = page.evaluate(async () => {
      const controller = new AbortController();
      const response = await fetch("/events", { signal: controller.signal });
      const reader = response.body?.getReader();
      if (!reader) throw new Error("SSE body missing");
      try {
        let frame = "";
        while (!frame.includes("browser_updated")) {
          const chunk = await reader.read();
          if (chunk.done) throw new Error("SSE closed before browser_updated");
          frame += new TextDecoder().decode(chunk.value);
        }
        return frame;
      } finally {
        controller.abort();
      }
    });
    await expect
      .poll(() => gateway.daemon.requests.filter((r) => r.path === "/api/v1/stream").length)
      .toBeGreaterThan(priorStreams);
    gateway.daemon.sendEvent("task.event", {
      task_id: "T1",
      event: { type: "browser_updated", browser: { live_view_url: BROWSER_RAW_LIVE_VIEW_URL } },
    });
    const frame = await stream;
    expect(frame).toContain("browser_updated");
    expect(frame).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
    await expect(page.getByTestId("browser-run-row").first()).toBeVisible();
    expect(await page.content()).not.toContain(BROWSER_RAW_LIVE_VIEW_URL);
  } finally {
    await gateway.close();
  }
});
