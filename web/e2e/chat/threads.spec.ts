// 会話一覧の e2e（ADR 2026-10-05-cos-chat-home D5・D6）: 新規・題名変更・検索・archive・?thread= の再開。
// 偽 daemon の fixture（rich profile）を使う。操作はすべて UI 経由で、状態確認は expect.poll まで。

import { expect, test } from "@playwright/test";
import { conversation, startChatGateway } from "./support";

test("新しい会話を作る、題名を変える、検索で見つけ、保管で一覧から消す", async ({ page }) => {
  const gateway = await startChatGateway();
  const { composer } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    // 一覧の容器（desktop の aside。mobile の drawer と id が違う）。
    const sidebar = page.locator("#chat-thread-sidebar");
    // 題名ボタンは exact（「…の題名を変更」「…をアーカイブ」の aria-label 部分一致を避ける）。
    const threadButton = (name: string) => sidebar.getByRole("button", { name, exact: true });
    await expect(threadButton("CoS と相談")).toBeVisible();

    // 新規: 新しい会話 → ?thread=chat-new-1 へ選び、空の会話が描かれる。
    await sidebar.getByRole("button", { name: "新しい会話" }).click();
    await expect(page).toHaveURL(/\?thread=chat-new-1$/);
    const fresh = page.getByText("CoS にメッセージを送って会話を始めましょう。");
    await expect(fresh).toBeVisible();

    // 題名変更: 一覧の「編集」で form になり、保存で反映（revision を通す）。
    await sidebar.getByRole("button", { name: "新しい会話の題名を変更" }).click();
    const formTitle = sidebar.getByRole("textbox", { name: "会話の題名" });
    await expect(formTitle).toHaveValue("新しい会話");
    await formTitle.fill("e2e の相談");
    await sidebar.getByRole("button", { name: "保存", exact: true }).click();
    await expect
      .poll(async () => (await (await fetch(`${gateway.base}/api/chat/threads/chat-new-1`)).json()).thread.title)
      .toBe("e2e の相談");
    await expect(threadButton("e2e の相談")).toBeVisible();

    // 検索: 本文にだけ含む語（履歴 240）で絞り込まれ、他は消える。
    const search = sidebar.getByRole("searchbox");
    await search.fill("履歴");
    await expect(threadButton("長い会話の確認")).toBeVisible();
    await expect(threadButton("e2e の相談")).toBeHidden();
    await search.fill("");
    await expect(threadButton("e2e の相談")).toBeVisible();

    // 保管（archive）: 一覧から消え、status=archived で API に残る。
    await sidebar.getByRole("button", { name: "e2e の相談をアーカイブ" }).click();
    await expect(threadButton("e2e の相談")).toBeHidden();
    const archived = await (await fetch(`${gateway.base}/api/chat/threads?status=archived`)).json();
    expect((archived.items as Array<{ id: string }>).map((item) => item.id)).toContain("chat-new-1");

    // composer は空の会話のまま残る（一覧の操作で会話の選択は変わらない）。
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
