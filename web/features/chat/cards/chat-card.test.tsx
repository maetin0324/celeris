import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApiClient } from "../../../api/client";
import type { ChatCard, ChatCardKind, InboxItem, InboxOption, OverrideResponse } from "../../../api/generated/types";
import { inboxKeys } from "../../../api/queries/keys";
import { card } from "../data/fixtures.test-support";
import { type CardApi, defaultCardApi, sendCardAnswer, sendOverride } from "./card-actions";
import { canOverride, overrideFailure, overrideSuccess, pendingHumanCount } from "./card-model";
import { type CardAnswerProps, type CardOverrideProps, ChatCardItem, ChatCardView } from "./chat-card";
import { PendingCountBadge, pendingBadgeText } from "./pending-badge";

const KINDS: ChatCardKind[] = ["task", "decision", "question", "approval", "plan_gate", "notice", "operation"];

const option = (key: string, label: string, over: Partial<InboxOption> = {}): InboxOption => ({
  key,
  label,
  effect: "",
  needs_note: false,
  ...over,
});

function inboxItem(over: Partial<InboxItem> = {}): InboxItem {
  return {
    id: "decision-d1",
    kind: "decision",
    title: "決定: 方式",
    age_secs: 10,
    answer: { method: "POST", path: "/api/inbox/items/decision-d1/answer", body_schema: {} },
    blocked_by: [],
    blocking: { summary: "", tasks: [], units: [] },
    created_at: "2026-10-06T00:00:00Z",
    links: [],
    options: [
      option("a", "案 A"),
      option("b", "案 B"),
      option("withdraw", "取り下げ"),
      option("other", "その他", { needs_note: true }),
    ],
    recommended: "a",
    ...over,
  };
}

const noAnswer = async () => ({ ok: true as const, removed: true });
const ready = (item: InboxItem = inboxItem()): CardAnswerProps => ({
  state: { status: "ready", item },
  pending: false,
  failure: null,
  onAnswer: noAnswer,
});
const overrideProps = (over: Partial<CardOverrideProps> = {}): CardOverrideProps => ({
  pending: false,
  outcome: null,
  onOverride: async () => ({ ok: false, message: "", conflict: false, stale: false }),
  ...over,
});

const cosOp = (over: Partial<ChatCard> = {}) =>
  card({
    kind: "operation",
    id: "o1",
    title: "answer",
    state: "applied",
    href: "/cos/operations/o1",
    actor: "cos",
    reason: "承認済み仕様内の選択",
    operation_id: "o1",
    ...over,
  });

function fakeApi(over: Partial<CardApi> = {}) {
  const calls: Array<{ fn: string; id: string; body: unknown }> = [];
  const api: CardApi = {
    answer: async (id, body) => {
      calls.push({ fn: "answer", id, body });
      return { item_id: id, removed: true, result: null };
    },
    override: async (id, body) => {
      calls.push({ fn: "override", id, body });
      return { operation_id: id, state: "superseded", action: body.action, paused_task_ids: [] };
    },
    ...over,
  };
  return { api, calls };
}

const conflict = () =>
  new ApiError("conflict", {
    method: "POST",
    path: "/api/cos/operations/o1/override",
    status: 409,
    body: { code: "chat_conflict", detail: "human revision wins" },
  });

afterEach(() => {
  configureApiClient({ fetcher: (input, init) => fetch(input, init) });
});

describe("chat cards: 表示", () => {
  it("chat_cards_every_kind_shows_title_state_actor_reason_and_detail_link", () => {
    for (const kind of KINDS) {
      const html = renderToStaticMarkup(
        <ChatCardView
          card={card({
            kind,
            id: `${kind}-1`,
            title: `題 ${kind}`,
            state: "running",
            actor: "cos",
            reason: "根拠",
            href: `/tasks/${kind}`,
          })}
        />,
      );
      expect(html).toContain(`data-card-kind="${kind}"`);
      expect(html).toContain(`題 ${kind}`);
      expect(html).toContain("対応中");
      expect(html).toContain("CoS");
      expect(html).toContain("理由: 根拠");
      expect(html).toContain(`href="/tasks/${kind}"`);
      expect(html).toContain(">詳細</a>");
      // 操作要素は 44px（min-h-11）
      expect(html).toMatch(/<a href="\/tasks\/[a-z_]+" class="inline-flex min-h-11 min-w-11/);
    }
  });

  it("chat_cards_actor_labels_and_external_href_is_not_linked", () => {
    const human = renderToStaticMarkup(
      <ChatCardView card={card({ kind: "task", actor: "human", href: "https://x.test/" })} />,
    );
    expect(human).toContain(">人<");
    expect(human).not.toContain("https://x.test/");
    expect(human).not.toContain(">詳細</a>");
    const system = renderToStaticMarkup(
      <ChatCardView card={card({ kind: "notice", actor: "system", reason: null })} />,
    );
    expect(system).toContain("システム");
    expect(system).not.toContain("理由:");
  });

  it("chat_cards_task_and_notice_have_no_inline_actions", () => {
    for (const kind of ["task", "notice"] as const) {
      const html = renderToStaticMarkup(<ChatCardView card={card({ kind, state: "pending" })} answer={ready()} />);
      expect(html).not.toContain("<button");
    }
  });
});

describe("chat cards: その場の回答", () => {
  it("chat_cards_answer_kinds_render_inbox_options_with_recommended_and_confirm", () => {
    for (const kind of ["decision", "question", "approval", "plan_gate"] as const) {
      const html = renderToStaticMarkup(<ChatCardView card={card({ kind, title: "方式" })} answer={ready()} />);
      expect(html).toContain("案 A（推奨）");
      expect(html).toContain("案 B");
      expect(html).toContain("bg-primary"); // 推奨は primary
      // 取り消しにくい選択は確認 dialog の trigger（aria-haspopup="dialog"）
      expect(html).toMatch(
        /aria-haspopup="dialog"[^>]*>取り下げ<\/button>|<button[^>]*aria-haspopup="dialog"[^>]*>取り下げ/,
      );
      // 自由文が要る選択肢はボタンにせず詳細へ誘導する
      expect(html).not.toMatch(/<button[^>]*>その他<\/button>/);
      expect(html).toContain("「その他」は理由を書いて詳細の画面で答えます。");
      expect(html).toContain(">詳細で答える</a>");
      expect(html).toContain('aria-label="「方式」の選択肢"');
    }
  });

  it("chat_cards_item_without_options_points_to_detail", () => {
    const html = renderToStaticMarkup(<ChatCardView card={card({})} answer={ready(inboxItem({ options: [] }))} />);
    expect(html).toContain("この待ちは詳細の画面で操作します。");
    expect(html).not.toContain("<button");
  });

  it("chat_cards_loading_gone_error_and_answered_states", () => {
    const view = (state: CardAnswerProps["state"]) =>
      renderToStaticMarkup(<ChatCardView card={card({})} answer={{ ...ready(), state }} />);
    expect(view({ status: "loading" })).toContain("選択肢を読み込んでいます");
    const gone = view({ status: "gone" });
    expect(gone).toContain("既に答えられたか、失効しています");
    expect(gone).not.toContain("<button");
    expect(view({ status: "error", message: "読めない" })).toContain('role="alert"');
    const answered = view({ status: "answered", label: "案 A", removed: true });
    expect(answered).toContain("「案 A」で答えました。");
    expect(answered).not.toContain("<button");
  });

  it("chat_cards_answered_and_superseded_cards_hide_actions", () => {
    for (const state of ["answered", "superseded", "revoked", "returned", "expired"]) {
      for (const kind of ["decision", "question", "approval", "plan_gate"] as const) {
        const html = renderToStaticMarkup(<ChatCardView card={card({ kind, state })} answer={ready()} />);
        expect(html).not.toContain("<button");
        expect(html).toContain(">詳細</a>");
      }
      const op = renderToStaticMarkup(<ChatCardView card={cosOp({ state })} override={overrideProps()} />);
      expect(op).not.toContain("<button");
    }
  });

  it("chat_cards_answer_failure_is_shown_with_detail_link", () => {
    const html = renderToStaticMarkup(
      <ChatCardView
        card={card({})}
        answer={{ ...ready(), failure: { ok: false, message: "この項目は専用の画面で操作します。", native: true } }}
      />,
    );
    expect(html).toContain('role="alert"');
    expect(html).toContain("この項目は専用の画面で操作します。");
  });

  it("chat_cards_answer_sends_the_inbox_answer_with_the_option_key", async () => {
    const { api, calls } = fakeApi();
    const item = inboxItem();
    expect(await sendCardAnswer(api, item, option("a", "案 A"))).toEqual({ ok: true, removed: true });
    expect(calls).toEqual([{ fn: "answer", id: "decision-d1", body: { option: "a" } }]);
    // 自由文が要る選択肢は送らない（詳細画面へ）
    const note = await sendCardAnswer(api, item, option("other", "その他", { needs_note: true }));
    expect(note).toMatchObject({ ok: false, field: "note" });
    expect(calls).toHaveLength(1);
  });

  it("chat_cards_answer_conflict_and_not_found_are_stale", async () => {
    const failWith = (status: number, code?: string) =>
      fakeApi({
        answer: async () => {
          throw new ApiError(status === 404 ? "not_found" : "conflict", {
            method: "POST",
            path: "/api/inbox/items/x/answer",
            status,
            body: code ? { code } : undefined,
          });
        },
      }).api;
    expect(await sendCardAnswer(failWith(409), inboxItem(), option("a", "案 A"))).toMatchObject({
      ok: false,
      stale: true,
    });
    expect(await sendCardAnswer(failWith(404), inboxItem(), option("a", "案 A"))).toMatchObject({
      ok: false,
      stale: true,
      message: "この項目は既に答えられたか、失効しています。",
    });
    expect(
      await sendCardAnswer(failWith(409, "native_action_required"), inboxItem(), option("a", "案 A")),
    ).toMatchObject({ ok: false, native: true });
  });

  it("chat_cards_item_reads_options_from_the_inbox_item_cache", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(inboxKeys.item("decision-d1"), inboxItem());
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <ChatCardItem card={card({ id: "decision-d1", title: "方式" })} api={fakeApi().api} />
      </QueryClientProvider>,
    );
    expect(html).toContain("案 A（推奨）");
    const closed = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <ChatCardItem card={card({ id: "decision-d1", state: "answered" })} api={fakeApi().api} />
      </QueryClientProvider>,
    );
    expect(closed).not.toContain("案 A");
  });

  it("chat_cards_default_api_uses_existing_inbox_and_override_endpoints", async () => {
    const seen: Array<{ url: string; method: string; body: unknown }> = [];
    configureApiClient({
      fetcher: vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        seen.push({
          url: String(input),
          method: init?.method ?? "GET",
          body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
        });
        const body = String(input).includes("/override")
          ? { operation_id: "o1", state: "superseded", action: "revoke", paused_task_ids: [] }
          : { item_id: "decision-d1", removed: true, result: null };
        return new Response(JSON.stringify(body), { status: 200 });
      }) as unknown as typeof fetch,
    });
    await defaultCardApi.answer("decision-d1", { option: "a" });
    await defaultCardApi.override("o1", { action: "revoke", reason: "見直す" });
    expect(seen).toEqual([
      { url: "/api/inbox/items/decision-d1/answer", method: "POST", body: { option: "a" } },
      { url: "/api/cos/operations/o1/override", method: "POST", body: { action: "revoke", reason: "見直す" } },
    ]);
  });
});

describe("chat cards: CoS 代答の取消・差し戻し", () => {
  it("chat_cards_operation_offers_revoke_and_return_behind_a_dialog", () => {
    const html = renderToStaticMarkup(<ChatCardView card={cosOp()} override={overrideProps()} />);
    expect(html).toContain("CoS の代答");
    expect(html).toContain("適用済み");
    expect(html).toMatch(/<button[^>]*aria-haspopup="dialog"[^>]*>取消<\/button>/);
    expect(html).toMatch(/<button[^>]*aria-haspopup="dialog"[^>]*>差し戻し<\/button>/);
    expect(html).toContain('href="/cos/operations/o1"');
  });

  it("chat_cards_only_applied_cos_operations_can_be_overridden", () => {
    expect(canOverride(cosOp())).toBe(true);
    expect(canOverride(cosOp({ actor: "system", state: "pending" }))).toBe(false);
    expect(canOverride(cosOp({ actor: "human" }))).toBe(false);
    expect(canOverride(cosOp({ state: "superseded" }))).toBe(false);
    expect(canOverride(card({ kind: "decision", actor: "cos", state: "applied" }))).toBe(false);
    const html = renderToStaticMarkup(
      <ChatCardView card={cosOp({ actor: "system", state: "pending" })} override={overrideProps()} />,
    );
    expect(html).not.toContain("<button");
  });

  it("chat_cards_override_sends_mode_and_trimmed_reason", async () => {
    const { api, calls } = fakeApi();
    const revoke = await sendOverride(api, "o1", "revoke", "  見直す ");
    expect(revoke).toMatchObject({ ok: true, mode: "revoke", state: "superseded" });
    const ret = await sendOverride(api, "o1", "return", "仕様を再検討する");
    expect(ret).toMatchObject({ ok: true, mode: "return" });
    expect(calls).toEqual([
      { fn: "override", id: "o1", body: { action: "revoke", reason: "見直す" } },
      { fn: "override", id: "o1", body: { action: "return", reason: "仕様を再検討する" } },
    ]);
    // 理由が空なら送らない
    expect(await sendOverride(api, "o1", "revoke", "   ")).toMatchObject({ ok: false, conflict: false, stale: false });
    expect(calls).toHaveLength(2);
  });

  it("chat_cards_override_409_is_a_conflict_between_human_and_cos", async () => {
    const { api } = fakeApi({
      override: async () => {
        throw conflict();
      },
    });
    const outcome = await sendOverride(api, "o1", "revoke", "見直す");
    expect(outcome).toMatchObject({ ok: false, conflict: true, stale: true });
    expect(overrideFailure(new Error("x"))).toMatchObject({ conflict: false });
    const html = renderToStaticMarkup(<ChatCardView card={cosOp()} override={overrideProps({ outcome })} />);
    expect(html).toContain('role="alert"');
    expect(html).toContain("競合しました（409）");
    expect(html).toContain("人と CoS の操作が競合しました");
    // 競合のあとも状態が変わっていなければ操作し直せる
    expect(html).toContain(">取消</button>");
  });

  it("chat_cards_override_needs_remediation_links_the_remediation_task", () => {
    const response: OverrideResponse = {
      operation_id: "o1",
      action: "return",
      state: "needs_remediation",
      remediation_task_id: "01TASK",
      paused_task_ids: ["t1", "t2"],
    };
    const outcome = overrideSuccess("return", response);
    const html = renderToStaticMarkup(<ChatCardView card={cosOp()} override={overrideProps({ outcome })} />);
    expect(html).toContain("代答を差し戻しました。2 件のタスクを一時停止しました。");
    expect(html).toContain("修正のタスクを起票しました");
    expect(html).toContain('href="/tasks/01TASK"');
    // 済んだ代答からは操作を消す
    expect(html).not.toContain(">取消</button>");
    expect(html).not.toContain(">差し戻し</button>");
  });

  it("chat_cards_override_revoke_success_hides_actions", () => {
    const outcome = overrideSuccess("revoke", {
      operation_id: "o1",
      action: "revoke",
      state: "superseded",
      paused_task_ids: [],
    });
    const html = renderToStaticMarkup(<ChatCardView card={cosOp()} override={overrideProps({ outcome })} />);
    expect(html).toContain("代答を取り消しました。");
    expect(html).not.toContain("修正のタスク");
    expect(html).not.toContain("<button");
  });

  it("chat_cards_override_failure_kinds", () => {
    const err = (status: number, kind: "forbidden" | "not_found" | "validation" | "network", body?: unknown) =>
      overrideFailure(new ApiError(kind, { method: "POST", path: "/x", status, body }));
    expect(err(403, "forbidden")).toMatchObject({ conflict: false, message: "この操作は人の認証でだけ行えます。" });
    expect(err(404, "not_found")).toMatchObject({ stale: true });
    expect(err(422, "validation", { code: "validation", detail: "reason must not be empty" })).toMatchObject({
      message: "reason must not be empty",
      stale: false,
    });
    expect(overrideFailure(new ApiError("network", { method: "POST", path: "/x" }))).toMatchObject({ stale: true });
  });
});

describe("chat cards: 人待ちの badge", () => {
  it("chat_cards_pending_badge_is_hidden_at_zero_and_capped", () => {
    expect(pendingBadgeText(0)).toBeNull();
    expect(pendingBadgeText(-1)).toBeNull();
    expect(pendingBadgeText(3)).toBe("3");
    expect(pendingBadgeText(120)).toBe("99+");
    expect(renderToStaticMarkup(<PendingCountBadge count={0} />)).toBe("");
    const html = renderToStaticMarkup(<PendingCountBadge count={120} />);
    expect(html).toContain('data-tone="info"');
    expect(html).toContain('<span aria-hidden="true">99+</span>');
    expect(html).toContain('<span class="sr-only">人待ち 120 件</span>');
  });

  it("chat_cards_pending_count_counts_open_answer_cards_once", () => {
    const cards = [
      card({ kind: "decision", id: "a", state: "pending" }),
      card({ kind: "decision", id: "a", state: "pending" }),
      card({ kind: "approval", id: "b", state: "escalated" }),
      card({ kind: "question", id: "c", state: "answered" }),
      card({ kind: "task", id: "d", state: "pending" }),
      cosOp(),
    ];
    expect(pendingHumanCount(cards)).toBe(2);
  });
});
