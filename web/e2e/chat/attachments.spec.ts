// ADR 2026-10-05-cos-chat-home D6: 添付の入力経路と upload queue の送信 gate。
import { expect, type Page, test } from "@playwright/test";
import { type ChatGateway, conversation, startChatGateway, waitForStream } from "./support";

const png = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/lXcAAAAASUVORK5CYII=",
  "base64",
);
const files = (page: Page) => page.getByRole("list", { name: "添付ファイル", exact: true }).getByRole("listitem");
const send = (page: Page) => page.getByRole("button", { name: "送信", exact: true });
const item = (page: Page, name: string) => files(page).filter({ hasText: name });

async function transfer(page: Page, kind: "drop" | "paste", name: string) {
  await page.evaluate(
    ({ kind, name, bytes }) => {
      const data = new DataTransfer();
      data.items.add(new File([new Uint8Array(bytes)], name, { type: "image/png" }));
      const target = document.querySelector(
        kind === "drop" ? 'section[aria-label="メッセージ入力"]' : 'textarea[aria-label="CoS へのメッセージ"]',
      );
      if (!target) throw new Error("composer missing");
      target.dispatchEvent(
        kind === "drop"
          ? new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: data })
          : new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: data }),
      );
    },
    { kind, name, bytes: [...png] },
  );
}

async function pending(gateway: ChatGateway, name: string) {
  await expect.poll(async () => (await gateway.pendingUploads()).some((upload) => upload.name === name)).toBe(true);
  const upload = (await gateway.pendingUploads()).find((upload) => upload.name === name);
  if (!upload) throw new Error(`missing upload: ${name}`);
  return upload.id;
}

const messages = async (gateway: ChatGateway) =>
  (await gateway.requests())
    .filter((request) => request.method === "POST" && request.path === "/api/v1/chat/threads/chat-main/messages")
    .map((request) => JSON.parse(request.body ?? "{}"));

test("ボタン・drop・画像 paste が同じキューに入り、upload 中の取消と進捗が表示される", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await gateway.setUploadState("hold");
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await waitForStream(gateway, "chat-main");
    const chooser = page.waitForEvent("filechooser");
    await page.getByRole("button", { name: "ファイルを添付", exact: true }).click();
    await (await chooser).setFiles({ name: "note.txt", mimeType: "text/plain", buffer: Buffer.from("hello") });
    await transfer(page, "drop", "dropped.png");
    await transfer(page, "paste", "pasted.png");
    await expect(files(page)).toHaveCount(3);
    await expect.poll(async () => (await gateway.pendingUploads()).length).toBe(3);
    for (const name of ["note.txt", "dropped.png", "pasted.png"]) {
      const progress = item(page, name).getByRole("progressbar", { name: `${name} のアップロード` });
      await expect(progress).toBeVisible();
      await expect(progress).toHaveAttribute("max", "100");
      // XHR が実送信した bytes の進捗。応答保留中も ready にはしない。
      await expect(progress).toHaveAttribute("value", "100");
    }
    for (const name of ["dropped.png", "pasted.png"]) {
      await expect(item(page, name).locator("img")).toBeVisible();
      await expect
        .poll(() =>
          item(page, name)
            .locator("img")
            .evaluate((img: HTMLImageElement) => img.naturalWidth),
        )
        .toBe(1);
    }
    await expect(send(page)).toBeDisabled();
    await conversation(page).input.fill("完了前には送れない");
    await conversation(page).input.press("Enter");
    expect(await messages(gateway)).toHaveLength(0);

    await item(page, "note.txt").getByRole("button", { name: "note.txt を取り消す", exact: true }).click();
    await expect(files(page)).toHaveCount(2);
    await expect(item(page, "note.txt")).toHaveCount(0);
    await expect
      .poll(async () => (await gateway.pendingUploads()).some((upload) => upload.name === "note.txt"))
      .toBe(false);
    for (const name of ["dropped.png", "pasted.png"]) {
      await gateway.releaseUpload(await pending(gateway, name), "succeed");
      await expect(item(page, name).getByRole("progressbar")).toHaveCount(0);
    }
    await send(page).click();
    await expect.poll(async () => (await messages(gateway)).length).toBe(1);
    expect((await messages(gateway))[0]).toMatchObject({
      text: "完了前には送れない",
      attachment_ids: expect.any(Array),
    });
    expect((await messages(gateway))[0].attachment_ids).toHaveLength(2);
    await expect(files(page)).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});

test("失敗は表示され送信を止め、同じ upload id の再試行成功後は画像だけ送れる", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await gateway.setUploadState("hold");
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await page.locator('input[type="file"][aria-label="添付ファイルを選択"]').setInputFiles({
      name: "retry.png",
      mimeType: "image/png",
      buffer: png,
    });
    const id = await pending(gateway, "retry.png");
    await expect(send(page)).toBeDisabled();
    await gateway.releaseUpload(id, "fail");
    await expect(item(page, "retry.png").getByRole("alert")).toContainText("失敗");
    await expect(item(page, "retry.png").getByRole("progressbar")).toHaveCount(0);
    await expect(send(page)).toBeDisabled();
    await conversation(page).input.fill("失敗中も送れない");
    await expect(send(page)).toBeDisabled();
    await conversation(page).input.press("Enter");
    expect(await messages(gateway)).toHaveLength(0);
    await conversation(page).input.fill("");
    await item(page, "retry.png").getByRole("button", { name: "retry.png を再試行", exact: true }).click();
    expect(await pending(gateway, "retry.png")).toBe(id);
    await expect(files(page)).toHaveCount(1);
    await expect(item(page, "retry.png").getByRole("alert")).toHaveCount(0);
    await expect(item(page, "retry.png").getByRole("progressbar")).toBeVisible();
    await expect(send(page)).toBeDisabled();
    await gateway.releaseUpload(id, "succeed");
    await expect(item(page, "retry.png").getByRole("progressbar")).toHaveCount(0);
    await expect(send(page)).toBeEnabled();
    await expect(conversation(page).input).toHaveValue("");
    await send(page).click();
    await expect.poll(async () => (await messages(gateway)).length).toBe(1);
    const body = (await messages(gateway))[0];
    expect(body.text).toBe("");
    expect(body.attachment_ids).toHaveLength(1);
    // daemon が受理した message の添付も同じ id（要求だけでなく永続 fixture を確認）。
    const response = await fetch(`${gateway.base}/api/chat/threads/chat-main/messages`);
    expect(response.ok).toBe(true);
    const history = await response.json();
    expect(history.items.at(-1)).toMatchObject({ text: "", attachment_ids: body.attachment_ids });
    await expect(files(page)).toHaveCount(0);
    await expect(send(page)).toBeDisabled();
  } finally {
    await gateway.close();
  }
});
