import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, configureApiClient } from "../../api/client";
import type { EventRow, EventsPage } from "../../api/generated/types";
import { decisionView, outcome } from "./decision-fixtures.test-support";
import {
  answerHistory,
  decisionHref,
  decisionIdFromItemId,
  fetchAnswerEvents,
  locateDecision,
  revisability,
  reviseBody,
  reviseDecision,
  reviseFailure,
  reviseReady,
  reviseSummary,
} from "./decision-model";

afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));

describe("decision revise: できるか", () => {
  it("decision_revise_only_answered_choice", () => {
    expect(revisability(decisionView())).toEqual({ ok: true });
    const open = revisability(decisionView({ status: "open", answer: null }));
    expect(open.ok).toBe(false);
    expect(!open.ok && open.reason).toContain("まだ回答されていません");
    const withdrawn = revisability(decisionView({ status: "withdrawn", answer: null, withdrawn_reason: "不要" }));
    expect(!withdrawn.ok && withdrawn.reason).toContain("取り下げ済み");
    const daemon = revisability(decisionView({ kind: "limit" }));
    expect(!daemon.ok && daemon.reason).toContain("daemon の決定");
  });

  it("decision_revise_ready_requires_change_or_note", () => {
    const view = decisionView();
    expect(reviseReady(view, { option: "", note: "" })).toBe(false);
    expect(reviseReady(view, { option: "explicit", note: "" })).toBe(false);
    expect(reviseReady(view, { option: "explicit", note: "理由を足す" })).toBe(true);
    expect(reviseReady(view, { option: "implicit", note: "" })).toBe(true);
    // 自由記述（other）は理由が必須で、本文から option を省く。
    expect(reviseReady(view, { option: "other", note: " " })).toBe(false);
    expect(reviseReady(view, { option: "other", note: "別の方式" })).toBe(true);
    expect(reviseBody(view, { option: "other", note: " 別の方式 " })).toEqual({ note: "別の方式" });
    expect(reviseBody(view, { option: "implicit", note: "" })).toEqual({ option: "implicit" });
    expect(reviseReady(view, { option: "implicit", note: "x".repeat(2001) })).toBe(false);
  });
});

describe("decision revise: 送信と結果", () => {
  it("decision_revise_posts_option_and_note_to_the_revise_api", async () => {
    const view = decisionView();
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValue(new Response(JSON.stringify(outcome(view)), { status: 200 }));
    configureApiClient({ fetcher });
    const result = await reviseDecision(view, { option: "implicit", note: "人の指摘で変更" });
    expect(result.notified_children).toEqual(["C1", "C2"]);
    const [path, init] = fetcher.mock.calls[0] ?? [];
    expect(path).toBe("/api/decisions/D1/revise");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(String(init?.body))).toEqual({ option: "implicit", note: "人の指摘で変更" });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it("decision_revise_summary_lists_effect_and_notified_children", () => {
    const after = decisionView({ answer: { option: "implicit", by: "human" } });
    const lines = reviseSummary(outcome(after));
    expect(lines[0]).toContain("(ii) implicit-account");
    expect(lines.join("\n")).toContain("子タスク 2 件にコメント");
    expect(lines.join("\n")).toContain("新しい回答を後続の仕事で参照");
    const none = reviseSummary(outcome(after, { notified_children: [], replan_requested: true, resumed: ["u1"] }));
    expect(none.join("\n")).toContain("通知はありません");
    expect(none.join("\n")).toContain("計画の立て直し");
    expect(none.join("\n")).toContain("u1");
  });

  it("decision_revise_errors_read_as_sentences", () => {
    const err = (status: number, body: unknown) =>
      new ApiError(
        status === 409 ? "conflict" : status === 404 ? "not_found" : status === 403 ? "forbidden" : "validation",
        {
          method: "POST",
          path: "/api/decisions/D1/revise",
          status,
          body,
        },
      );
    const conflict = reviseFailure(err(409, { code: "decision_not_open", decision_status: "withdrawn" }));
    expect(conflict.message).toContain("今は答えを変えられません（状態: 取り下げ済み）");
    expect(conflict.stale).toBe(true);
    expect(reviseFailure(err(422, { detail: "unknown option" })).message).toContain("unknown option");
    expect(reviseFailure(err(404, {})).message).toContain("見つかりません");
    expect(reviseFailure(err(403, {})).message).toContain("管理の権限");
    const timeout = reviseFailure(new ApiError("timeout", { method: "POST", path: "/api/decisions/D1/revise" }));
    expect(timeout.message).toContain("結果を確認できません");
    expect(reviseFailure(new Error("x")).message).toContain("変えられませんでした");
  });
});

describe("decision: 履歴と行き先", () => {
  it("decision_history_from_timeline_last_answer_wins", () => {
    const view = decisionView({ answer: { option: "implicit", by: "human", note: "変更" } });
    const events: EventRow[] = [
      {
        task_id: "T1",
        id: 1,
        seq: 1,
        ts: "2026-10-09T01:00:00Z",
        event: { type: "decision_answered", id: "D1", option: "explicit", by: "cos", note: null },
      },
      {
        task_id: "T1",
        id: 1,
        seq: 2,
        ts: "2026-10-09T01:30:00Z",
        event: { type: "decision_answered", id: "OTHER", option: "x", by: "human" },
      },
      {
        task_id: "T1",
        id: 1,
        seq: 3,
        ts: "2026-10-09T02:00:00Z",
        event: { type: "decision_answered", id: "D1", option: "implicit", by: "human", note: "変更" },
      },
    ];
    const rows = answerHistory(view, events);
    expect(rows.map((r) => [r.label, r.by, r.note])).toEqual([
      ["(i) explicit-account", "cos", null],
      ["(ii) implicit-account", "human", "変更"],
    ]);
    // timeline が無ければ今の答えだけ。
    expect(answerHistory(view)).toEqual([
      { at: "2026-10-09T01:00:00Z", option: "implicit", label: "(ii) implicit-account", note: "変更", by: "human" },
    ]);
    expect(answerHistory(decisionView({ status: "open", answer: null }))).toEqual([]);
  });

  it("loads every answer page and keeps event order even if timestamps go backwards", async () => {
    const first: EventRow = {
      id: 1,
      task_id: "T1",
      seq: 2001,
      ts: "2026-10-09T03:00:00Z",
      event: { type: "decision_answered", id: "D1", option: "explicit", by: "human" },
    };
    const last: EventRow = {
      ...first,
      id: 2,
      seq: 4000,
      ts: "2026-10-09T02:00:00Z",
      event: { ...first.event, type: "decision_answered", id: "D1", option: "implicit", by: "human" },
    };
    const pages: EventsPage[] = [
      { items: [first], has_more: true },
      { items: [last], has_more: false },
    ];
    const fetcher = vi
      .fn<typeof fetch>()
      .mockImplementation(async () => new Response(JSON.stringify(pages.shift()), { status: 200 }));
    configureApiClient({ fetcher });
    const events = await fetchAnswerEvents("T1");
    expect(events).toEqual([first, last]);
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
      "/api/tasks/T1/events?types=decision_answered&after_seq=-1&limit=500",
      "/api/tasks/T1/events?types=decision_answered&after_seq=2001&limit=500",
    ]);
    expect(answerHistory(decisionView(), events).map((r) => r.option)).toEqual(["explicit", "implicit"]);
  });

  it("keeps the current answer while older history is being refreshed", () => {
    const event: EventRow = {
      id: 1,
      task_id: "T1",
      seq: 1,
      ts: "2026-10-09T00:00:00Z",
      event: { type: "decision_answered", id: "D1", option: "implicit", by: "human" },
    };
    expect(answerHistory(decisionView(), [event]).map((r) => r.option)).toEqual(["implicit", "explicit"]);
  });

  it("decision_links_from_inbox_and_chat_cards", async () => {
    expect(decisionIdFromItemId("decision-01ABC")).toBe("01ABC");
    expect(decisionIdFromItemId("decision:D1")).toBe("D1");
    expect(decisionIdFromItemId("question-1")).toBeNull();
    expect(decisionHref("T 1", "D1")).toBe("/tasks/T%201#decision-D1");
    const body = JSON.stringify({ items: [decisionView({}, { task_id: "T9" })] });
    const fetcher = vi.fn<typeof fetch>().mockImplementation(async () => new Response(body, { status: 200 }));
    configureApiClient({ fetcher });
    await expect(locateDecision("D1")).resolves.toBe("/tasks/T9#decision-D1");
    await expect(locateDecision("NOPE")).resolves.toBeNull();
    expect(fetcher.mock.calls[0]?.[0]).toBe("/api/decisions");
  });
});
