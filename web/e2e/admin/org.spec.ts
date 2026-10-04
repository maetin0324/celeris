import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// beforeAll の一時 dir・server をこの file の試験で共有するので、file 内は 1 worker で順に流す（2026-10-04 の並列化と同じ扱い）。
test.describe.configure({ mode: "default" });

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

  test("/org/secretary が横 scroll なく読める", async ({ page }) => {
    await page.goto(`${gateway.base}/org/secretary`);
    await expect(page).toHaveURL(/\/org\/cos$/);
    await expect(page.getByRole("heading", { level: 1, name: "組織の人 cos" })).toBeVisible();
    await expectNoHorizontalScroll(page);
  });

  test("/org の skill 欄と確認ダイアログが横 scroll なく読める", async ({ page }) => {
    await page.goto(`${gateway.base}/org?selected=ui-ux`);
    await page.getByRole("button", { name: "frontend-design を外す" }).click();
    await expect(page.getByRole("alertdialog")).toBeVisible();
    await expectNoHorizontalScroll(page);
  });

  test("/org/cos が横 scroll なく読める", async ({ page }) => {
    await page.goto(`${gateway.base}/org/cos`);
    await expect(page.getByRole("heading", { level: 1, name: "組織の人 cos" })).toBeVisible();
    await expectNoHorizontalScroll(page);
  });
});

type Mutation = { method: string; url: string };
async function routeMutations(page: import("@playwright/test").Page, status: number, body: unknown = {}) {
  const calls: Mutation[] = [];
  await page.route("**/api/org/**", async (route) => {
    const request = route.request();
    if (request.method() === "GET") return route.continue();
    calls.push({ method: request.method(), url: request.url() });
    await route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  });
  return calls;
}

test.describe("skill の操作", () => {
  test("外す前に確認ダイアログが影響（届かなくなる担当）を示し、戻ると送らない", async ({ page }) => {
    const calls = await routeMutations(page, 200);
    await page.goto(`${gateway.base}/org?selected=ui-ux`);
    const skills = page.getByRole("region", { name: "担当の skill" });
    await expect(skills.getByText("CoS から継承")).toBeVisible();
    await skills.getByRole("button", { name: "frontend-design を外す" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("UI/UX、とても長い名前の課");
    await expect(dialog).toContainText("さらに下の課 の worker に frontend-design が届かなくなります");
    await expect(dialog).toContainText("元に戻す方法");
    await dialog.getByRole("button", { name: "戻る" }).click();
    await expect(dialog).toBeHidden();
    expect(calls).toEqual([]);

    await skills.getByRole("button", { name: "frontend-design を外す" }).click();
    await dialog.getByRole("button", { name: "frontend-design を外す" }).click();
    await expect(dialog).toBeHidden();
    expect(calls.map((call) => call.method)).toEqual(["DELETE"]);
    expect(calls[0]?.url).toMatch(/\/api\/org\/ui-ux\/skills\/frontend-design$/);
    await expect(skills.getByRole("status")).toHaveText("frontend-design を外しました。");
    await expect(skills.getByRole("list", { name: "mount された skill" })).toBeFocused();
  });

  test("未選択と失敗の error は select と aria-describedby で結ばれ、focus が欄へ移る", async ({ page }) => {
    const calls = await routeMutations(page, 422, { detail: "この skill は mount できません" });
    await page.goto(`${gateway.base}/org?selected=eng`);
    const skills = page.getByRole("region", { name: "担当の skill" });
    const select = skills.getByLabel("mount する skill");
    await skills.getByRole("button", { name: "mount", exact: true }).click();
    await expect(select).toBeFocused();
    await expect(select).toHaveAttribute("aria-invalid", "true");
    await expect(select).toHaveAccessibleDescription(/mount する skill を選んでください。/);
    expect(calls).toEqual([]);

    await select.selectOption("review");
    await expect(select).not.toHaveAttribute("aria-invalid", "true");
    await skills.getByRole("button", { name: "mount", exact: true }).click();
    await expect(select).toHaveAccessibleDescription(/この skill は mount できません/);
    await expect(select).toBeFocused();
    expect(calls.map((call) => call.method)).toEqual(["POST"]);
    // 入力欄の枠は --color-input のまま。
    const border = await select.evaluate((element) => ({
      actual: getComputedStyle(element).borderTopColor,
      input: (() => {
        const probe = document.createElement("div");
        probe.style.color = "var(--color-input)";
        document.body.append(probe);
        const color = getComputedStyle(probe).color;
        probe.remove();
        return color;
      })(),
    }));
    expect(border.actual).toBe(border.input);
  });

  test("403 では操作を無効にし、理由を出す", async ({ page }) => {
    await routeMutations(page, 403, { detail: "forbidden" });
    await page.goto(`${gateway.base}/org?selected=eng`);
    const skills = page.getByRole("region", { name: "担当の skill" });
    await skills.getByLabel("mount する skill").selectOption("review");
    await skills.getByRole("button", { name: "mount", exact: true }).click();
    const reason = skills.getByRole("alert");
    await expect(reason).toContainText("権限がありません（403）");
    await expect(reason).toBeFocused();
    await expect(skills.getByRole("button", { name: "mount", exact: true })).toBeDisabled();
    await expect(skills.getByLabel("mount する skill")).toBeDisabled();
  });
});
