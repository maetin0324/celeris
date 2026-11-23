import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// 案件の概要の panel（ADR 2026-11-23-project-observability-publish D1・D4）。実の gateway と偽 daemon の rich profile
// （`GET /projects/P1/overview` は fake-daemon.mjs の projectOverviewFixture）で、人が 14 項目・印・強い警告を読み、
// drill-down の link で既存の画面（run の log・成果物の viewer の Notebook・作業ツリー・変更の Git・文献・chat の thread）へ移れることを確かめる。
// 外部の URL は開かない（試験は外部ネットワークに出ない）。属性（https・新しい tab・noopener）だけを見る。

const items = [
  "original_goal",
  "approved_strategy",
  "candidates",
  "hypotheses",
  "verification",
  "refutations",
  "latest_findings",
  "discussion_sources",
  "experiments_and_tasks",
  "budget_spent",
  "next_verification",
  "pending_human_decisions",
  "risks",
  "publish_readiness",
];

let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;

test.beforeEach(async () => {
  gateway = await startFixtureGateway();
});

test.afterEach(async () => {
  await gateway.close();
});

async function openOverview(page: Page) {
  await page.goto(`${gateway.base}/projects/P1`);
  const panel = page.getByTestId("project-overview-panel");
  await expect(panel).toBeVisible();
  return panel;
}

test("概要は 14 項目を決めた順に出し、印を文字で区別し、強い警告だけを先頭にまとめる", async ({ page }) => {
  const panel = await openOverview(page);
  const list = panel.getByRole("list", { name: "案件の概要の 14 項目" });
  await expect(list.locator(":scope > li")).toHaveCount(14);
  expect(
    await list.locator(":scope > li").evaluateAll((rows) => rows.map((row) => row.getAttribute("data-item"))),
  ).toEqual(items);
  const heading = (key: string) => list.locator(`li[data-item='${key}'] > h3`);
  await expect(heading("original_goal")).toContainText("O1 原目的");
  await expect(heading("original_goal").locator("[data-mark]")).toHaveText("確認済み");
  await expect(heading("candidates").locator("[data-mark]")).toHaveText("推定");
  await expect(heading("pending_human_decisions").locator("[data-mark]")).toHaveText("要判断");
  await expect(heading("approved_strategy").locator("[data-mark]")).toHaveText("矛盾・反証");
  await expect(heading("publish_readiness")).toContainText("記録無し");
  await expect(heading("publish_readiness").locator("[data-mark]")).toHaveCount(0);

  // 強い警告は panel の先頭（14 項目より前）にあり、通常の警告（cost_unknown）を含まない。
  const strong = panel.getByRole("region", { name: "強い警告" });
  await expect(panel.locator(":scope > *").first()).toHaveAttribute("data-testid", "overview-strong-warnings");
  await expect(strong).toHaveAttribute("data-count", "5");
  await expect(strong.getByRole("heading")).toHaveText("強い警告 5 件");
  await expect(strong.locator("li[data-warning]")).toHaveCount(5);
  await expect(strong.locator("li[data-warning='strategy_stale']")).toContainText(
    "強い警告（承認戦略）: 承認した戦略が古い",
  );
  await expect(strong.locator("li[data-warning='citation_stale']")).toContainText("議論の引用が編集・撤回された");
  await expect(strong.locator("li[data-warning='cost_unknown']")).toHaveCount(0);
  await expect(list.locator("li[data-item='risks']")).toContainText("単価の分からない model の run がある 1 件");

  // 議論の外部の文は untrusted、task の status は GET /projects/P1 の tasks[] から引く。
  await expect(list.locator("li[data-item='discussion_sources']")).toContainText(
    "ChatGPT の要約（外部の文・untrusted）",
  );
  await expect(list.locator("li[data-item='experiments_and_tasks'] [data-status='ready']")).toBeVisible();
});

test("drill-down: log・Notebook・作業ツリー・Git・文献・chat の thread・task へ移る", async ({ page }) => {
  let panel = await openOverview(page);
  await panel.locator("li[data-item='verification'] [data-link='log']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\/runs\/R1$/);
  await expect(page.getByRole("heading", { name: "run ログ T1 / R1" })).toBeVisible();

  panel = await openOverview(page);
  await panel.locator("li[data-item='verification'] [data-link='notebook']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\?tab=artifacts$/);
  await expect(page.getByTestId("task-artifacts")).toBeVisible();

  panel = await openOverview(page);
  await panel.locator("li[data-item='verification'] [data-link='files']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\/files$/);
  await expect(page.getByRole("heading", { name: "作業ツリーと成果物 T1" })).toBeVisible();

  panel = await openOverview(page);
  await panel.locator("li[data-item='refutations'] [data-link='git']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\/changes$/);
  await expect(page.getByRole("heading", { name: "変更 T1" })).toBeVisible();

  panel = await openOverview(page);
  const paper = panel.locator("li[data-item='refutations'] a[data-link='literature']");
  await expect(paper).toHaveAttribute("href", "https://example.org/papers/io-sweep");
  await expect(paper).toHaveAttribute("target", "_blank");
  await expect(paper).toHaveAttribute("rel", "noopener noreferrer");
  await expect(paper).toContainText("文献（外部・untrusted）");
  const kb = panel.locator("li[data-item='next_verification'] [data-link='literature']");
  await expect(kb).toContainText("projects/demo.md");
  await kb.getByRole("link", { name: "知識を検索" }).click();
  await expect(page).toHaveURL(/\/knowledge$/);

  panel = await openOverview(page);
  await panel.locator("li[data-item='discussion_sources'] [data-link='chat']").first().click();
  await expect(page).toHaveURL(/\/\?thread=chat-history$/);

  panel = await openOverview(page);
  await panel.locator("li[data-item='pending_human_decisions'] [data-link='task']").click();
  await expect(page).toHaveURL(/\/tasks\/T1$/);
});
