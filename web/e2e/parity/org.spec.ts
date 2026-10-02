import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

const dir = mkdtempSync(path.join(tmpdir(), "celeris-org-"));
const tokenFile = path.join(dir, "token");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
const now = "2026-09-30T00:00:00Z";
const nodes = [
  {
    id: "cos",
    name: "CoS",
    kind: "secretary",
    created_at: now,
    updated_at: now,
    profile: { skills_mounts: ["review"] },
  },
  { id: "ops", name: "運用部", kind: "department", parent_id: "cos", created_at: now, updated_at: now },
];
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: {
    ...defaultFixtures,
    "/api/v1/org": {
      items: nodes,
      effective_profiles: [{ node_id: "cos", chain: ["cos"], skills_mounts: ["review"], skills: ["slurm"] }],
    },
    "/api/v1/skills": {
      initialized: true,
      root: "/tmp/skills",
      items: [
        { name: "review", description: "レビュー" },
        { name: "hpc", description: "HPC" },
      ],
    },
    "/api/v1/skills/review": { name: "review", skill_md: "# レビュー\n\n**確認**する" },
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

test("parity: /org/secretary → /org/cos", async ({ page }) => {
  await page.goto(`${gateway.base}/org/secretary`);
  await expect(page).toHaveURL(/\/org\/cos$/);
  await expect(page.getByRole("heading", { level: 1, name: "組織の人 cos" })).toBeVisible();
});

test("parity: /org 木・選択・作成・変更・削除・skill", async ({ page }) => {
  const actions: Array<{ method: string; url: string; body: unknown }> = [];
  await page.route("**/api/org**", async (route) => {
    const request = route.request();
    if (request.method() === "GET") return route.continue();
    actions.push({ method: request.method(), url: request.url(), body: request.postDataJSON() });
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  await page.goto(`${gateway.base}/org?selected=cos`);
  await expect(page.getByRole("heading", { level: 2, name: "組織の木" })).toBeVisible();
  await expect(page.getByRole("heading", { level: 3, name: "CoS" })).toBeVisible();
  await page.getByRole("button", { name: "部 · 運用部" }).click();
  await expect(page).toHaveURL(/selected=ops/);
  await page.goBack();
  await expect(page).toHaveURL(/selected=cos/);
  await page.getByRole("region", { name: "担当の追加" }).getByLabel("ID").fill("new-team");
  await page.getByRole("region", { name: "担当の追加" }).getByLabel("名前").fill("新しい課");
  await page.getByRole("region", { name: "担当の追加" }).getByRole("button", { name: "追加" }).click();
  await expect.poll(() => actions.some((item) => item.method === "POST" && item.url.endsWith("/api/org"))).toBe(true);
  await page.getByRole("region", { name: "担当の編集" }).getByLabel("名前").fill("CoS 改名");
  await page.getByRole("region", { name: "担当の編集" }).getByRole("button", { name: "保存" }).click();
  await expect
    .poll(() => actions.some((item) => item.method === "PATCH" && item.url.endsWith("/api/org/cos")))
    .toBe(true);
  await page.getByRole("button", { name: "review を表示" }).click();
  await expect(page.getByText("確認", { exact: true })).toBeVisible();
  const beforeSkillChange = {
    org: daemon.requests.filter((request) => request.path === "/api/v1/org").length,
    skills: daemon.requests.filter((request) => request.path === "/api/v1/skills").length,
    tasks: daemon.requests.filter((request) => request.path === "/api/v1/tasks").length,
  };
  await page.getByRole("button", { name: "外す" }).click();
  await expect
    .poll(() => actions.some((item) => item.method === "DELETE" && item.url.endsWith("/api/org/cos/skills/review")))
    .toBe(true);
  await page.getByLabel("mount する skill").selectOption("hpc");
  await page.getByRole("button", { name: "mount", exact: true }).click();
  await expect
    .poll(() => actions.some((item) => item.method === "POST" && item.url.endsWith("/api/org/cos/skills")))
    .toBe(true);
  expect(actions.find((item) => item.url.endsWith("/api/org/cos/skills"))?.body).toEqual({ skill: "hpc" });
  await expect
    .poll(() => daemon.requests.filter((request) => request.path === "/api/v1/org").length)
    .toBeGreaterThan(beforeSkillChange.org);
  await expect
    .poll(() => daemon.requests.filter((request) => request.path === "/api/v1/skills").length)
    .toBeGreaterThan(beforeSkillChange.skills);
  expect(daemon.requests.filter((request) => request.path === "/api/v1/tasks").length).toBe(beforeSkillChange.tasks);
  await page.getByRole("button", { name: "部 · 運用部" }).click();
  page.once("dialog", (dialog) => void dialog.accept());
  await page.getByRole("region", { name: "担当の編集" }).getByRole("button", { name: "削除" }).click();
  await expect
    .poll(() => actions.some((item) => item.method === "DELETE" && item.url.endsWith("/api/org/ops")))
    .toBe(true);
});

test("/org fixture screenshots", async ({ page }) => {
  const out = process.env.WEB_SHOTS_OUT;
  test.skip(!out, "WEB_SHOTS_OUT is required");
  mkdirSync(out as string, { recursive: true });
  for (const [name, route] of [
    ["org", "/org"],
    ["org-selected", "/org?selected=cos"],
  ] as const) {
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}${route}`);
      await expect(page.getByRole("heading", { level: 2, name: "組織の木" })).toBeVisible();
      await page.screenshot({ path: path.join(out as string, `${name}-fixture-${width}.png`), fullPage: true });
    }
  }
});
