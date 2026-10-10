import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

test("gateway without live relay reports every run unavailable and shows event monitoring", async ({ page }) => {
  const gateway = await startBrowserGateway({ liveUpstream: false, backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    const response = await page.request.get(`${gateway.base}/browser/runs`);
    const { items } = (await response.json()) as { items: Array<{ live: { state: string; reason: string } }> };
    expect(response.ok()).toBeTruthy();
    expect(items.length).toBeGreaterThan(1);
    expect(items.every((item) => item.live.state === "disabled" && item.live.reason === "relay_unavailable")).toBe(
      true,
    );

    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(page.getByTestId("browser-live-iframe")).toHaveCount(0);
    await expect(page.getByText("映像なし — イベントで監視中")).toBeVisible();
    await expect(page.getByText("Live View の中継を利用できません。")).toBeVisible();
  } finally {
    await gateway.close();
  }
});

test("a human-waiting run also shows event monitoring without an iframe", async ({ page }) => {
  const gateway = await startBrowserGateway({
    liveUpstream: false,
    backend: { credentialWait: false, runStateOverride: "WAITING_FOR_HUMAN" },
  });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(page.getByTestId("browser-live-iframe")).toHaveCount(0);
    await expect(page.getByText("映像なし — イベントで監視中")).toBeVisible();
    await expect(page.getByText("Live View の中継を利用できません。")).toBeVisible();
  } finally {
    await gateway.close();
  }
});
