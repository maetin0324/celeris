// 再接続と scroll の e2e（ADR 2026-10-05-cos-chat-home D2・D5・D6）:
// - 再読込で cursor（Last-Event-ID）から続きを読み、既読 event を二重に適用しない。
// - stream 切断の再接続、410（cursor 失効）で snapshot 取り直し。
// - 上へ scroll 中は位置を保ち「最新へ」と未読件数が出る。押すと下端へ。

import { expect, test } from "@playwright/test";
import {
  conversation,
  expectComposerStatus,
  expectOnce,
  isAtBottom,
  makeMessage,
  startChatGateway,
  waitForStream,
} from "./support";

const streamRequests = (
  requests: Array<{ path: string; method: string; lastEventId?: string | null; query?: string }>,
) => requests.filter((r) => r.method === "GET" && r.path === "/api/v1/chat/threads/chat-main/stream");

test("再読込は cursor から続きを読み、古い event を二重に適用しない", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await expect(section.getByText("進捗を教えて")).toBeVisible();
    await waitForStream(gateway, "chat-main");

    // 既読（fixture の snapshot で 2 件の message event がある）の上に新着を 1 件足す。
    const firstId = await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 3, "user", "再読込の確認") },
    });
    await expect(section.getByText("再読込の確認")).toBeVisible();

    // 再読込: snapshot（message 3 件）+ cursor 以降の replay。新着は 1 回だけ表示。
    await page.reload();
    await waitForStream(gateway, "chat-main");
    await expect(section.getByText("再読込の確認")).toBeVisible();
    await expectOnce(section, '[data-slot="chat-body"]', "再読込の確認");

    // 2 番目の stream 接続は cursor（再読込の snapshot = 読んだ最後の event id）から再開する。
    // 古い event（firstId 以下）は取り直さない（二重適用しない）。
    await expect.poll(async () => streamRequests(await gateway.requests()).length, { timeout: 10_000 }).toBe(2);
    const cursor = streamRequests(await gateway.requests()).at(-1);
    expect(cursor?.lastEventId).toBe(firstId);
  } finally {
    await gateway.close();
  }
});

test("再読込で 2 つ目の stream 接続が cursor から継ぐ（Last-Event-ID と after が一致）", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section } = conversation(page);
  try {
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await waitForStream(gateway, "chat-main");
    const connections = async () => streamRequests(await gateway.requests()).length;
    await expect.poll(connections, { timeout: 10_000 }).toBe(1);

    // 新着 1 件（event id 3）を cursor に反映してから再読込。
    const firstId = await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 3, "user", "再読込の確認") },
    });
    await expect(section.getByText("再読込の確認")).toBeVisible();
    await expectOnce(section, '[data-slot="chat-body"]', "再読込の確認");

    // 2 つ目の接続（再読込）は Last-Event-ID と ?after= に同じ cursor を載せる（不一致は 400 になる）。
    await page.reload();
    await expect(section.getByText("進捗を教えて")).toBeVisible();
    await expect.poll(connections, { timeout: 10_000 }).toBe(2);
    const [, second] = streamRequests(await gateway.requests());
    // 再読込の snapshot（最新 event id = 新着）から続き：applied した最後の id と一致する。
    expect(second.lastEventId).toBe(firstId);
    const query = new URLSearchParams(second.query ?? "");
    expect(query.get("after")).toBe(firstId);
  } finally {
    await gateway.close();
  }
});

test("stream 欠落のあと cursor が 410 で失効したら、snapshot を取り直して続きから読む", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await expect(section.getByText("進捗を教えて")).toBeVisible();
    await waitForStream(gateway, "chat-main");

    // 切断: 偽 daemon 側が SSE を切る（offline だけでは live の接続が切れないため）。
    // 張り直しは指数 backoff（base 1 s）で、その間 cursor は最初の 0 のままだ。
    const cut = await gateway.disconnect();
    expect(cut).toBeGreaterThan(0);
    await expectComposerStatus(page, "接続が切れています。再接続を試みています");

    // 切断中に新着 2 件と cursor の失効を入れる（cursor 0 に対して 410）。
    await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 3, "user", "失効する前") },
    });
    await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 4, "user", "失効のあとの 1") },
    });
    await gateway.expire("chat-main", "1");

    // 張り直し（cursor 0）が 410 → snapshot 取り直し → 取り直した cursor（最新 = 2）で接続。
    // 失効中に届いた 2 件は snapshot の messages から表示される（二重にしない）。
    await expect(section.getByText("失効する前")).toBeVisible();
    await expect(section.getByText("失効のあとの 1")).toBeVisible();
    await expectOnce(section, '[data-slot="chat-body"]', "失効する前");
    await expect(page.getByText("接続が切れています。再接続を試みています")).toHaveCount(0);
    // 取り直した cursor（snapshot の最新 event id）で再接続している。
    await expect
      .poll(async () => streamRequests(await gateway.requests()).at(-1)?.lastEventId, { timeout: 10_000 })
      .toBe("2");
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-resync-1440.png` });

    // 取り直した cursor から新着も継げる。
    await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 5, "user", "失効のあとの 2") },
    });
    await expect(section.getByText("失効のあとの 2")).toBeVisible();
  } finally {
    await gateway.close();
  }
});

test("上へ scroll 中は位置を保ち、未読と「最新へ」が出て、押すと下端へ戻る", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section, scroller } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-history`);
    await expect(section.getByText("履歴 240", { exact: true })).toBeVisible();

    // 下端（追従中）→ 上へ scroll で「最新へ」が出る。
    const metrics = () =>
      scroller.evaluate((el) => ({
        scrollTop: el.scrollTop,
        scrollHeight: el.scrollHeight,
        clientHeight: el.clientHeight,
      }));
    await expect.poll(async () => isAtBottom(await metrics()), { timeout: 10_000 }).toBe(true);
    await scroller.evaluate((el) => el.scrollTo(0, 0));
    // scroll の event が届いてから新着を出す（console の e2e と同じ 2 frame）。
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const jump = page.getByRole("button", { name: /最新へ/ });
    await expect(jump).toBeVisible();

    // 上を見ている間に新着 2 件：位置は保たれ、未読件数は 2 になる。
    const anchor = await scroller.evaluate((el) => el.scrollTop);
    for (const [seq, text] of [
      [241, "新着 A"],
      [242, "新着 B"],
    ] as Array<[number, string]>) {
      await gateway.emit("chat-history", {
        type: "message",
        data: { message: makeMessage("chat-history", seq, "assistant", text) },
      });
      await expect(section.getByText(text, { exact: true })).toBeVisible();
    }
    // 未読件数は aria-label（「最新へ（未読 2 件）」）に出る。
    await expect(jump).toHaveAttribute("aria-label", "最新へ（未読 2 件）");
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-jump-latest-1440.png` });
    await expect
      .poll(async () => (await scroller.evaluate((el) => el.scrollTop)) - anchor, { timeout: 10_000 })
      .toBe(0);

    // 「最新へ」で下端へ。未読が消える。
    await jump.click();
    await expect.poll(async () => isAtBottom(await metrics()), { timeout: 10_000 }).toBe(true);
    await expect(jump).toBeHidden();
    await expect(section.getByText("新着 B", { exact: true })).toBeVisible();
  } finally {
    await gateway.close();
  }
});

test("古い頁を足しても見ている位置を保つ（before_seq）", async ({ page }) => {
  const gateway = await startChatGateway();
  const { scroller, section } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-history`);
    await expect(scroller.getByRole("button", { name: "以前のメッセージ" })).toBeVisible();

    // 下端から少し上まで scroll し、古い頁を読む。足したぶんだけずれて同じ内容が見える。
    await scroller.evaluate((el) => {
      el.scrollTop = 200;
      el.dispatchEvent(new Event("scroll", { bubbles: true }));
    });
    const before = await scroller.evaluate((el) => el.scrollTop);
    const visible = await scroller.locator("[data-key]").first().getAttribute("data-key");
    await scroller.getByRole("button", { name: "以前のメッセージ" }).click();
    await expect
      .poll(
        async () =>
          (await scroller.locator("[data-key]").first().getAttribute("data-key")) !== visible ||
          (await scroller.evaluate((el) => el.scrollTop)) > before,
        { timeout: 10_000 },
      )
      .toBe(true);
    // 古い message（seq 171〜190）が上に加わっている。
    await expect(section.getByText("履歴 180", { exact: true })).toBeVisible();
    // 見ている位置は保たれる（anchor よりも高い scrollTop）。
    const after = await scroller.evaluate((el) => el.scrollTop);
    expect(after).toBeGreaterThan(before);
  } finally {
    await gateway.close();
  }
});
