import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";
import {
  applyStateRoute,
  LONG_TASK_ID,
  MANY_INBOX,
  MANY_NOTICES,
  MANY_TASKS,
  type StateKey,
  stateByKey,
  states,
} from "../support/states";

// 状態の変種（support/states.ts）× 代表画面。状態ごとに画面を開き、その状態が描かれることを確かめる。
// 1 画面ごとに偽 daemon を起こし直す（loading の保留・route を他の画面に持ち越さない）。

type Gateway = Awaited<ReturnType<typeof startFixtureGateway>>;
type Check = (page: Page, url: string, gateway: Gateway) => Promise<void>;

/** 画面の横 scroll が無い（文書の幅が viewport に収まる）。 */
async function expectNoHorizontalScroll(page: Page) {
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow, "横 scroll が出ている").toBeLessThanOrEqual(0);
}

/** 画面が状態を描けない所見（記録 qa-fixtures.md に書いたもの）。期待どおり失敗する試験として残す。 */
const findings: Record<string, string> = {
  "long-text /inbox":
    "受信箱の項目の題名 link が inline-flex の中で break-words のため、区切りの無い長い語が折り返さず 360px で横 scroll が出る",
};

const timeout = { timeout: 20_000 };

const checks: Record<StateKey, Check> = {
  "long-text": async (page, url) => {
    await expect(page.locator("#main")).toContainText("averyveryverylongword", timeout);
    await expectNoHorizontalScroll(page);
  },
  "long-id": async (page, url) => {
    if (url.includes(LONG_TASK_ID)) await expect(page.locator("h1")).toContainText(LONG_TASK_ID, timeout);
    else await expect(page.locator('[data-fetch-state="ready"]').first()).toBeVisible(timeout);
    if (url === "/tasks") await expect(page.locator(`[data-task-id="${LONG_TASK_ID}"]`)).toHaveCount(1);
    await expectNoHorizontalScroll(page);
  },
  empty: async (page, url) => {
    const main = page.locator("#main");
    const message: Record<string, string> = {
      "/inbox": "いま決めることはありません。",
      "/notifications": "通知はありません。",
      "/tasks": "条件に一致するタスクがありません",
      "/graph": "0 ノード / 0 辺",
    };
    await expect(main).toContainText(message[url], timeout);
    await expect(page.locator('[data-fetch-state="error"]')).toHaveCount(0);
  },
  many: async (page, url) => {
    const main = page.locator("#main");
    if (url === "/tasks") {
      await expect(main).toContainText(`${MANY_TASKS} 件`, timeout);
      expect(await page.locator("[data-task-id]").count()).toBeGreaterThanOrEqual(20);
    }
    if (url === "/inbox")
      await expect(main.getByText(`多数の判断 ${MANY_INBOX}`, { exact: true })).toBeAttached(timeout);
    if (url === "/notifications") await expect(main).toContainText(`未読 ${MANY_NOTICES} 件`, timeout);
    await expectNoHorizontalScroll(page);
  },
  loading: async (page, _url, gateway) => {
    const loading = page.locator('[data-fetch-state="loading"]');
    await expect(loading.first()).toBeVisible(timeout);
    await expect(loading.first()).toHaveAttribute("aria-busy", "true");
    await expect(page.getByRole("status").filter({ hasText: "読み込み中…" }).first()).toBeVisible(timeout);
    expect(gateway.daemon.heldCount).toBeGreaterThan(0);
    // 保留を解くと読み込み中が消える（時計でなく出来事で解く）。
    gateway.daemon.releaseHeld();
    await expect(loading).toHaveCount(0, timeout);
  },
  error: async (page) => {
    const error = page.locator('[data-fetch-state="error"]').first();
    await expect(error).toBeVisible(timeout);
    await expect(error).toHaveAttribute("role", "alert");
    await expect(error.getByRole("button", { name: "再試行" })).toBeVisible();
  },
  stale: async (page, url) => {
    const status = page.locator("[data-connection]");
    await expect(status).toHaveAttribute("data-connection", "reconnecting", timeout);
    await expect(status).toHaveText("接続状態: 再接続中");
    // 切断中も取得済みのデータは残る。
    const content: Record<string, string> = {
      "/": "画面群の長文",
      "/tasks": "複数の画面",
      "/tasks/T1/runs/R1": "検証ログ 1",
      "/providers": "claude-main",
    };
    await expect(page.locator("#main")).toContainText(content[url], timeout);
  },
  forbidden: async (page) => {
    const denied = page.locator('[data-fetch-state="permission-denied"]').first();
    await expect(denied).toBeVisible(timeout);
    await expect(denied).toContainText("権限がありません");
  },
};

for (const { key } of states) {
  const state = stateByKey(key);
  test.describe(`state ${key}`, () => {
    for (const url of state.screens) {
      test(`${key} ${url}`, async ({ page }) => {
        const finding = findings[`${key} ${url}`];
        test.fail(finding !== undefined, finding);
        const gateway = await startFixtureGateway(state.daemon);
        try {
          await page.setViewportSize({ width: 360, height: 800 });
          await applyStateRoute(page, state);
          await page.goto(`${gateway.base}${url}`);
          await expect(page.locator("h1")).toBeVisible(timeout);
          await checks[key](page, url, gateway);
        } finally {
          await gateway.close();
        }
      });
    }
  });
}

test("states.ts は 8 状態を持ち、各状態が 2〜4 画面に当たる", () => {
  expect(states.map((state) => state.key)).toEqual([
    "long-text",
    "long-id",
    "empty",
    "many",
    "loading",
    "error",
    "stale",
    "forbidden",
  ]);
  for (const state of states) {
    expect(state.screens.length, state.key).toBeGreaterThanOrEqual(2);
    expect(state.screens.length, state.key).toBeLessThanOrEqual(4);
  }
});
