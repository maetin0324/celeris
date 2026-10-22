import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import type { Locator } from "@playwright/test";
import { renderToStaticMarkup } from "react-dom/server";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  OVERLAY_FIXTURE_TIMEOUT,
  openOverlayFixture,
  seriousViolations,
} from "../../components/ui/overlay-browser-test";
import { integration } from "./fixtures/task-integration-data";
import { TaskIntegrationActions } from "./task-integration";

it.each([
  ["unintegrated", "未取り込み"],
  ["integrated", "取り込み済み"],
  ["discarded", "捨てた"],
  ["held", "保留"],
] as const)("renders %s independently from promotion pending", (state, label) => {
  const html = renderToStaticMarkup(
    <TaskIntegrationActions
      taskId="T 1"
      integration={{ ...integration, state }}
      promotion={{ status: "promotion_pending" }}
    />,
  );
  expect(html).toContain(label);
  expect(html).toContain("昇格待ち");
});

it("renders not_applicable as a read-only state without operation buttons", () => {
  const html = renderToStaticMarkup(
    <TaskIntegrationActions
      taskId="T 1"
      integration={{
        ...integration,
        state: "not_applicable",
        reason: "remote repository: not checked by the sweep",
      }}
    />,
  );
  expect(html).toContain("対象外（遠隔 repo）");
  expect(html).toContain("remote repository: not checked by the sweep");
  expect(html).toContain('data-state="not_applicable"');
  expect(html).not.toContain("<button");
});

describe("integration buttons call the task-api contract", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  const calls: Array<{ method: string; path: string; body: unknown }> = [];
  beforeAll(async () => {
    fixture = await openOverlayFixture("/features/tasks/fixtures/task-integration.html");
    await fixture.page.route("**/api/**", async (route) => {
      const request = route.request();
      calls.push({ method: request.method(), path: new URL(request.url()).pathname, body: request.postDataJSON() });
      await route.fulfill({ json: { integrated: false } });
    });
  }, OVERLAY_FIXTURE_TIMEOUT);
  afterAll(async () => fixture?.close());

  async function clickAndExpect(region: Locator, method: string, repo: string, reason?: string) {
    const { page } = fixture;
    const index = calls.length;
    await region.getByRole("button", { name: method, exact: true }).click();
    if (method === "discard") {
      const dialog = page.getByRole("alertdialog");
      await dialog.waitFor();
      expect(calls).toHaveLength(index);
      await dialog.getByRole("button", { name: "対象の変更を破棄" }).click();
      await dialog.waitFor({ state: "hidden" });
    }
    await region
      .getByRole("status")
      .filter({ hasText: method === "merge" ? "取り込みを依頼しました" : "操作を受け付けました" })
      .waitFor();
    expect(calls.slice(index)).toEqual([
      {
        method: "POST",
        path: `/api/tasks/T%201/integrations/${repo}/${method}`,
        body: reason === undefined ? null : { reason },
      },
    ]);
    if (method === "merge") expect(await region.getByRole("status").textContent()).toContain("取り込みを依頼しました");
  }

  it.each([
    ["unintegrated", "web%20%E6%97%A5%E6%9C%AC%E8%AA%9E"],
    ["受信箱", "another%20repo"],
  ])(
    "%s: reason is required; merge/discard/hold use repo name and ReasonBody",
    async (name, repo) => {
      const region = fixture.page.getByRole("region", { name, exact: true });
      const input = region.getByRole("textbox");
      await input.fill("   ");
      const index = calls.length;
      expect(await region.getByRole("button", { name: "discard", exact: true }).isDisabled()).toBe(true);
      await region.getByRole("button", { name: "hold", exact: true }).click();
      await region.getByRole("alert").waitFor();
      expect(await region.getByRole("alert").textContent()).toContain("理由を入力");
      expect(calls).toHaveLength(index);
      await clickAndExpect(region, "merge", repo);
      await input.fill("  判断の理由  ");
      await clickAndExpect(region, "hold", repo, "判断の理由");
      // Cancel must not send a disposition.
      await region.getByRole("button", { name: "discard", exact: true }).click();
      await fixture.page.getByRole("alertdialog").getByRole("button", { name: "戻る" }).click();
      expect(calls).toHaveLength(index + 2);
      await clickAndExpect(region, "discard", repo, "判断の理由");
    },
    30_000,
  );

  it.each([
    ["held", "web%20%E6%97%A5%E6%9C%AC%E8%AA%9E"],
    ["discarded", "web%20%E6%97%A5%E6%9C%AC%E8%AA%9E"],
    ["受信箱", "another%20repo"],
  ])("%s: reopen uses the native task integration route", async (name, repo) => {
    await clickAndExpect(fixture.page.getByRole("region", { name, exact: true }), "reopen", repo);
  });

  it("keeps 44px targets and accessible confirmation at mobile widths", async () => {
    const { page } = fixture;
    const directory = process.env.INTEGRATION_SHOTS_DIR;
    if (directory) await mkdir(directory, { recursive: true });
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      const region = page.getByRole("region", { name: "unintegrated", exact: true });
      await region.getByRole("textbox").fill("理由");
      expect(await seriousViolations(page)).toEqual([]);
      if (directory) await page.screenshot({ path: join(directory, `integration-${width}.png`), fullPage: true });
      await region.getByRole("button", { name: "discard", exact: true }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.waitFor();
      expect(await seriousViolations(page)).toEqual([]);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      for (const control of await page.locator("button:visible, input:visible").all()) {
        const box = await control.boundingBox();
        expect(box?.width).toBeGreaterThanOrEqual(44);
        expect(box?.height).toBeGreaterThanOrEqual(44);
      }
      if (directory) await page.screenshot({ path: join(directory, `discard-${width}.png`), fullPage: true });
      await dialog.getByRole("button", { name: "戻る" }).click();
    }
  }, 60_000);
});
