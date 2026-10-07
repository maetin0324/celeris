// メッセージ表示の e2e（ADR 2026-10-05-cos-chat-home D5・D6）: Markdown・code・表・tool の折りたたみ・
// streaming（text_delta を流して二重表示が無いこと）・確定 message で置き換え。
// 本文は偽 daemon の emit（制御 endpoint）で決定的に流す。

import { expect, test } from "@playwright/test";
import { conversation, expectOnce, makeMessage, startChatGateway, waitForStream } from "./support";

// 表は code fence より前に置く。code fence の直後に表が続くと現在の
// components/content/markdown.tsx（parseBlocks の fence 除去で段落先頭に
// 改行が残る）では表として解析されない既知の不具合がある（別 WU の共有部品。
// 未解決・follow-up を参照）。
const MARKDOWN = [
  "## 結果",
  "",
  "**太字** と `インライン` と [リンク](/console) と <script>alert(1)</script> は文字のまま。",
  "",
  "| 項目 | 値 |",
  "| --- | --- |",
  "| 結果 | 成功 |",
  "| 時間 | 12 s |",
  "",
  "```sh",
  "pnpm test",
  "```",
].join("\n");

test("Markdown・code・表・tool の折りたたみと展開を 1 回だけ表示する", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await expect(section.getByText("進捗を教えて")).toBeVisible();

    // 見出し・太字・インラインコード・link（既存 fixture の ## 進捗 と ```sh）
    await expect(section.getByRole("heading", { name: "進捗" })).toBeVisible();
    await expect(section.locator("code").filter({ hasText: "pnpm test" })).toBeVisible();

    // 表・script 文字列・長い本文を 1 回だけ追加。
    await gateway.emit("chat-main", {
      type: "message",
      data: { message: makeMessage("chat-main", 3, "assistant", MARKDOWN) },
    });
    await expect(section.getByRole("heading", { name: "結果" })).toBeVisible();
    await expect(section.locator("strong").filter({ hasText: "太字" })).toBeVisible();
    await expect(section.getByRole("link", { name: "リンク" })).toHaveAttribute("href", "/console");
    await expect(section).toContainText("<script>alert(1)</script>");
    await expect(section.locator("table")).toHaveCount(1);
    await expect(section.getByRole("cell", { name: "成功" })).toBeVisible();
    // 本文の「1 回だけ」: 追加メッセージの固有の見出しが 1 つ（同一本文の二重表示が無い）。
    await expectOnce(section, "[data-slot='chat-body']", "結果");

    // code block: 言語ラベル付きの scroll 領域（追加メッセージの sh ブロック。
    // fixture のメッセージにも sh があるため、本文「結果」で絞り込む）。
    const newBody = section.locator('[data-slot="chat-body"]').filter({ hasText: "結果" });
    await expect(newBody.getByRole("region", { name: "コード（sh）" })).toContainText("pnpm test");

    // tool: detail がある行は折り畳まれて detail が隠れ、展開で CodeBlock が出る。
    await gateway.emit("chat-main", {
      type: "tool",
      data: {
        call_id: "call-1",
        name: "read",
        summary: "src/app.tsx を読む",
        state: "running",
        detail: "line 1\nline 2",
        error: false,
        truncated: false,
      },
    });
    const toolRow = section.getByRole("button", { name: /read/ });
    await expect(toolRow).toBeVisible();
    await expect(toolRow).toHaveAttribute("aria-expanded", "false");
    await expect(section).not.toContainText("line 1");
    await toolRow.click();
    await expect(toolRow).toHaveAttribute("aria-expanded", "true");
    await expect(section.getByRole("region", { name: "read の詳細" })).toContainText("line 1");
    await expect(section.getByText("実行中", { exact: true })).toBeVisible();

    // tool の完了（error=false）で「成功」に変わる。
    await gateway.emit("chat-main", {
      type: "tool",
      data: {
        call_id: "call-1",
        name: "read",
        summary: "src/app.tsx を読む",
        state: "completed",
        detail: "line 1\nline 2",
        error: false,
        truncated: false,
      },
    });
    // 表の cell「成功」と区別するため、read tool の行だけに絞る。
    const readTool = section.locator('[data-slot="chat-tool"]').filter({ hasText: "src/app.tsx を読む" });
    await expect(readTool.getByText("成功", { exact: true })).toBeVisible();

    // detail がない tool は button にならない（静的な 1 行）。
    await gateway.emit("chat-main", {
      type: "tool",
      data: {
        call_id: "call-2",
        name: "grep",
        summary: "TODO を探す",
        state: "completed",
        error: false,
        truncated: false,
      },
    });
    await expect(section.getByRole("button", { name: /grep/ })).toHaveCount(0);
    await expect(section.locator('[data-slot="chat-tool"]').filter({ hasText: "grep" })).toContainText("成功");

    // 表・code と両 tool の状態を同じ画面に収める。
    await toolRow.click();
    await expect(toolRow).toHaveAttribute("aria-expanded", "false");
    await conversation(page).scroller.evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await expect(readTool.getByText("成功", { exact: true })).toBeVisible();
    await expect(section.locator('[data-slot="chat-tool"]').filter({ hasText: "grep" })).toBeVisible();

    // スクリーンショット: 広い幅（1440 px）の会話（Markdown・表・code・tool 折りたたみ）。
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-content-1440.png` });
  } finally {
    await gateway.close();
  }
});

test("streaming は text_delta を流し、確定 message に置き換えて二重表示にしない", async ({ page }) => {
  const gateway = await startChatGateway();
  const { section } = conversation(page);
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-history`);
    await expect(section.getByText("履歴 240", { exact: true })).toBeVisible();
    await waitForStream(gateway, "chat-history");

    // run を作り status「考え中」を出す。
    const runId = await gateway.hold("chat-history");
    await gateway.emit("chat-history", { type: "status", run_id: runId, data: { phase: "thinking", summary: "" } });
    const statusLine = section.locator('[data-slot="chat-status"]');
    await expect(statusLine).toHaveCount(1);
    await expect(statusLine).toContainText("考え中");

    // 確定 message が出る前に text_delta が 3 回（UTF-8 byte offset で追う）。
    // \n\n は Markdown が段落に切るため、chunk の着いたことを段落本文で確かめる。
    const messageId = "chat-history-m900";
    const bodyWith = (text: string) => section.locator('[data-slot="chat-body"]').filter({ hasText: text });
    for (const [offset, chunk, expectText] of [
      [0, "進捗", "進捗"],
      [6, "はこうです", "進捗はこうです"],
      [21, "。\n\n2 段落", "2 段落"],
    ] as Array<[number, string, string]>) {
      await gateway.emit("chat-history", {
        type: "text_delta",
        run_id: runId,
        message_id: messageId,
        data: { offset, text: chunk },
      });
      await expect.poll(async () => bodyWith(expectText).count(), { timeout: 10_000 }).toBe(1);
    }
    // streaming 中の本文は caret 付きで 1 項目（二重にない）。
    await expect(section.locator('[data-slot="chat-caret"]')).toHaveCount(1);
    await expectOnce(section, '[data-slot="chat-body"]', "進捗はこうです");
    // hold 制御により完了イベントは流れない。生成途中の本文と停止を同時に撮る。
    await section.locator('[data-slot="chat-caret"]').scrollIntoViewIfNeeded();
    await expect(section.getByText("2 段落", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "停止", exact: true })).toBeVisible();
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) await page.screenshot({ path: `${shots}/chat-streaming-1440.png` });

    // run が確定したら、running の message が同じ id で置き換わる（caret 消え・項目 1 つ）。
    await gateway.emit("chat-history", {
      type: "message",
      message_id: messageId,
      run_id: runId,
      data: {
        message: makeMessage("chat-history", 900, "assistant", "進捗はこうです。\n\n2 段落", {
          state: "running",
          run_id: runId,
        }),
      },
    });
    await gateway.emit("chat-history", {
      type: "run",
      run_id: runId,
      data: {
        run: {
          id: runId,
          thread_id: "chat-history",
          input_message_id: "chat-history-m1",
          output_message_id: messageId,
          state: "completed",
          started_at: "2026-10-05T12:00:00Z",
          finished_at: "2026-10-05T12:00:05Z",
        },
      },
    });
    await expect(section.locator('[data-slot="chat-caret"]')).toHaveCount(0);
    // 確定した返りの本文が 1 回だけ（draft と message の二重表示が無い）。
    await expectOnce(section, '[data-slot="chat-body"]', "進捗はこうです。");
    // status 行は run の確定で消える。
    await expect(statusLine).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});
