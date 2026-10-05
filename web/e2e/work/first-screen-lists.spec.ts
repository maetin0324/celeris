import { expect, type Locator, type Page, test } from "@playwright/test";
import { expectInFirstScreen, FIRST_SCREEN_SIZES, waitForScreen } from "../support/first-screen";
import { startFixtureGateway } from "../support/fixture-gateway";

// 一覧画面（/tasks・/board・/releases）の先頭行が、scroll 前の最初の 1 画面に入る（2026-10-06 の要望）。
// 狭い幅ではフィルタを 1 行 toolbar・状態の横 scroll 1 行・折りたたみに詰めたので、その位置を測る。
// 詰めてもフィルタは URL の query から復元される（選択状態が見える）ことも確かめる。

let fixture: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  fixture = await startFixtureGateway();
});
test.afterAll(async () => {
  await fixture.close();
});

type ListScreen = { path: string; heading: string; first: (page: Page, width: number) => Locator };

const screens: ListScreen[] = [
  { path: "/tasks", heading: "タスク", first: (page) => page.locator("[data-task-id]").first() },
  {
    path: "/board",
    heading: "ボード",
    first: (page) => page.getByTestId("board-table").locator("[data-task-id]").first(),
  },
  {
    path: "/releases",
    heading: "リリース",
    first: (page, width) =>
      width < 640
        ? page.locator("[data-testid^='mobile-release-']").first()
        : page.locator("[data-testid^='release-']").first(),
  },
];

for (const size of FIRST_SCREEN_SIZES) {
  for (const screen of screens) {
    test(`${screen.path} ${size.width}: 一覧の先頭行が最初の 1 画面に入る`, async ({ page }) => {
      await page.setViewportSize(size);
      await page.goto(`${fixture.base}${screen.path}`);
      await waitForScreen(page, screen.heading);
      await expectInFirstScreen(page, screen.first(page, size.width), `${screen.path} の先頭行`);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
      ).toBe(0);
    });
  }
}

test("/tasks 360: 状態は横 scroll の 1 行、URL の status・order を復元して選択状態を見せる", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${fixture.base}/tasks?status=ready&order=created_desc`);
  await waitForScreen(page, "タスク");
  await expect(page.getByTestId("tasks-status-chips").getByRole("button", { name: /実行待ち/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("tasks-status-chips").getByRole("button", { name: /完了/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await expect(page.getByLabel("並び")).toHaveValue("created_desc");
  await expect(page.locator("[data-testid='tasks-filter'] select[name='limit']")).toBeHidden();
  // 状態の chip 8 個は 1 行（同じ上端）に並び、溢れた分は区画の中だけ横に scroll する。
  const chips = page.getByTestId("tasks-status-chips");
  const tops = await chips
    .locator("button")
    .evaluateAll((buttons) => buttons.map((button) => button.getBoundingClientRect().top));
  expect(tops).toHaveLength(9);
  expect(new Set(tops.map(Math.round)).size).toBe(1);
  expect(await chips.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  // 検索と並びは同じ行。
  const search = await page.getByRole("searchbox", { name: "検索" }).boundingBox();
  const order = await page.getByLabel("並び").boundingBox();
  expect(search && order && Math.abs(search.y - order.y) < 2).toBe(true);
});

test("/tasks 360: 件数は「その他の条件」で開き、絞り込みで URL に残る", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${fixture.base}/tasks`);
  await waitForScreen(page, "タスク");
  const limit = page.locator("[data-testid='tasks-filter'] select[name='limit']");
  await expect(limit).toBeHidden();
  const more = page.getByRole("button", { name: "その他の条件" });
  await expect(more).toHaveAttribute("aria-expanded", "false");
  await more.click();
  await expect(more).toHaveAttribute("aria-expanded", "true");
  await limit.selectOption("50");
  await page
    .getByTestId("tasks-status-chips")
    .getByRole("button", { name: /停止中/ })
    .click();
  await page.getByRole("button", { name: "絞り込み" }).click();
  await expect(page).toHaveURL(/status=blocked/);
  await expect(page).toHaveURL(/limit=50/);
  // 件数は狭い幅で畳むが、URL に値があるときは開いたまま見せる。
  await expect(limit).toBeVisible();
  await expect(limit).toHaveValue("50");
  await expect(page.getByTestId("tasks-status-chips").getByRole("button", { name: /停止中/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

test("/board 360: 状態は横 scroll の 1 行、URL の column と project を復元する", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${fixture.base}/board?column=waiting`);
  await waitForScreen(page, "ボード");
  const nav = page.getByRole("navigation", { name: "状態で絞り込む" });
  await expect(nav.locator("[aria-current='page']")).toHaveText(/^待ち/);
  const tops = await nav.getByRole("link").evaluateAll((links) => links.map((l) => l.getBoundingClientRect().top));
  expect(new Set(tops.map(Math.round)).size).toBe(1);
  // 案件と検索は同じ行。
  const project = await page.getByTestId("board-toolbar").locator("select[name='project']").boundingBox();
  const search = await page.getByTestId("board-toolbar").getByRole("searchbox", { name: "検索" }).boundingBox();
  expect(project && search && Math.abs(project.y - search.y) < 2).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)).toBe(
    0,
  );
});

test("/releases: この画面から昇格していなければ「昇格の結果」の節を出さない", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${fixture.base}/releases`);
  await waitForScreen(page, "リリース");
  await expect(page.getByRole("heading", { name: "昇格の結果" })).toHaveCount(0);
  // 稼働中の版の 3 項目は項目名と値を横に並べ、折り返しても 2 行に収まる（以前は 3 項目 × 2 段の 6 行）。
  const terms = page.getByTestId("releases-running").locator("dt");
  await expect(terms).toHaveCount(3);
  const boxes = await terms.evaluateAll((dts) => dts.map((dt) => dt.getBoundingClientRect().top));
  expect(new Set(boxes.map(Math.round)).size).toBeLessThanOrEqual(2);
  const dd = await page.getByTestId("releases-running").locator("dd").first().boundingBox();
  const dt = await terms.first().boundingBox();
  expect(dd && dt && Math.abs(dd.y - dt.y) < 2).toBe(true);
});
