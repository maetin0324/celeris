// モバイル（320 px / 390 px）の e2e（ADR 2026-10-05-cos-chat-home D6）。
// 横溢れ 0・44 px 操作領域・drawer（会話一覧）の開閉と focus 復元・下部タブバーと composer の共存・
// visualViewport の縮小（キーボード）でも入力/送信/停止が見えること。
// 44 px と横溢れの検査は mobile-audit.mjs と同じ方式（画面内 evaluate で box を測る）。

import { expect, type Locator, type Page, test } from "@playwright/test";
import { conversation, startChatGateway, waitForStream } from "./support";

/** 画面内の制御要素の 44 px 未満と横溢れを測る（mobile-audit.mjs と同じ評価）。 */
async function audit(page: Page) {
  return page.evaluate(() => {
    const doc = document.documentElement;
    const small: string[] = [];
    for (const el of document.querySelectorAll(
      "a, button, input:not([type=hidden]), select, textarea, [role=button]",
    )) {
      const box = el.getBoundingClientRect();
      if (box.width === 0 && box.height === 0) continue;
      if (box.width < 44 || box.height < 44)
        small.push(
          `${el.tagName.toLowerCase()} ${el.getAttribute("aria-label") ?? el.textContent?.slice(0, 20) ?? ""} ${Math.round(box.width)}x${Math.round(box.height)}`,
        );
    }
    return { overflow: doc.scrollWidth - doc.clientWidth, small };
  });
}

// 青と黄色の格子（96×64）。1px の白い fixture では preview の見た目を確認できない。
const COLORED_PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAGAAAABACAIAAABqVuVZAAAAl0lEQVR4nO3YwQmAMBQFwfTi2dYszCKsyRIsIQQvGxh45yWZ4x/H9Sztvc+l7d4fu38AEKB2HxAgQIAAhfuAAAECBCjcBwToJ1DtQbU+IECAAAEK9wEBAgQIULgPCBAgQIDCfQezyQABAgQIULgPCBAgQIDCfUCAAAECFO47mE0GCBAgQIDCfUCAAAECFO4DAgQIEKBw/wOPPwosxBMoPAAAAABJRU5ErkJggg==",
  "base64",
);

test("320 px と 390 px で横溢れは 0、操作領域は 44 px 以上", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    for (const width of [320, 390]) {
      await page.setViewportSize({ width, height: 700 });
      await page.goto(`${gateway.base}/?thread=chat-main`);
      await expect(page.getByRole("region", { name: "メッセージ入力" })).toBeVisible();
      await expect(page.locator('article[data-card-kind="decision"] button').first()).toBeVisible();
      const result = await audit(page);
      expect(result.small, `${width}px の 44px 未満の制御`).toEqual([]);
      expect(result.overflow, `${width}px の横溢れ`).toBe(0);
    }
  } finally {
    await gateway.close();
  }
});

for (const width of [320, 390]) {
  test(`${width} px: 狭い幅では会話一覧は drawer になり、開閉と Escape の focus 復元が成り立つ`, async ({ page }) => {
    const gateway = await startChatGateway();
    try {
      await page.setViewportSize({ width, height: 700 });
      await page.goto(`${gateway.base}/?thread=chat-main`);
      const trigger = page.getByRole("button", { name: "会話一覧を開く" });
      await expect(trigger).toBeVisible();

      // 開く: dialog（会話一覧）が出る。
      await trigger.click();
      const dialog = page.getByRole("dialog", { name: "会話一覧" });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("button", { name: "CoS と相談", exact: true })).toBeVisible();
      const shots = process.env.CHAT_SHOT_DIR;
      if (shots && width === 320) await page.screenshot({ path: `${shots}/chat-drawer-320.png` });
      expect(await audit(page)).toEqual({ overflow: 0, small: [] });
      await dialog.getByRole("button", { name: "閉じる", exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(trigger).toBeFocused();
      await trigger.click();
      await expect(dialog).toBeVisible();

      // Escape: 閉じて focus は trigger に戻る。
      await page.keyboard.press("Escape");
      await expect(dialog).toBeHidden();
      await expect
        .poll(async () => await page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? ""), {
          timeout: 5_000,
        })
        .toBe("会話一覧を開く");

      // 項目を選ぶと drawer は閉じ、会話の選択は ?thread= に残る。
      await trigger.click();
      await expect(dialog).toBeVisible();
      await dialog.getByRole("button", { name: "長い会話の確認", exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect(page).toHaveURL(/\?thread=chat-history$/);
      const result = await audit(page);
      expect(result.overflow).toBe(0);
      expect(result.small).toEqual([]);
    } finally {
      await gateway.close();
    }
  });
}

test("下部タブバーと composer は共存し、composer はタブバーの上に収まる", async ({ page }) => {
  const gateway = await startChatGateway();
  const { composer, input } = conversation(page);
  try {
    await page.setViewportSize({ width: 320, height: 700 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    const tabbar = page.locator('[data-testid="mobile-tabbar"]');
    await expect(tabbar).toBeVisible();
    await expect(composer).toBeVisible();
    await expectComposerLayout(page);
    // composer の下辺は tabbar の上辺以下（タブバーに隠れない）。
    const boxes = await page.evaluate(() => {
      const composer = document.querySelector('section[aria-label="メッセージ入力"]');
      const tabbar = document.querySelector('[data-testid="mobile-tabbar"]');
      if (!composer || !tabbar) return null;
      return { composerBottom: composer.getBoundingClientRect().bottom, tabbarTop: tabbar.getBoundingClientRect().top };
    });
    expect(boxes).not.toBeNull();
    expect(boxes?.composerBottom ?? 0).toBeLessThanOrEqual((boxes?.tabbarTop ?? 0) + 1);

    // タブバーの tab は 44 px 以上（5 列で 320/5 = 64 px）。
    const result = await audit(page);
    expect(result.small).toEqual([]);
    expect(result.overflow).toBe(0);

    // 送信: 入力して Enter でキューに入る（タブバーの上で composer が動く）。
    await input.fill("狭い幅の送信");
    await input.press("Enter");
    await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("狭い幅の送信");
  } finally {
    await gateway.close();
  }
});

// 入力欄は frame の末尾にあり、会話本文を覆わない（sticky の inset 二重適用の回帰）。
async function expectComposerLayout(page: Page) {
  await expect
    .poll(() =>
      page.evaluate(() => {
        const frame = document.querySelector("[data-chat-home]")?.getBoundingClientRect();
        const composer = document.querySelector('section[aria-label="メッセージ入力"]')?.getBoundingClientRect();
        const messages = document.querySelector('[data-slot="chat-messages"]')?.getBoundingClientRect();
        return (
          !!frame &&
          !!composer &&
          !!messages &&
          messages.bottom <= composer.top + 1 &&
          Math.abs(composer.bottom - (frame.bottom - 1)) <= 1
        );
      }),
    )
    .toBe(true);
}

// キーボード表示の再現: 実 browser では on-screen keyboard でしか visualViewport は縮まないので、
// composer・chat-home が読む `window.visualViewport` を縮小できる fake に差し替える。
// resize/scroll のイベントだけを実の viewport と同じ形で出す。
async function installViewportFake(page: Page) {
  await page.addInitScript(() => {
    const listeners = new Set<() => void>();
    let height = window.innerHeight;
    const fake = {
      offsetTop: 0,
      offsetLeft: 0,
      scale: 1,
      pageTop: 0,
      pageLeft: 0,
      get height() {
        return height;
      },
      set height(value: number) {
        height = value;
        for (const listener of listeners) listener();
      },
      get width() {
        return window.innerWidth;
      },
      addEventListener: (type: string, listener: () => void) => {
        if (type === "resize" || type === "scroll") listeners.add(listener);
      },
      removeEventListener: (type: string, listener: () => void) => {
        if (type === "resize" || type === "scroll") listeners.delete(listener);
      },
    };
    Object.defineProperty(window, "visualViewport", { value: fake, configurable: true });
    // 試験が縮小時に呼ぶ。
    (window as unknown as { __shrinkViewport: (value: number) => void }).__shrinkViewport = (next) => {
      fake.height = next;
    };
  });
}

// toBeVisible だけでは keyboard に隠れた要素も通るため、視界内の矩形と hit target を確認する。
async function expectInsideViewport(target: Locator) {
  await expect(target).toBeVisible();
  await expect
    .poll(() =>
      target.evaluate((element) => {
        const box = element.getBoundingClientRect();
        const vv = window.visualViewport;
        if (!vv) return false;
        const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2);
        return (
          box.top >= vv.offsetTop &&
          box.bottom <= vv.offsetTop + vv.height + 1 &&
          box.left >= 0 &&
          box.right <= vv.width &&
          !!hit &&
          element.contains(hit)
        );
      }),
    )
    .toBe(true);
}

for (const width of [320, 390]) {
  test(`${width} px: キーボード（visualViewport の縮小）でも入力・送信・停止の操作は見える`, async ({ page }) => {
    const gateway = await startChatGateway();
    const { composer, input } = conversation(page);
    try {
      await page.setViewportSize({ width, height: 700 });
      await installViewportFake(page);
      await page.goto(`${gateway.base}/?thread=chat-main`);
      await waitForStream(gateway, "chat-main");

      // 実行中（hold）: 停止ボタンが出る。
      await gateway.hold("chat-main");
      await expect(page.getByRole("button", { name: "停止" })).toBeVisible();
      const sendButton = page.getByRole("button", { name: "送信", exact: true });

      // visualViewport を 480 に縮めてキーボード表示を再現。
      await input.click();
      await page.evaluate(() =>
        (window as unknown as { __shrinkViewport: (value: number) => void }).__shrinkViewport(480),
      );
      // composer は縮んだ視界内に残る（keyboardOffset と frame の再計算で視界底に張り付く）。
      await expectInsideViewport(composer);
      await expectInsideViewport(input);
      await expectInsideViewport(sendButton);
      const stop = page.getByRole("button", { name: "停止", exact: true });
      await expectInsideViewport(stop);
      await expectComposerLayout(page);
      await input.fill("キーボードの送信");
      await expect(sendButton).toBeEnabled();
      await sendButton.click();
      await expect(page.getByRole("list", { name: "送信待ち" })).toContainText("キーボードの送信");
      await expectInsideViewport(input);
      await expectInsideViewport(sendButton);
      await expectInsideViewport(stop);
      await expectComposerLayout(page);
      await stop.click();
      await expect(page.getByRole("list", { name: "停止中の送信待ち" })).toContainText("キーボードの送信");
      await expectInsideViewport(input);
      await expectInsideViewport(sendButton);
      expect(await audit(page)).toEqual({ overflow: 0, small: [] });
      // キーボードを閉じた後もタブバーと入力欄が重ならない。
      await page.evaluate(() =>
        (window as unknown as { __shrinkViewport: (value: number) => void }).__shrinkViewport(700),
      );
      await expectInsideViewport(composer);
      await expect
        .poll(async () => {
          const box = await composer.boundingBox();
          const tabs = await page.getByTestId("mobile-tabbar").boundingBox();
          return !!box && !!tabs && box.y + box.height <= tabs.y + 1;
        })
        .toBe(true);
    } finally {
      await gateway.close();
    }
  });
}

test("320 px で添付・カードは横溢れを出さず、カードの操作領域は 44 px 以上", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 320, height: 700 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    // カード群（task ほか 7 枚）を表示。
    await expect(page.locator('article[data-card-kind="operation"]')).toBeVisible();
    let result = await audit(page);
    expect(result.overflow).toBe(0);
    expect(result.small).toEqual([]);

    // 添付 1 件（キュー行の取消ボタンは 44 px 以上、行は横溢れしない）。
    await page.locator('input[type="file"][aria-label="添付ファイルを選択"]').setInputFiles({
      name: "image.png",
      mimeType: "image/png",
      buffer: COLORED_PNG,
    });
    await expect(page.getByRole("list", { name: "添付ファイル" }).getByRole("listitem")).toContainText("image.png");
    result = await audit(page);
    expect(result.overflow).toBe(0);
    expect(result.small).toEqual([]);

    await expect(page.getByRole("button", { name: "送信", exact: true })).toBeEnabled();
    await expect
      .poll(() =>
        page
          .getByRole("list", { name: "添付ファイル", exact: true })
          .locator("img")
          .evaluate((img: HTMLImageElement) => img.naturalWidth),
      )
      .toBe(96);
    await expectComposerLayout(page);
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) {
      const scroller = conversation(page).scroller;
      await scroller.evaluate((el) => {
        el.scrollTop = el.scrollHeight;
      });
      await page.screenshot({ path: `${shots}/chat-mobile-320.png` });
    }
    await page.getByRole("button", { name: "送信", exact: true }).click();
    await expect(page.getByRole("list", { name: "添付ファイル", exact: true })).toHaveCount(0);
    const sentImage = conversation(page).section.locator('[data-slot="chat-attachment"] img');
    await sentImage.scrollIntoViewIfNeeded();
    await expect(sentImage).toBeVisible();
    await expect.poll(() => sentImage.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBe(96);
    expect(await audit(page)).toEqual({ overflow: 0, small: [] });
    if (shots) await page.screenshot({ path: `${shots}/chat-attachment-sent-320.png` });
  } finally {
    await gateway.close();
  }
});

test("カードの操作（選択肢・取消/差し戻し・詳細）は desktop でも 44 px 以上", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await expect(page.locator('article[data-card-kind="operation"]')).toBeVisible();
    const result = await audit(page);
    expect(result.small).toEqual([]);
    expect(result.overflow).toBe(0);
    await expectComposerLayout(page);
    const shots = process.env.CHAT_SHOT_DIR;
    if (shots) {
      await page.locator('article[data-card-kind="decision"] button').first().waitFor();
      await conversation(page).scroller.evaluate((el) => {
        el.scrollTop = 0;
      });
      await page.screenshot({ path: `${shots}/chat-cards-1440.png` });
    }
  } finally {
    await gateway.close();
  }
});
