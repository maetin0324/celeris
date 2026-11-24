import { expect, type Page, test } from "@playwright/test";
import { projectOverviewGolden } from "../support/fake-daemon.mjs";
import { startFixtureGateway } from "../support/fixture-gateway";

// 案件の概要の panel（ADR 2026-11-23-project-observability-publish D1・D4）。実の gateway と偽 daemon の rich profile で、
// 人が 14 項目・印・強い警告を読み（手書きの projectOverviewFixture）、drill-down の link で既存の画面（run の log・成果物の
// viewer の Notebook・作業ツリー・変更の Git・文献・chat の thread・task）へ移れることを確かめる。drill-down は、本物の
// backend の試験が生成した golden（crates/task-api/tests/fixtures/project_overview_drilldown.json）を偽 daemon がそのまま
// 返す構成（projectOverview: "drilldown"）で、golden の link を押す。
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

  // 証拠の Knowledge の参照は path だけの文献で、path を写せる文字で出し、知識の検索の画面へ移す（golden の O6 は url の文献だけ）。
  const kb = list.locator("li[data-item='refutations'] span[data-link='literature']");
  await expect(kb).toContainText("projects/demo.md");
  await kb.getByRole("link", { name: "知識を検索" }).click();
  await expect(page).toHaveURL(/\/knowledge$/);
});

// golden の中の link（本物の backend が出した形）。label は T1・R1・TH1・M1 に置き換え済み。
const golden = projectOverviewGolden();
function goldenLink<K extends string>(item: string, kind: K) {
  const entry = golden.items
    .find((row) => row.key === item)
    ?.entries.find((row) => row.links.some((link) => link.kind === kind));
  const link = entry?.links.find((row) => row.kind === kind);
  if (!entry || !link) throw new Error(`golden has no ${kind} link in ${item}`);
  return { entry: entry.id, link: link as Extract<(typeof entry.links)[number], { kind: K }> };
}

test.describe("drill-down（本物の backend の概要の golden）", () => {
  test.beforeEach(async () => {
    await gateway.close();
    gateway = await startFixtureGateway({ projectOverview: "drilldown" });
  });

  async function entryLink(page: Page, item: string, entry: string, kind: string) {
    const panel = await openOverview(page);
    return panel.locator(`li[data-item='${item}'] [data-entry='${entry}'] [data-link='${kind}']`).first();
  }

  test("log・Notebook・作業ツリー・task 付きの Git・文献・chat の thread・task へ移る", async ({ page }) => {
    const log = goldenLink("verification", "log");
    await (await entryLink(page, "verification", log.entry, "log")).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${log.link.task_id}/runs/${log.link.run_id}$`));
    await expect(
      page.getByRole("heading", { name: `run ログ ${log.link.task_id} / ${log.link.run_id}` }),
    ).toBeVisible();
    await expect(page.getByText("循環の辺 3 本が成分の大半を作っていました")).toBeVisible();

    const notebook = goldenLink("verification", "notebook");
    await (await entryLink(page, "verification", notebook.entry, "notebook")).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${notebook.link.task_id}\\?tab=artifacts$`));
    const artifacts = page.getByTestId("task-artifacts");
    await expect(artifacts).toBeVisible();
    await expect(artifacts.locator("[data-artifact='0']")).toContainText(notebook.link.name);

    await (await entryLink(page, "verification", notebook.entry, "files")).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${notebook.link.task_id}/files$`));
    await expect(page.getByRole("heading", { name: `作業ツリーと成果物 ${notebook.link.task_id}` })).toBeVisible();

    for (const item of ["refutations", "experiments_and_tasks"]) {
      const git = goldenLink(item, "git");
      expect(git.link.task_id).toBeTruthy();
      const anchor = await entryLink(page, item, git.entry, "git");
      await expect(anchor).toContainText(`Git ${git.link.repo}@${git.link.sha.slice(0, 12)}`);
      await anchor.click();
      await expect(page).toHaveURL(new RegExp(`/tasks/${git.link.task_id}/changes$`));
      await expect(page.getByRole("heading", { name: `変更 ${git.link.task_id}` })).toBeVisible();
      await expect(page.locator(`[data-repo='${git.link.repo}']`)).toBeVisible();
    }

    const artifact = goldenLink("approved_strategy", "artifact");
    await (await entryLink(page, "approved_strategy", artifact.entry, "artifact")).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${artifact.link.task_id}\\?tab=artifacts$`));
    await expect(page.getByTestId("task-artifacts").locator("[data-artifact='0']")).toContainText(artifact.link.name);

    const paper = goldenLink("refutations", "literature");
    const anchor = await entryLink(page, "refutations", paper.entry, "literature");
    await expect(anchor).toHaveAttribute("href", paper.link.url ?? "");
    await expect(anchor).toHaveAttribute("target", "_blank");
    await expect(anchor).toHaveAttribute("rel", "noopener noreferrer");
    await expect(anchor).toContainText("文献（外部・untrusted）");

    for (const item of ["discussion_sources", "pending_human_decisions"]) {
      const chat = goldenLink(item, "chat");
      await (await entryLink(page, item, chat.entry, "chat")).click();
      await expect(page).toHaveURL(new RegExp(`/\\?thread=${chat.link.thread_id}$`));
      await expect(page.getByText("案: 循環の辺を少数だけ切る識別実験を先に行う。")).toBeVisible();
    }

    const task = goldenLink("experiments_and_tasks", "task");
    await (await entryLink(page, "experiments_and_tasks", task.entry, "task")).click();
    await expect(page).toHaveURL(new RegExp(`/tasks/${task.link.task_id}$`));
    await expect(page.getByRole("heading", { name: `タスクの詳細 ${task.link.task_id}` })).toBeVisible();
    await expect(page.getByText("実験: task-core の循環の辺を数える").first()).toBeVisible();
  });
});
