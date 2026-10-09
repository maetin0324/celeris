// チャット内カードの e2e（ADR 2026-10-05-cos-chat-home D5・D6）。
// task・決定・質問・認可・plan gate・知らせ・CoS 代答の表示、その場での回答（受信箱 API へ）、
// 代答の取消・差し戻し（override API）・409 の競合表示・詳細 link の遷移、受信箱 thread の人待ち件数。

import { expect, type Page, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { chatInboxItemsFixture } from "../support/fake-daemon.mjs";
import { makeMessage, startChatGateway, waitForStream } from "./support";

const card = (page: Page, kind: string) => page.locator(`article[data-card-kind="${kind}"]`);

test("7 種のカードが表示され、その場で回答できるカードは選択肢で答えられる", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await expect(page.getByText("進捗を教えて")).toBeVisible();
    await waitForStream(gateway, "chat-main");

    for (const [kind, label, title] of [
      ["task", "タスク", "タスク: web の認証"],
      ["decision", "決定", "決定: 認証方式を決める"],
      ["question", "質問", "質問: 画面の幅はどれにする"],
      ["approval", "認可", "認可: cluster-hpc への依頼"],
      ["plan_gate", "計画の承認", "計画の承認: 受信箱の画面"],
      ["notice", "知らせ", "知らせ: task の完了"],
      ["operation", "CoS の代答", "CoS が代わりに回答しました"],
    ] as const) {
      const one = card(page, kind);
      await expect(one).toHaveCount(1);
      await expect(one.getByText(label, { exact: true })).toBeVisible();
      await expect(one.getByText(title, { exact: true })).toBeVisible();
    }

    // 決定カード: 受信箱の項目（decision:D1）の選択肢がその場で出、推奨が分かる。
    const decision = card(page, "decision");
    await expect(decision.getByRole("button", { name: "gateway の session（推奨）" })).toBeVisible();
    await expect(decision.getByRole("button", { name: "daemon token" })).toBeVisible();

    // 人待ちのカードは「人待ち」の state badge を出す。
    await expect(decision.getByText("人待ち", { exact: true })).toBeVisible();
    // 代答は「適用済み」（取消・差し戻しが使える状態）。
    await expect(card(page, "operation").getByText("適用済み", { exact: true })).toBeVisible();

    // 選択肢で答える: 受信箱 API へ option が通り、カードは「…で答えました」に変わる。
    await decision.getByRole("button", { name: "daemon token" }).click();
    await expect
      .poll(async () => await (await fetch(`${gateway.base}/api/inbox/items/decision%3AD1`)).status, {
        timeout: 10_000,
      })
      .toBe(404);
    await expect(decision.getByText("「daemon token」で答えました。")).toBeVisible();
    await expect(decision).toHaveAttribute("data-card-state", "answered");
    await expect(decision.getByText("人待ち", { exact: true })).toHaveCount(0);
    const plan = card(page, "plan_gate");
    await expect(plan.getByRole("button", { name: "取り下げる", exact: true })).toHaveCount(0);
    await expect(plan.getByText(/「取り下げる」は理由|「計画をやり直す」・「取り下げる」は理由/)).toBeVisible();
    await plan.getByRole("button", { name: "承認する（推奨）", exact: true }).click();
    await expect(plan.getByRole("status")).toContainText("「承認する」で答えました。");
    // 質問・認可もそれぞれの受信箱項目へ回答し、完了表示へ変わる。
    for (const [kind, label, id] of [
      ["question", "320 px 向け（推奨）", "question:Q1"],
      ["approval", "認可する（推奨）", "authorization:A1"],
    ]) {
      const one = card(page, kind);
      await one.getByRole("button", { name: label, exact: true }).click();
      await expect(one.getByRole("status")).toContainText("で答えました。");
      await expect(one).toHaveAttribute("data-card-state", "answered");
      await expect
        .poll(async () => (await fetch(`${gateway.base}/api/inbox/items/${encodeURIComponent(id)}`)).status)
        .toBe(404);
    }
    // Reload uses GET 404 rather than the local answer result; all four kinds stay closed.
    await page.reload();
    for (const kind of ["decision", "question", "approval", "plan_gate"]) {
      const one = card(page, kind);
      await expect(one).toHaveAttribute("data-card-state", "closed");
      await expect(one.getByText("終了", { exact: true })).toBeVisible();
      // 回答の button は消える。決定カードだけ、回答済みの答えを変えに詳細へ移る button が残る。
      if (kind === "decision") {
        await expect(one.getByRole("button")).toHaveCount(1);
        await expect(one.getByRole("button", { name: "決定の詳細・答えを変える" })).toBeVisible();
      } else await expect(one.getByRole("button")).toHaveCount(0);
    }
    // 回答済みの決定カードから、決定を出した task の詳細の決定の行へ移る（GET /decisions で出した task を引く）。
    await page.route("**/api/decisions", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ items: [{ task_id: "T1", root_id: "T1", created_at: "", decision: { id: "D1" } }] }),
      }),
    );
    await card(page, "decision").getByRole("button", { name: "決定の詳細・答えを変える" }).click();
    await expect(page).toHaveURL(/\/tasks\/T1#decision-D1$/);
  } finally {
    await gateway.close();
  }
});

test("理由不要の破壊的な選択肢は確認を挟み、理由必須の選択肢は詳細へ誘導する", async ({ page }) => {
  // 通常 fixture の withdraw は理由必須。ここでは API が理由不要を返す場合の確認経路を検証する。
  const inboxItems = chatInboxItemsFixture().map((item) => ({
    ...item,
    options: item.options.map((option) =>
      item.id === "plan_gate:T2" && option.key === "withdraw" ? { ...option, needs_note: false } : option,
    ),
  }));
  const gateway = await startChatGateway({ inboxItems });
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await waitForStream(gateway, "chat-main");
    const plan = card(page, "plan_gate");

    // 破壊的な「取り下げる」は確認ダイアログ（AlertDialog=alertdialog）を挟む。
    await plan.getByRole("button", { name: "取り下げる", exact: true }).click();
    const dialog = page.getByRole("alertdialog", { name: /取り下げる/ });
    await expect(dialog).toBeVisible();
    // 取消: ダイアログを閉じても項目は残る。
    await dialog.getByRole("button", { name: "戻る", exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect
      .poll(async () => await (await fetch(`${gateway.base}/api/inbox/items/plan_gate%3AT2`)).status, {
        timeout: 10_000,
      })
      .toBe(200);
    // 確定: 再オープンして確認。
    await plan.getByRole("button", { name: "取り下げる", exact: true }).click();
    const reopen = page.getByRole("alertdialog", { name: /取り下げる/ });
    await reopen.getByRole("button", { name: "「計画の承認: 受信箱の画面」を取り下げる" }).click();
    await expect
      .poll(async () => await (await fetch(`${gateway.base}/api/inbox/items/plan_gate%3AT2`)).status, {
        timeout: 10_000,
      })
      .toBe(404);
    await expect(plan.getByText("「取り下げる」で答えました。")).toBeVisible();

    // 自由文（理由）の要る選択肢（「その他」）はボタンにならず、「詳細で答える」の link が出る。
    const question = card(page, "question");
    await expect(question.getByRole("button", { name: "320 px 向け（推奨）" })).toBeVisible();
    await expect(question.getByText("「その他」は理由を書いて詳細の画面で答えます")).toBeVisible();
    await expect(question.getByRole("link", { name: "詳細で答える" })).toHaveAttribute("href", "/inbox");
  } finally {
    await gateway.close();
  }
});

test("CoS 代答は取消・差し戻しでき、理由が必須、409 は競合として表示される", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await waitForStream(gateway, "chat-main");
    const operation = card(page, "operation");

    // 409: 競合の state にすると 409 が「競合しました（409）」Notice（role=alert）としてカードに残る。
    // 成功するまで取消・差し戻しのボタンは残る（結果が ok で無い限り）。
    await gateway.setOverrideState("conflict");
    await operation.getByRole("button", { name: "差し戻し", exact: true }).click();
    const returnDialog = page.getByRole("alertdialog", { name: /差し戻し/ });
    await expect(returnDialog).toBeVisible();
    // 理由入力は必須（空だと送れない、ダイアログは開いたまま）。
    await returnDialog.getByRole("button", { name: "代答を差し戻し" }).click();
    await expect(returnDialog.getByRole("alert")).toContainText("理由を書いてください");
    await returnDialog.locator("textarea").fill("内容が違います");
    await returnDialog.getByRole("button", { name: "代答を差し戻し" }).click();
    await expect(returnDialog).toBeHidden();
    await expect(operation.getByRole("alert").filter({ hasText: "競合しました（409）" })).toBeVisible();
    expect((await gateway.overrideLog()).at(-1)).toEqual({
      operation_id: "OP1",
      action: "return",
      reason: "内容が違います",
    });

    // 取消: 競合を解くと成功。結果は role=status の「代答を取り消しました」で、
    // 成功後はボタンが消える（もう操作できない）。
    await gateway.setOverrideState("succeed");
    await operation.getByRole("button", { name: "取消", exact: true }).click();
    const revokeDialog = page.getByRole("alertdialog", { name: /取消/ });
    await revokeDialog.locator("textarea").fill("誤って適用されました");
    await revokeDialog.getByRole("button", { name: "代答を取消" }).click();
    await expect(revokeDialog).toBeHidden();
    await expect(operation.getByRole("status").filter({ hasText: "代答を取り消しました" })).toBeVisible();
    await expect(operation.getByRole("button", { name: "取消" })).toHaveCount(0);
    await expect(operation.getByRole("button", { name: "差し戻し" })).toHaveCount(0);
    expect((await gateway.overrideLog()).at(-1)).toEqual({
      operation_id: "OP1",
      action: "revoke",
      reason: "誤って適用されました",
    });
  } finally {
    await gateway.close();
  }
});

test("CoS 代答を差し戻すと結果がカードに残り、再操作できなくなる", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.goto(`${gateway.base}/?thread=chat-main`);
    const operation = card(page, "operation");
    await operation.getByRole("button", { name: "差し戻し", exact: true }).click();
    const dialog = page.getByRole("alertdialog", { name: /差し戻し/ });
    await dialog.getByRole("textbox", { name: "理由（必須）" }).fill("人が判断し直します");
    await dialog.getByRole("button", { name: "代答を差し戻し", exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(operation.getByRole("status")).toContainText("代答を差し戻しました。");
    await expect(operation.getByRole("button")).toHaveCount(0);
    expect(await gateway.overrideLog()).toEqual([
      { operation_id: "OP1", action: "return", reason: "人が判断し直します" },
    ]);
  } finally {
    await gateway.close();
  }
});

test("詳細 link は web 内の実在画面へ SPA で行き、戻るで元会話がそのまま残る", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    await waitForStream(gateway, "chat-main");

    // task カード: /tasks/T1 へ。
    await card(page, "task").getByRole("link", { name: "詳細" }).click();
    await expect(page).toHaveURL(/\/tasks\/T1$/);
    await expect(page.getByRole("heading", { name: "タスクの詳細 T1" })).toBeVisible();

    // 代答カード: ?thread=chat-main&operation=OP1 へ（SPA の navigation。URL が変わる）。
    await page.goBack();
    await expect(page).toHaveURL(/\/\?thread=chat-main$/);
    await card(page, "operation").getByRole("link", { name: "詳細" }).click();
    await expect(page).toHaveURL(/\?thread=chat-main&operation=OP1$/);
  } finally {
    await gateway.close();
  }
});

test("受信箱 thread の badge は受信箱の人待ち項目数を示し、回答で減る", async ({ page }) => {
  const gateway = await startChatGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${gateway.base}/?thread=chat-main`);
    // 一覧の先頭は受信箱 thread。badge は全受信箱の 6 件（カード以外の待ちも含む）。
    const sidebar = page.locator("#chat-thread-sidebar");
    await expect.poll(async () => sidebar.getByText(/受信箱 \d 件待ち/).count(), { timeout: 10_000 }).toBe(1);
    await expect(sidebar.getByText("受信箱 6 件待ち")).toBeVisible();

    // 人待ちカードを 1 件答えると badge も減る。
    const decision = card(page, "decision");
    await decision.getByRole("button", { name: "daemon token" }).click();
    await expect(sidebar.getByText("受信箱 5 件待ち")).toBeVisible();
  } finally {
    await gateway.close();
  }
});

test.describe("受信箱の通知だけの CoS digest", () => {
  test.use({ bypassCSP: true });

  test("通知を折りたたんで人待ちを見せ、本文を開いて確認できる", async ({ page }) => {
    const gateway = await startChatGateway();
    try {
      await page.goto(`${gateway.base}/?thread=chat-inbox`);
      await waitForStream(gateway, "chat-inbox");
      const notice = {
        kind: "notice",
        id: "notice:n1",
        title: "検証完了",
        state: "observed",
        actor: "cos",
        reason: "対応不要",
        href: "/inbox",
        operation_id: null,
      };
      const digest = makeMessage(
        "chat-inbox",
        100,
        "assistant",
        "受信箱の一次対応の結果（1 件）\n\n### 検証完了（知らせ）\n\n- 判断: 見ただけ（判断は不要）\n- 理由: 対応不要",
        { cards: [notice], client_message_id: "cos-triage-digest:r1", run_id: "r1" },
      );
      await gateway.emit("chat-inbox", { type: "message", data: { message: digest } });
      const waiting = makeMessage("chat-inbox", 101, "assistant", "人が決めること: 認証方式", {
        cards: [
          notice,
          {
            kind: "decision",
            id: "decision:D1",
            title: "認証方式",
            state: "escalated",
            actor: "cos",
            reason: "人の判断が必要",
            href: "/inbox",
            operation_id: null,
          },
        ],
      });
      await gateway.emit("chat-inbox", { type: "message", data: { message: waiting } });
      const folded = page.locator('[data-slot="chat-notice-folded"]');
      const waitingCard = page.getByRole("article", { name: "決定: 認証方式", exact: true });
      for (const width of [360, 390, 412, 1440]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(folded).toHaveCount(1);
        await expect(folded).not.toHaveAttribute("open");
        await expect(waitingCard.getByText("人の判断待ち", { exact: true })).toBeVisible();
        await expect(waitingCard.getByRole("button", { name: "daemon token" })).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
        ).toBe(0);
        const summary = folded.locator("summary");
        await summary.scrollIntoViewIfNeeded();
        const box = await summary.boundingBox();
        expect(box?.height).toBeGreaterThanOrEqual(44);
        expect(await seriousViolations(page)).toEqual([]);
        const shots = process.env.CHAT_SHOT_DIR;
        if (shots) await page.screenshot({ path: `${shots}/inbox-digest-${width}.png`, fullPage: true });
        await summary.click();
        await expect(folded.getByText("対応不要", { exact: false }).first()).toBeVisible();
        await summary.click();
      }
    } finally {
      await gateway.close();
    }
  });
});
