import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// /org と /org/cos を 360px で開き、横 scroll が出ないこと（ページ全体が clientWidth に収まること）を見る。
// 長い名前・ID・深い木を置き、折り返しと字下げの上限が効いているかを確かめる。
const dir = mkdtempSync(path.join(tmpdir(), "celeris-admin-org-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const now = "2026-10-04T00:00:00Z";
const longId = "section-with-a-very-long-identifier-that-should-wrap-inside-the-row";
const nodes = [
  {
    id: "cos",
    name: "CoS",
    kind: "secretary",
    created_at: now,
    updated_at: now,
    profile: { skills_mounts: ["review"] },
  },
  { id: "eng", name: "技術部", kind: "department", parent_id: "cos", position: 1, created_at: now, updated_at: now },
  {
    id: "ui-ux",
    name: "UI/UX",
    kind: "section",
    parent_id: "eng",
    created_at: now,
    updated_at: now,
    profile: { skills_mounts: ["ui-ux-quality-gate", "frontend-design"], policy: ["小さく変える"] },
  },
  {
    id: longId,
    name: "とても長い名前の課でスマホ幅でも折り返して読めることを確かめるための課",
    kind: "section",
    parent_id: "ui-ux",
    created_at: now,
    updated_at: now,
  },
  { id: "deep", name: "さらに下の課", kind: "section", parent_id: longId, created_at: now, updated_at: now },
];
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: {
    ...defaultFixtures,
    "/api/v1/org": {
      items: nodes,
      effective_profiles: [
        {
          node_id: "cos",
          chain: ["cos"],
          skills_mounts: ["review"],
          skills: ["slurm", "rust", "react", "accessibility"],
          harnesses_allowed: ["conversation", "plan", "coding"],
          tier: "standard",
        },
        {
          node_id: "ui-ux",
          chain: ["cos", "eng", "ui-ux"],
          skills_mounts: ["review", "ui-ux-quality-gate", "frontend-design"],
        },
      ],
    },
    "/api/v1/skills": { initialized: true, root: "/tmp/skills", items: [{ name: "review", description: "レビュー" }] },
  },
});
let gateway: Awaited<ReturnType<typeof startGateway>>;
test.beforeAll(async () => {
  gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
});
test.afterAll(async () => {
  await gateway.close();
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});

async function expectNoHorizontalScroll(page: import("@playwright/test").Page) {
  const size = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
  expect(size.scrollWidth).toBeLessThanOrEqual(size.clientWidth);
}

test.describe("width: 360", () => {
  test.use({ viewport: { width: 360, height: 800 } });

  test("/org の木と課の詳細が横 scroll なく読める", async ({ page }) => {
    await page.goto(`${gateway.base}/org`);
    await expect(page.getByRole("heading", { level: 2, name: "組織の木" })).toBeVisible();
    await expect(page.getByRole("button", { name: /課 · とても長い名前の課/ })).toBeVisible();
    await expectNoHorizontalScroll(page);

    await page.getByRole("button", { name: /課 · UI\/UX/ }).click();
    await expect(page).toHaveURL(/selected=ui-ux/);
    const settings = page.getByRole("definition").filter({ hasText: "cos → eng → ui-ux" });
    await expect(settings).toBeVisible();
    await expect(page.getByRole("region", { name: "配下の担当の一覧" })).toBeVisible();
    await expectNoHorizontalScroll(page);

    await page.goto(`${gateway.base}/org?selected=${longId}`);
    await expect(page.getByRole("heading", { level: 3, name: /とても長い名前の課/ })).toBeVisible();
    await expectNoHorizontalScroll(page);
  });

  test("/org/cos が横 scroll なく読める", async ({ page }) => {
    await page.goto(`${gateway.base}/org/cos`);
    await expect(page.getByRole("heading", { level: 1, name: "組織の人 cos" })).toBeVisible();
    await expectNoHorizontalScroll(page);
  });
});
