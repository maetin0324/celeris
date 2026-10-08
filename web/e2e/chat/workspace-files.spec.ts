// ADR 2026-10-08-cos-workspace-files-in-chat D3: CoS が thread workspace に書いた md が返事の添付として届き、
// 返事の本文の workspace path が link になる。link を押すと画面遷移せずに md を描画して開く。
// 添付は偽 daemon の upload API に直接（FIXTURE_TOKEN）置き、返事は emit で決定的に流す。

import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { conversation, makeMessage, startChatGateway, waitForStream } from "./support";

const ABS = "/local/celeris/data/db/cos/threads/chat-main/workspace";
const SETUP = ["# manaba 監視の始め方", "", "1. 「受信箱」画面を開く", "2. 「回答する」を押す"].join("\n");

async function upload(daemonUrl: string, name: string, type: string, body: string): Promise<string> {
  const form = new FormData();
  form.append("client_upload_id", `ws-${name}`);
  form.append("file", new Blob([body], { type }), name);
  const response = await fetch(`${daemonUrl}/api/v1/chat/threads/chat-main/attachments`, {
    method: "POST",
    headers: { authorization: `Bearer ${FIXTURE_TOKEN}` },
    body: form,
  });
  if (!response.ok) throw new Error(`upload ${name} failed: ${response.status}`);
  return ((await response.json()) as { attachment: { id: string } }).attachment.id;
}

for (const folded of [false, true]) {
  test(`CoS の workspace の md が${folded ? "折りたたんだ通知" : "返事"}の添付になり、本文の path から描画して開ける`, async ({
    page,
  }) => {
    const gateway = await startChatGateway();
    const { section } = conversation(page);
    try {
      await page.goto(`${gateway.base}/?thread=chat-main`);
      await waitForStream(gateway, "chat-main");
      const md = await upload(gateway.daemonUrl, "manaba-monitor-setup.md", "text/markdown", SETUP);
      const pdf = await upload(gateway.daemonUrl, "reference.pdf", "application/pdf", "%PDF-1.4\nfixture\n");
      const csv = await upload(gateway.daemonUrl, "table.csv", "text/csv", "course,due\nA,10/9\n");
      await gateway.emit("chat-main", {
        type: "message",
        data: {
          message: makeMessage(
            "chat-main",
            3,
            "assistant",
            `結論: 下の手順を開いてください。\n\n詳しくは \`${ABS}/artifacts/manaba-monitor-setup.md\` と table.csv に書きました。[手順を開く](artifacts/manaba-monitor-setup.md)。reference.pdf も参照。`,
            {
              cards: folded
                ? [
                    {
                      kind: "notice",
                      id: "notice-files",
                      title: "文書の知らせ",
                      state: "observed",
                      href: "/inbox",
                      actor: "cos",
                      reason: null,
                      operation_id: null,
                    },
                  ]
                : [],
              attachment_ids: [md, csv, pdf],
              workspace_files: [
                { path: "artifacts/manaba-monitor-setup.md", attachment_id: md },
                { path: "table.csv", attachment_id: csv },
                { path: "reference.pdf", attachment_id: pdf },
              ],
            },
          ),
        },
      });
      if (folded) {
        const details = section.locator('[data-slot="chat-notice-folded"]');
        await expect(details).not.toHaveAttribute("open");
        await details.locator("summary").click();
      }
      const link = section.getByRole("link", { name: `${ABS}/artifacts/manaba-monitor-setup.md` });
      await expect(link).toHaveAttribute("href", `/api/chat/attachments/${md}/content`);
      const attachments = section.getByRole("list", { name: "添付", exact: true });
      await expect(attachments.getByRole("link", { name: /^manaba-monitor-setup\.md.*をダウンロード$/ })).toBeVisible();

      const url = page.url();
      await link.click();
      expect(page.url()).toBe(url);
      const item = section.locator(`#chat-attachment-${md}`);
      await expect(item.getByRole("heading", { name: "manaba 監視の始め方" })).toBeVisible();
      await expect(item.getByText("「受信箱」画面を開く")).toBeVisible();
      await expect(item.getByRole("button", { name: /本文を閉じる/ })).toHaveAttribute("aria-expanded", "true");

      await item.getByRole("button", { name: /本文を閉じる/ }).click();
      await section.getByRole("link", { name: "手順を開く", exact: true }).click();
      await expect(item.getByRole("heading", { name: "manaba 監視の始め方" })).toBeVisible();

      // 画面内表示の無い PDF の本文リンクは download として動く。
      const downloaded = page.waitForEvent("download");
      await section.getByRole("link", { name: "reference.pdf", exact: true }).click();
      expect((await downloaded).suggestedFilename()).toBe("reference.pdf");

      // csv は「本文をここで見る」で行番号付きの text として開く。
      await section.getByRole("link", { name: "table.csv", exact: true }).click();
      await expect(section.locator(`#chat-attachment-${csv}`).getByText("course,due")).toBeVisible();
    } finally {
      await gateway.close();
    }
  });
}
