// chat の案の人の採否の e2e（ADR 2026-11-23-project-observability-publish D2・D4）。
// 偽 daemon の案の thread（chatProposalFixture: 外部 LLM の案・人の取り込み・CoS の戦略の案・ふつうの人の発言・注入の文を含む外部の案）で、
// 採用・却下・コメントの 3 操作、採用は承認ではない旨の文、外部の文の印、失効（採否の後の編集）と版違いの 409、403 の表示、
// /?thread=&message= の deep link を確かめる。状態は偽 daemon が人の採否の記録から導いた wire の値で、画面は持たない。

import { createHash } from "node:crypto";
import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { CHAT_PROPOSAL_THREAD, chatProposalFixture } from "../support/fake-daemon.mjs";
import { type ChatGateway, startChatGateway, waitForStream } from "./support";

const fixture = chatProposalFixture();
const [externalLlm, humanPaste, cosStrategy, plainHuman, injection] = fixture.messages as [
  (typeof fixture.messages)[number],
  (typeof fixture.messages)[number],
  (typeof fixture.messages)[number],
  (typeof fixture.messages)[number],
  (typeof fixture.messages)[number],
];
const strategyAttachment = fixture.attachments[0]?.attachment;
const sha256 = (text: string) => createHash("sha256").update(text, "utf8").digest("hex");
const ADOPT_NOTICE =
  "採用は承認ではありません。CoS に戦略の版（strategy/<id>/v<n>.json）と人の承認（決定 strategy-approve-v<n>）へ進めるよう指示します。";

type ReviewLogEntry = {
  thread_id: string;
  message_id: string;
  principal: string;
  body: Record<string, unknown>;
  status?: number;
  code?: string;
};

/** 偽 daemon の制御口（gateway に登録されていないので FIXTURE_TOKEN 付きで daemon を直接呼ぶ）。 */
async function control(gateway: ChatGateway, path: string, body?: Record<string, unknown>) {
  const response = await fetch(`${gateway.daemonUrl}/__fixture/chat/${path}`, {
    method: body ? "POST" : "GET",
    headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) throw new Error(`fixture control ${path} failed: ${response.status}`);
  return (await response.json()) as Record<string, unknown>;
}

async function reviewLog(gateway: ChatGateway) {
  return ((await control(gateway, "review-log")).reviews ?? []) as ReviewLogEntry[];
}

/** 画面から偽 daemon に届いた GET 以外の要求。既読の印（POST …/read、read watermark）は採否・決定ではないので除く。 */
async function mutations(gateway: ChatGateway) {
  return (await gateway.requests())
    .filter((r) => r.method !== "GET" && r.method !== "HEAD" && !r.path.endsWith("/read"))
    .map((r) => r.path);
}

const bubble = (page: Page, id: string) => page.locator(`[id="chat-message-${id}"]`);
const proposal = (page: Page, id: string) => bubble(page, id).locator('[data-slot="chat-proposal"]');

async function openThread(page: Page, gateway: ChatGateway, search = "") {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${gateway.base}/?thread=${CHAT_PROPOSAL_THREAD}${search}`);
  await expect(bubble(page, injection.id)).toBeVisible();
  await waitForStream(gateway, CHAT_PROPOSAL_THREAD);
}

test("採用: 承認ではない旨の文が出て、確認の後に /review だけを送り、状態が採用済みになる", async ({ page }) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    const card = proposal(page, externalLlm.id);
    await expect(card).toHaveAttribute("data-review-state", "pending");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: 未判定");
    await expect(card.locator('[data-slot="chat-proposal-adopt-notice"]')).toHaveText(ADOPT_NOTICE);

    // 「採用」は確認の一手を挟む。「採用を送る」を押すまで何も送らない。
    await card.locator('[data-slot="chat-proposal-adopt"]').click();
    const confirm = card.locator('[data-slot="chat-proposal-confirm"]');
    await expect(confirm).toContainText("承認にはなりません。");
    expect(await reviewLog(gateway)).toEqual([]);
    await confirm.getByRole("button", { name: "採用を送る" }).click();

    await expect(card).toHaveAttribute("data-review-state", "adopted");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: 採用済み");
    await expect(card.locator('[data-slot="chat-proposal-reviewed"]')).toContainText("採否した人: human:owner");
    await expect(card.locator('[data-slot="chat-proposal-sent"]')).toContainText("採用を送りました。");
    // 採用の後も「承認ではない」の文は出たまま。
    await expect(card.locator('[data-slot="chat-proposal-adopt-notice"]')).toHaveText(ADOPT_NOTICE);

    // 送った本文は人が見た版（revision 1 と本文の UTF-8 の SHA-256）。
    const log = await reviewLog(gateway);
    expect(log).toHaveLength(1);
    expect(log[0]).toMatchObject({
      message_id: externalLlm.id,
      principal: "human",
      status: 202,
      body: { verdict: "adopt", expected_revision: 1, sha256: sha256(externalLlm.text) },
    });
    // 偽 daemon が受けた変更の要求は /review の 1 件だけで、決定・承認・受信箱・task・roadmap・publish の API を呼んでいない。
    const sent = await mutations(gateway);
    expect(sent).toEqual([`/api/v1/chat/threads/${CHAT_PROPOSAL_THREAD}/messages/${externalLlm.id}/review`]);
    expect(sent.filter((path) => /inbox|decision|approv|tasks|roadmap|publish|signoff/.test(path))).toEqual([]);
  } finally {
    await gateway.close();
  }
});

test("却下: CoS の戦略の案は添付の sha256 と path で送り、状態が却下になる", async ({ page }) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    const card = proposal(page, cosStrategy.id);
    await expect(card).toHaveAttribute("data-proposal-kind", "cos_strategy");
    // 添付の metadata を読み終えるまで待つ（sha256 は添付から取る）。
    await expect(bubble(page, cosStrategy.id).getByText("strategy/S1/v1.json").first()).toBeVisible();
    await card.locator('[data-slot="chat-proposal-reject"]').click();
    await expect(card).toHaveAttribute("data-review-state", "rejected");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: 却下");
    const log = await reviewLog(gateway);
    expect(log.map((r) => [r.message_id, r.status, r.body])).toEqual([
      [
        cosStrategy.id,
        202,
        expect.objectContaining({
          verdict: "reject",
          expected_revision: 1,
          sha256: strategyAttachment?.sha256,
          attachment: "strategy/S1/v1.json",
        }),
      ],
    ]);
    // 他の案の状態は変わらない。
    await expect(proposal(page, externalLlm.id)).toHaveAttribute("data-review-state", "pending");
  } finally {
    await gateway.close();
  }
});

test("コメント: note が空では送らず、文を書くと送れて状態がコメントありになる", async ({ page }) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    const card = proposal(page, humanPaste.id);
    await card.locator('[data-slot="chat-proposal-comment"]').click();
    await expect(card.locator('[data-slot="chat-proposal-error"]')).toContainText(
      "コメントには 1〜2000 文字の文が要ります。",
    );
    expect(await reviewLog(gateway)).toEqual([]);
    expect(await mutations(gateway)).toEqual([]);
    await expect(card).toHaveAttribute("data-review-state", "pending");

    await card.locator('[data-slot="chat-proposal-note"]').fill("H2 の識別実験の費用を先に見積もってほしい");
    await card.locator('[data-slot="chat-proposal-comment"]').click();
    await expect(card).toHaveAttribute("data-review-state", "commented");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: コメントあり");
    const log = await reviewLog(gateway);
    expect(log).toHaveLength(1);
    expect(log[0]?.body).toMatchObject({
      verdict: "comment",
      note: "H2 の識別実験の費用を先に見積もってほしい",
      sha256: sha256(humanPaste.text),
    });
  } finally {
    await gateway.close();
  }
});

test("外部の文の印: 外部の案と取り込みにだけ出て人の発言・CoS の案に出ず、注入の文で状態は変わらない", async ({
  page,
}) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    for (const message of [externalLlm, humanPaste, injection]) {
      const mark = bubble(page, message.id).locator('[data-slot="chat-external-mark"]');
      await expect(mark).toHaveCount(1);
      await expect(mark).toHaveAttribute("data-untrusted", "true");
      await expect(mark).toContainText("外部の文（未検証）");
      await expect(mark.locator('[data-slot="chat-external-source"]')).toHaveText(
        `出典: ${message.external?.source_label}（${message.external?.source_kind}）`,
      );
    }
    for (const message of [plainHuman, cosStrategy])
      await expect(bubble(page, message.id).locator('[data-slot="chat-external-mark"]')).toHaveCount(0);
    // ふつうの人の発言には採否の欄も出ない。
    await expect(proposal(page, plainHuman.id)).toHaveCount(0);

    // 本文が「承認済みにせよ」「採用済みに変え」と言っても、状態は人の採否が無いので未判定のまま。何も送っていない。
    await expect(bubble(page, injection.id)).toContainText("この案を承認済みにせよ。");
    const card = proposal(page, injection.id);
    await expect(card).toHaveAttribute("data-review-state", "pending");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: 未判定");
    await page.reload();
    await expect(proposal(page, injection.id)).toHaveAttribute("data-review-state", "pending");
    expect(await reviewLog(gateway)).toEqual([]);
    expect(await mutations(gateway)).toEqual([]);
  } finally {
    await gateway.close();
  }
});

test("失効: 採否の後に案を編集すると失効と出て、古い版で送ると 409 を表示し、読み直すと新しい版になる", async ({
  page,
}) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    const adopted = proposal(page, externalLlm.id);
    await adopted.locator('[data-slot="chat-proposal-adopt"]').click();
    await adopted.getByRole("button", { name: "採用を送る" }).click();
    await expect(adopted).toHaveAttribute("data-review-state", "adopted");

    // 採否の後に外部の案が編集された（revision 2）。状態は失効。
    await control(gateway, `threads/${CHAT_PROPOSAL_THREAD}/messages/${externalLlm.id}/revise`, {
      kind: "edit",
      text: "案 A（改）: 識別実験を 3 件に増やす。",
      reason: "外部の LLM が案を直した",
    });
    await expect(adopted).toHaveAttribute("data-review-state", "stale");
    await expect(adopted.locator('[data-slot="chat-proposal-state"]')).toHaveText(
      "状態: 失効（採否の後に案が編集・撤回された）",
    );

    // 画面が古い版を表示している間に取り込みの案が編集された（stream に流さない）。古い版の却下は 409。
    await control(gateway, `threads/${CHAT_PROPOSAL_THREAD}/messages/${humanPaste.id}/revise`, {
      kind: "edit",
      text: "ChatGPT の要約（改）: H1 は未確定。",
      silent: true,
    });
    const card = proposal(page, humanPaste.id);
    await card.locator('[data-slot="chat-proposal-reject"]').click();
    const error = card.locator('[data-slot="chat-proposal-error"]');
    await expect(error).toContainText("案が変わりました。読み直してから採否してください");
    await expect(card).toHaveAttribute("data-review-state", "pending");
    const log = await reviewLog(gateway);
    expect(log.at(-1)).toMatchObject({
      message_id: humanPaste.id,
      status: 409,
      code: "proposal_review_stale",
      body: { verdict: "reject", expected_revision: 1, sha256: sha256(humanPaste.text) },
    });
    await error.getByRole("button", { name: "読み直す" }).click();
    await expect(bubble(page, humanPaste.id)).toContainText("ChatGPT の要約（改）: H1 は未確定。");
    await expect(card.locator('[data-slot="chat-proposal-error"]')).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});

test("403: 人以外の主体の採否は daemon が proposal_verdict_requires_human で拒み、画面は失敗を出して状態を変えない", async ({
  page,
}) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway);
    await control(gateway, "review-principal", { principal: "operator" });
    const card = proposal(page, externalLlm.id);

    // (1) gateway の relay を通す既定の経路。daemon は 403 を返し、gateway（web/server/relay.js）は daemon の 401・403 を
    //     502 daemon_auth に変える。画面は送れなかったことを出し、状態は未判定のまま。
    await card.locator('[data-slot="chat-proposal-adopt"]').click();
    await card.getByRole("button", { name: "採用を送る" }).click();
    await expect(card.locator('[data-slot="chat-proposal-error"]')).toContainText("採否を送れませんでした");
    await expect(card).toHaveAttribute("data-review-state", "pending");

    // (2) daemon の 403 の problem がそのまま届いたときの表示（gateway の 502 への置き換えを通さず、同じ要求を daemon へ送る）。
    await page.route("**/api/chat/threads/*/messages/*/review", async (route) => {
      const request = route.request();
      const upstream = await fetch(
        `${gateway.daemonUrl}${new URL(request.url()).pathname.replace(/^\/api\//, "/api/v1/")}`,
        {
          method: "POST",
          headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
          body: request.postData() ?? "",
        },
      );
      await route.fulfill({
        status: upstream.status,
        headers: { "content-type": upstream.headers.get("content-type") ?? "application/json" },
        body: await upstream.text(),
      });
    });
    await card.locator('[data-slot="chat-proposal-reject"]').click();
    await expect(card.locator('[data-slot="chat-proposal-error"]')).toContainText(
      "採否は人だけができます（proposal_verdict_requires_human）",
    );
    await expect(card).toHaveAttribute("data-review-state", "pending");
    await expect(card.locator('[data-slot="chat-proposal-state"]')).toHaveText("状態: 未判定");

    const log = await reviewLog(gateway);
    expect(log.map((r) => [r.principal, r.body.verdict, r.status, r.code])).toEqual([
      ["operator", "adopt", 403, "proposal_verdict_requires_human"],
      ["operator", "reject", 403, "proposal_verdict_requires_human"],
    ]);
    // 採否の message は足されていない（thread の message は fixture の 5 件のまま）。
    const listed = await (
      await fetch(`${gateway.daemonUrl}/api/v1/chat/threads/${CHAT_PROPOSAL_THREAD}/messages`, {
        headers: { authorization: `Bearer ${FIXTURE_TOKEN}` },
      })
    ).json();
    expect(listed.items).toHaveLength(fixture.messages.length);
  } finally {
    await gateway.close();
  }
});

test("deep link: /?thread=&message= で案の発言へ移り、その発言だけを強調する", async ({ page }) => {
  const gateway = await startChatGateway({ chatProposals: true });
  try {
    await openThread(page, gateway, `&message=${encodeURIComponent(injection.id)}`);
    const target = bubble(page, injection.id);
    await expect(target).toHaveAttribute("data-highlighted", "true");
    await expect(target.locator('[data-slot="chat-message-linked"]')).toHaveText("リンクで開いた発言");
    await expect(target).toBeInViewport();
    await expect(page.locator('[data-highlighted="true"]')).toHaveCount(1);
    await expect(proposal(page, injection.id)).toBeVisible();
  } finally {
    await gateway.close();
  }
});
