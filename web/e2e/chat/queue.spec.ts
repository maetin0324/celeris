// 停止・キュー・割り込み・入力（Enter/Shift+Enter/IME）の e2e（ADR 2026-10-05-cos-chat-home D2・D5・D6）。
// run の状態は偽 daemon の emit（run event）で決定的に進める（worker の進行は試験しない）。

import { expect, test } from "@playwright/test";
import { conversation, expectComposerStatus, startChatGateway, waitForStream } from "./support";

const bodyOf = (requests: Array<{ path: string; method: string; body?: string }>, threadId: string) =>
  requests
    .filter((r) => r.method === "POST" && r.path === `/api/v1/chat/threads/${threadId}/messages`)
    .map((r) => JSON.parse(r.body ?? "{}") as Record<string, unknown>);

test("Enter で送信、Shift+Enter は改行だけ、IME composition 中の Enter は送信しない", async ({ page }) => {
  const gateway = await startChatGateway();
  const { composer, input } = conversation(page);
  try {
    await page.goto(`${gateway.base}/?thread=chat-legacy`);
    await expect(composer).toBeVisible();
    await waitForStream(gateway, "chat-legacy");
    const sent = async () => bodyOf(await gateway.requests(), "chat-legacy");

    // Shift+Enter: 改行だけで送らない。
    await input.fill("一行目");
    await input.press("Shift+Enter");
    await expect(input).toHaveValue("一行目\n");
    await expect.poll(sent).toHaveLength(0);

    // Enter: 送信（queue）。
    await input.press("Enter");
    await expect.poll(sent, { timeout: 10_000 }).toHaveLength(1);
    expect((await sent())[0].text).toBe("一行目\n");
    await expect(input).toHaveValue("");

    // IME composition 中の Enter（keyCode 229・isComposing）は送信しない。
    // 実 IME の確定キーは改行を挿入しないため、改行なしの keydown を dispatch して再現する
    // （生の Enter は browser の既定動作で改行を足す artifact になる）。
    await input.click();
    await page.evaluate(() => {
      const target = document.activeElement as HTMLTextAreaElement;
      target.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    });
    await page.keyboard.insertText("し");
    await page.evaluate(() => {
      const target = document.activeElement as HTMLTextAreaElement;
      // keyCode / isComposing は標準 constructor の init に無いので defineProperty で載せる。
      const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
      Object.defineProperties(enter, { isComposing: { value: true }, keyCode: { value: 229 } });
      target.dispatchEvent(enter);
    });
    await expect.poll(sent).toHaveLength(1);
    await page.evaluate(() => {
      const target = document.activeElement as HTMLTextAreaElement;
      target.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: "し" }));
    });
    // composition 中の Enter が改行を足していないことを確認してから送る。
    await expect(input).toHaveValue("し");
    await input.press("Enter");
    await expect.poll(sent, { timeout: 10_000 }).toHaveLength(2);
    expect((await sent())[1].text).toBe("し");
  } finally {
    await gateway.close();
  }
});

test("キューに追加された順に表示し、取消で消す", async ({ page }) => {
  const gateway = await startChatGateway();
  const { input } = conversation(page);
  try {
    await page.goto(`${gateway.base}/?thread=chat-legacy`);
    await expect(input).toBeVisible();
    await input.fill("1 件目");
    await input.press("Enter");
    await input.fill("2 件目");
    await input.press("Enter");
    const queue = page.getByRole("list", { name: "送信待ち" });
    await expect(queue.getByRole("listitem").nth(0)).toContainText("1. 1 件目");
    await expect(queue.getByRole("listitem").nth(1)).toContainText("2. 2 件目");

    // 取消: 先頭を取り消すと 1 件になる。
    await queue.getByRole("button", { name: "1番目を取り消す" }).click();
    await expect(queue.getByRole("listitem")).toHaveCount(1);
    await expect(queue).toContainText("2 件目");
  } finally {
    await gateway.close();
  }
});

test("停止でキューが停止し、停止中の送信は resume_queue で再開する", async ({ page }) => {
  const gateway = await startChatGateway();
  const { input } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-legacy`);
    await gateway.hold("chat-legacy");
    await waitForStream(gateway, "chat-legacy");

    // 実行中に 1 件キューへ。
    await input.fill("待ち 1");
    await input.press("Enter");
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("1. 待ち 1");

    // 停止: run は stopped、キューは停止（停止中の送信待ち＋再開ボタン）。
    await page.getByRole("button", { name: "停止" }).click();
    await expectComposerStatus(page, "キューを停止中です");
    await expect(page.getByRole("list", { name: "停止中の送信待ち" })).toContainText("1. 待ち 1");
    await expect(page.getByRole("button", { name: "キューを再開" })).toBeVisible();
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-queue-paused-1440.png` });

    // 停止中の送信は resume_queue=true（明示再開）が付き、キューへ並ぶ。
    await input.fill("復旧");
    await input.press("Enter");
    const sent = async () => bodyOf(await gateway.requests(), "chat-legacy");
    await expect.poll(async () => (await sent()).length, { timeout: 10_000 }).toBe(2);
    expect((await sent()).at(-1)).toMatchObject({ text: "復旧", mode: "queue", resume_queue: true });
    // 再開してキュー 2 件が元の順番で表示される。
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("1. 待ち 1");
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("2. 復旧");
    await expect(page.getByRole("button", { name: "キューを再開" })).toBeHidden();
  } finally {
    await gateway.close();
  }
});

test("停止のあと「キューを再開」でキューを再開できる", async ({ page }) => {
  const gateway = await startChatGateway();
  const { input } = conversation(page);
  try {
    await page.goto(`${gateway.base}/?thread=chat-legacy`);
    await gateway.hold("chat-legacy");
    await waitForStream(gateway, "chat-legacy");
    await input.fill("停止待ち");
    await input.press("Enter");
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("停止待ち");

    await page.getByRole("button", { name: "停止" }).click();
    await expect(page.getByRole("list", { name: "停止中の送信待ち" })).toBeVisible();

    // キューを再開: queue event paused=false（API の thread も unpause される）。
    await page.getByRole("button", { name: "キューを再開" }).click();
    await expect
      .poll(
        async () => (await (await fetch(`${gateway.base}/api/chat/threads/chat-legacy`)).json()).thread.queue_paused,
        { timeout: 10_000 },
      )
      .toBe(false);
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("停止待ち");
    await expect(page.getByRole("button", { name: "キューを再開" })).toBeHidden();
  } finally {
    await gateway.close();
  }
});

test("割り込み送信は mode=interrupt で active run を stopping にする", async ({ page }) => {
  const gateway = await startChatGateway();
  const { input } = conversation(page);
  try {
    await page.goto(`${gateway.base}/?thread=chat-legacy`);
    await gateway.hold("chat-legacy");
    await waitForStream(gateway, "chat-legacy");
    await input.fill("急ぎです");
    await expect(page.getByRole("button", { name: "割り込んで送信" })).toBeEnabled();
    await page.getByRole("button", { name: "割り込んで送信" }).click();
    const sent = async () => bodyOf(await gateway.requests(), "chat-legacy");
    await expect.poll(async () => (await sent()).length, { timeout: 10_000 }).toBe(1);
    expect((await sent()).at(-1)).toMatchObject({ text: "急ぎです", mode: "interrupt" });
    // 割り込みで active run は stopping（キューは止めない）。
    await expect
      .poll(
        async () => (await (await fetch(`${gateway.base}/api/chat/threads/chat-legacy`)).json()).active_run?.state,
        {
          timeout: 10_000,
        },
      )
      .toBe("stopping");
    await expectComposerStatus(page, "停止を要求中です");
  } finally {
    await gateway.close();
  }
});
