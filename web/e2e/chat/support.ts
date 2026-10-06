// web/e2e/chat/ の会話側 e2e の共通ヘルパ（ADR 2026-10-05-cos-chat-home D6 の UI 行）。
// 偽 daemon の制御 endpoint（/__fixture/chat/…）と SSE 出来事待ち（expect.poll）で進めるので、
// sleep の長さや確率に頼らない。制御 endpoint は gateway に登録されていないため、
// FIXTURE_TOKEN を付けた daemon の URL 直接呼び出しを使う（fake-daemon.test.ts と同じ経路）。

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, type Locator, type Page } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, type FakeDaemonOptions } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

export type ChatEventInput = {
  type: string;
  data: Record<string, unknown>;
  run_id?: string;
  message_id?: string;
};

export type ChatGateway = {
  base: string;
  daemonUrl: string;
  /** 偽 daemon に直接（FIXTURE_TOKEN 付き）event を追加する。応答の event.id を返す。 */
  emit(threadId: string, event: ChatEventInput): Promise<string>;
  /** 実行中の run を作る（ないときだけ）。run id を返す。 */
  hold(threadId: string): Promise<string>;
  /** cursor の失効（before_id 以前は 410）。 */
  expire(threadId: string, beforeId?: string): Promise<string>;
  /** 全チャットの SSE 接続を切る（再接続の試験用）。切った接続数を返す。 */
  disconnect(): Promise<number>;
  /** 偽 daemon が見た要求（path・method・body・query 付き）。 */
  requests(): Promise<
    Array<{ path: string; method: string; body?: string; query?: string; lastEventId?: string | null }>
  >;
  close(): Promise<void>;
};

export async function startChatGateway(options: FakeDaemonOptions = {}): Promise<ChatGateway> {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-chat-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ profile: "rich", ...options, token: FIXTURE_TOKEN });
  const daemonUrl = await daemon.start();
  const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
  const control = async (pathName: string, body: Record<string, unknown>) => {
    const response = await fetch(`${daemonUrl}${pathName}`, {
      method: "POST",
      headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!response.ok) throw new Error(`fixture control ${pathName} failed: ${response.status}`);
    return (await response.json()) as Record<string, unknown>;
  };
  return {
    base: gateway.base,
    daemonUrl,
    async emit(threadId, event) {
      const result = await control(`/__fixture/chat/threads/${encodeURIComponent(threadId)}/emit`, event);
      return String((result.event as { id: string })?.id ?? "");
    },
    async hold(threadId) {
      const result = await control("/__fixture/chat/hold", { thread_id: threadId });
      return String(result.run_id ?? "");
    },
    async expire(threadId, beforeId) {
      const result = await control(
        `/__fixture/chat/threads/${encodeURIComponent(threadId)}/expire`,
        beforeId ? { before_id: beforeId } : {},
      );
      return String(result.expired_before ?? "");
    },
    async disconnect() {
      const result = await control("/__fixture/chat/disconnect", {});
      return Number(result.disconnected ?? 0);
    },
    async requests() {
      return daemon.requests as Array<{
        path: string;
        method: string;
        body?: string;
        query?: string;
        lastEventId?: string | null;
      }>;
    },
    async close() {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

export type ConversationHandles = {
  section: ReturnType<Page["getByRole"]>;
  scroller: ReturnType<Page["locator"]>;
  composer: ReturnType<Page["getByRole"]>;
  input: ReturnType<Page["getByRole"]>;
};

/** チャット画面の操作対象。section=会話の scroll 領域、composer=入力欄の section。 */
export function conversation(page: Page): ConversationHandles {
  const section = page.getByRole("region", { name: "会話" });
  return {
    section,
    scroller: page.locator('section[aria-label="会話"]'),
    composer: page.getByRole("region", { name: "メッセージ入力" }),
    input: page.getByRole("textbox", { name: "CoS へのメッセージ" }),
  };
}

/** composer の status 行（aria-live）が文言を包含するようになるまで待つ。 */
export async function expectComposerStatus(page: Page, text: string) {
  await expect(page.getByRole("region", { name: "メッセージ入力" }).locator("div[aria-live='polite']")).toContainText(
    text,
  );
}

/** 会話の scroll 領域の下端かどうか（閾値 48 px、MessageList と同じ）。 */
export function isAtBottom(metrics: { scrollTop: number; scrollHeight: number; clientHeight: number }) {
  return metrics.scrollHeight - metrics.clientHeight - metrics.scrollTop <= 48;
}

/** 本文が 1 回だけ出ていることを確認する（二重表示の検査）。page かその部分の locator を渡す。 */
export async function expectOnce(scope: Page | Locator, selector: string, text: string) {
  await expect.poll(() => scope.locator(selector).filter({ hasText: text }).count(), { timeout: 10_000 }).toBe(1);
}

/** 会話の SSE 接続（偽 daemon への stream 要求）が立ったのを待つ。 */
export async function waitForStream(gateway: ChatGateway, threadId: string) {
  await expect
    .poll(
      async () =>
        (await gateway.requests()).some(
          (r) => r.method === "GET" && r.path === `/api/v1/chat/threads/${threadId}/stream`,
        ),
      { timeout: 10_000 },
    )
    .toBe(true);
}

/** 表示用の ChatMessage を作る（emit の message event に使う）。 */
export function makeMessage(
  threadId: string,
  seq: number,
  role: "user" | "assistant" | "system",
  text: string,
  extra: Record<string, unknown> = {},
) {
  return {
    id: `${threadId}-m${seq}`,
    thread_id: threadId,
    seq,
    role,
    text,
    state: "completed",
    attachment_ids: [],
    cards: [],
    client_message_id: null,
    run_id: null,
    reply_to_id: null,
    created_at: "2026-10-05T12:00:00Z",
    updated_at: "2026-10-05T12:00:00Z",
    ...extra,
  };
}
