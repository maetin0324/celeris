// 会話一覧の e2e（ADR 2026-10-22-cos-fixed-threads D2・D5）: 統括 CoS が既定、案件 CoS の切替・数・検索・旧会話の保管・?thread= の再開。
// 偽 daemon の fixture（rich profile）を使う。操作はすべて UI 経由で、状態確認は expect.poll まで。

import { expect, test } from "@playwright/test";
import { conversation, startChatGateway } from "./support";

test("統括 CoS を既定で開き、案件 CoS へ切り替え、検索で見つけ、旧会話だけを保管できる", async ({ page }) => {
  const gateway = await startChatGateway();
  const { composer } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/`);
    // URL に会話が無ければ統括 CoS（fixture の chat-main）を選ぶ。新しい会話は作らない。
    await expect(page).toHaveURL(/\?thread=chat-main$/);
    const sidebar = page.locator("#chat-thread-sidebar");
    const threadButton = (name: string) => sidebar.getByRole("button", { name, exact: true });
    await expect(threadButton("CoS と相談")).toBeVisible();
    await expect(sidebar.getByRole("button", { name: "新しい会話" })).toHaveCount(0);
    // 案件 CoS の要対応の数が出る。
    await expect(sidebar.locator('[data-thread-kind="project"]').getByText("要対応 2")).toBeVisible();

    // 切替: 案件 CoS を選ぶと ?thread= が変わる。
    await threadButton("長い会話の確認").click();
    await expect(page).toHaveURL(/\?thread=chat-history$/);
    await expect(page.getByText("履歴 240", { exact: true })).toBeVisible();

    // 検索: 本文にだけ含む語（履歴 240）で絞り込まれ、他は消える。
    const search = sidebar.getByRole("searchbox");
    await search.fill("履歴");
    await expect(threadButton("長い会話の確認")).toBeVisible();
    await expect(threadButton("CoS と相談")).toBeHidden();
    await search.fill("");
    await expect(threadButton("CoS と相談")).toBeVisible();

    // CoS の会話は保管できず、旧会話だけが保管（archive）できる。
    await expect(sidebar.getByRole("button", { name: "CoS と相談をアーカイブ" })).toHaveCount(0);
    await sidebar.getByRole("button", { name: "旧 Console の会話をアーカイブ" }).click();
    await expect(threadButton("旧 Console の会話")).toBeHidden();
    const archived = await (await fetch(`${gateway.base}/api/chat/threads?status=archived`)).json();
    expect((archived.items as Array<{ id: string }>).map((item) => item.id)).toContain("chat-legacy");

    await expect(composer).toBeVisible();
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-threads-1440.png` });
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

test("一覧の badge（種類・要対応）は一行に収まる", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    const sidebar = page.locator("#chat-thread-sidebar");
    const row = sidebar.locator('[data-thread-kind="project"]').filter({ hasText: "要対応 2" });
    const button = row.getByRole("button", { name: "受信箱の一次対応", exact: true });
    await expect(button).toBeVisible();
    const rowBox = await button.boundingBox();
    if (!rowBox) throw new Error("会話行の矩形が取得できません");
    for (const badge of await button.locator('[data-slot="badge"]').all()) {
      const box = await badge.boundingBox();
      if (!box) throw new Error("badge の矩形が取得できません");
      expect(box.height).toBeLessThan(box.width);
      expect(box.height).toBeLessThanOrEqual(rowBox.height);
    }
  } finally {
    await gateway.close();
  }
});
