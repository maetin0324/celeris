// ホームの e2e（ADR 2026-10-28-single-human-window-general-cos D5）: 統括 CoS が既定で一覧は無い、案件スレッドは ?thread= で読み統括へ戻る、?thread= の再開。
// 偽 daemon の fixture（rich profile）を使う。操作はすべて UI 経由で、状態確認は expect.poll まで。

import { expect, test } from "@playwright/test";
import { conversation, startChatGateway } from "./support";

test("ホームは統括 CoS を開き、スレッドを選ぶ一覧は無い（ADR 2026-10-28-single-human-window-general-cos D5）", async ({
  page,
}) => {
  const gateway = await startChatGateway();
  const { composer } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/`);
    // URL に会話が無ければ統括 CoS（fixture の chat-main）を開く。新しい会話は作らない。
    await expect(page).toHaveURL(/\?thread=chat-main$/);
    await expect(page.locator("#chat-thread-sidebar")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "会話一覧を開く" })).toHaveCount(0);
    await expect(page.locator('[data-thread-bar="general"]')).toBeVisible();
    await expect(composer).toBeVisible();
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-threads-1440.png` });
  } finally {
    await gateway.close();
  }
});

test("案件スレッドは ?thread= で読み、統括 CoS へ戻れる", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-history`);
    await expect(page.getByText("履歴 240", { exact: true })).toBeVisible();
    const bar = page.locator('[data-thread-bar="project"]');
    await expect(bar).toContainText("読む用です");
    await bar.getByRole("button", { name: "統括 CoS に戻る" }).click();
    await expect(page).toHaveURL(/\?thread=chat-main$/);
  } finally {
    await gateway.close();
  }
});

test("共有された ?thread= URL で開き、再読込しても同じ会話と本文・composer が残る", async ({ page }) => {
  const gateway = await startChatGateway();
  const { composer, scroller } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-history`);
    // 最新頁（240 件のうち seq 191〜240）が表示され、古い頁のボタンがある。
    await expect(page.getByText("履歴 240", { exact: true })).toBeVisible();
    await expect(scroller.getByRole("button", { name: "以前のメッセージ" })).toBeVisible();
    await expect(page).toHaveURL(/\?thread=chat-history$/);

    // 再読込: snapshot を取り直すだけ（stream の cursor 続きは再読込後から）。
    await page.reload();
    await expect(page.getByText("履歴 240", { exact: true })).toBeVisible();
    await expect(composer).toBeVisible();
    await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeVisible();
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-resume-1440.png` });
  } finally {
    await gateway.close();
  }
});
