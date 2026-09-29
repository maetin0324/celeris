import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { answerDecision, planGateTask, withdrawDecision } from "~/celeris/decisions-admin.server";
import type { AttentionItem, DecisionInboxItem, DecisionOutcome, ExecutionView, Task } from "~/celeris/types";
import { DecisionFlash, DecisionItemCard, PlanApprovalCard, PlanGateFlash } from "~/components/DecisionControls";
import { ExecutionModeControl } from "~/components/ExecutionModeControl";
import { task as taskFixture } from "../mock-celeris/fixtures";
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

/**
 * celeris ADR-0079 D7 / D8、§7 R4b (b): 受信箱の「決定」でその場で答える（`option` と `note` が API に送られる）・
 * 取り下げる、「計画の承認」の 3 つのボタン（`POST /tasks/{id}/execution/plan-gate`）、409 / 422 の文言。
 */

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => {
  await mock.close();
});

function form(entries: Array<[string, string]>): FormData {
  const f = new FormData();
  for (const [k, v] of entries) f.append(k, v);
  return f;
}

function render(el: ReactElement): string {
  const router = createMemoryRouter([{ path: "/", element: el }], { initialEntries: ["/"] });
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

const D = "01DECISION0000000000000001";
const T = "01PLANGATETASK00000000001";

const item: DecisionInboxItem = {
  age_secs: 5400,
  cost_of_reversal: "medium",
  cost_note: "項目 ID の移行",
  created_at: "2026-09-29T00:00:00Z",
  id: D,
  key: "h1",
  kind: "choice",
  needed_before: ["p2-b"],
  options: [
    { key: "org-vault", label: "既存の組織 vault" },
    { key: "local-file", label: "ローカルの暗号化ファイル", consequence: "移行が要る" },
  ],
  origin: "planner",
  path: [
    { task_id: T, title: "browser capability", stage: "Phase 2" },
    { task_id: "c", title: "broker", unit: "P2-B" },
  ],
  question: "credential backend をどれにするか",
  recommended: "org-vault",
  root_id: T,
  task_id: "c",
};

function outcome(status: "answered" | "withdrawn"): DecisionOutcome {
  return {
    cancelled: status === "withdrawn" ? ["p2-b"] : [],
    decision: {
      created_at: "2026-09-29T00:00:00Z",
      root_id: T,
      task_id: "c",
      decision: {
        id: D,
        key: "h1",
        kind: "choice",
        question: item.question,
        options: item.options,
        recommended: "org-vault",
        cost_of_reversal: "medium",
        needed_before: ["p2-b"],
        path: item.path,
        raised_by: { origin: "planner", task_id: "c" },
        status,
      },
    },
    effect: status === "answered" ? "resume" : "withdraw",
    replan_requested: false,
    resumed: status === "answered" ? ["p2-b"] : [],
  };
}

describe("answerDecision / withdrawDecision（POST /decisions/{id}/answer|withdraw）", () => {
  it("選んだ option と note をそのまま送り、再開した unit を返す", async () => {
    mock.on("POST", `/api/v1/decisions/${D}/answer`, (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ option: "local-file", note: "移行は来週" });
      sendJson(res, 200, outcome("answered"));
    });
    const o = await answerDecision(
      client,
      form([
        ["decision_id", D],
        ["option", "local-file"],
        ["note", "移行は来週"],
      ]),
    );
    expect(o.ok).toBe(true);
    if (!o.ok) throw new Error("expected success");
    expect(o.result.resumed).toEqual(["p2-b"]);
    const html = renderToStaticMarkup(createElement(DecisionFlash, { outcome: o }));
    expect(html).toContain("決定に答えました。再開した unit: p2-b。");
  });

  it("自由記述（option が空）は option を送らず note だけ。空白の note も送らない", async () => {
    const bodies: unknown[] = [];
    mock.on("POST", `/api/v1/decisions/${D}/answer`, (_req, res, body) => {
      bodies.push(JSON.parse(body));
      sendJson(res, 200, outcome("answered"));
    });
    await answerDecision(
      client,
      form([
        ["decision_id", D],
        ["option", ""],
        ["note", "両方を比べてから決めて"],
      ]),
    );
    await answerDecision(
      client,
      form([
        ["decision_id", D],
        ["option", "org-vault"],
        ["note", "   "],
      ]),
    );
    expect(bodies).toEqual([{ note: "両方を比べてから決めて" }, { option: "org-vault" }]);
  });

  it("409 decision_not_open と 422（選択肢の外）の文言をそのまま出す", async () => {
    mock.on("POST", `/api/v1/decisions/${D}/answer`, (_req, res, body) => {
      const b = JSON.parse(body) as { option?: string };
      if (b.option === "nope") {
        sendProblem(res, {
          status: 422,
          code: "validation",
          detail: "option: nope is not one of the options",
          extra: { errors: [{ field: "option", message: "option: nope is not one of the options" }] },
        });
      } else {
        sendProblem(res, {
          status: 409,
          code: "decision_not_open",
          detail: "decision is answered, not open",
          extra: { decision_status: "answered" },
        });
      }
    });
    const conflict = await answerDecision(
      client,
      form([
        ["decision_id", D],
        ["option", "org-vault"],
      ]),
    );
    expect(conflict.ok).toBe(false);
    if (conflict.ok) throw new Error("expected failure");
    expect(conflict.error.status).toBe(409);
    const conflictHtml = renderToStaticMarkup(createElement(DecisionFlash, { outcome: conflict }));
    expect(conflictHtml).toContain('data-flash-code="decision_not_open"');
    expect(conflictHtml).toContain("decision is answered, not open");

    const invalid = await answerDecision(
      client,
      form([
        ["decision_id", D],
        ["option", "nope"],
      ]),
    );
    if (invalid.ok) throw new Error("expected failure");
    expect(invalid.error.status).toBe(422);
    expect(invalid.error.fields.option).toEqual(["option: nope is not one of the options"]);
    expect(renderToStaticMarkup(createElement(DecisionFlash, { outcome: invalid }))).toContain(
      "option: nope is not one of the options",
    );
  });

  it("取り下げは note を reason として送り、取り下げた unit を出す", async () => {
    mock.on("POST", `/api/v1/decisions/${D}/withdraw`, (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ reason: "この機能は要らない" });
      sendJson(res, 200, outcome("withdrawn"));
    });
    const o = await withdrawDecision(
      client,
      form([
        ["decision_id", D],
        ["option", "org-vault"],
        ["note", "この機能は要らない"],
      ]),
    );
    expect(o.ok).toBe(true);
    expect(renderToStaticMarkup(createElement(DecisionFlash, { outcome: o }))).toContain(
      "決定を取り下げました。取り下げた unit: p2-b。",
    );
  });

  it("decision_id の無いフォームは送らない", async () => {
    const o = await answerDecision(client, form([["option", "a"]]));
    expect(o.ok).toBe(false);
    if (o.ok) throw new Error("expected failure");
    expect(o.error.status).toBe(400);
    expect(mock.requests.filter((r) => r.method === "POST")).toHaveLength(0);
  });
});

describe("planGateTask（POST /tasks/{id}/execution/plan-gate）", () => {
  it.each([
    ["approve", "", { action: "approve" }, "plan_approved", "running"],
    ["replan", "段階を 2 つに", { action: "replan", note: "段階を 2 つに" }, "plan_replan", "ready"],
    ["withdraw", "", { action: "withdraw" }, "cancelled", "cancelled"],
  ] as const)("%s は action と note を写す", async (action, note, expected, reason, to) => {
    mock.on("POST", `/api/v1/tasks/${T}/execution/plan-gate`, (_req, res, body) => {
      expect(JSON.parse(body)).toEqual(expected);
      sendJson(res, 200, { id: T, from: "blocked", to, reason, cascaded: [] });
    });
    const o = await planGateTask(
      client,
      T,
      form([
        ["plan_action", action],
        ["note", note],
      ]),
    );
    expect(o.ok).toBe(true);
    const html = renderToStaticMarkup(createElement(PlanGateFlash, { outcome: o }));
    expect(html).toContain(`計画の承認に応えました`);
    expect(html).toContain(`reason: ${reason}`);
  });

  it("空の replan の 422 と、承認待ちでない 409 の文言", async () => {
    mock.on("POST", `/api/v1/tasks/${T}/execution/plan-gate`, (_req, res, body) => {
      const b = JSON.parse(body) as { action: string };
      if (b.action === "replan") {
        sendProblem(res, {
          status: 422,
          code: "validation",
          detail: "note: replan requires a non-empty note",
          extra: { errors: [{ field: "note", message: "note: replan requires a non-empty note" }] },
        });
      } else {
        sendProblem(res, {
          status: 409,
          code: "invalid_transition",
          detail: "task is not awaiting plan approval",
          extra: { task_status: "ready" },
        });
      }
    });
    const replan = await planGateTask(client, T, form([["plan_action", "replan"]]));
    if (replan.ok) throw new Error("expected failure");
    expect(replan.error.status).toBe(422);
    expect(replan.error.fields.note).toEqual(["note: replan requires a non-empty note"]);
    const approve = await planGateTask(client, T, form([["plan_action", "approve"]]));
    if (approve.ok) throw new Error("expected failure");
    expect(approve.error.status).toBe(409);
    const html = renderToStaticMarkup(createElement(PlanGateFlash, { outcome: approve }));
    expect(html).toContain("状態が変わりました（409 invalid_transition）");
    expect(html).toContain("task is not awaiting plan approval");
  });

  it("知らない plan_action は送らない", async () => {
    const o = await planGateTask(client, T, form([["plan_action", "skip"]]));
    if (o.ok) throw new Error("expected failure");
    expect(o.error.status).toBe(400);
    expect(mock.requests.filter((r) => r.method === "POST")).toHaveLength(0);
  });
});

describe("受信箱の画面（決定・計画の承認）", () => {
  it("決定の 1 件: パンくず・問い・推奨の印・自由記述・後戻り・待つもの・答える / 取り下げる", () => {
    const html = render(createElement(DecisionItemCard, { item, fetchedAt: "2026-09-29T01:30:00Z" }));
    expect(html).toContain("browser capability › Phase 2 › broker › P2-B");
    expect(html).toContain("credential backend をどれにするか");
    expect(html).toContain('data-testid="decision-recommended"');
    expect(html).toMatch(/data-testid="decision-option-org-vault" name="option" checked=""/);
    expect(html).toContain('data-testid="decision-option-free"');
    expect(html).toContain("後戻り: 中");
    expect(html).toContain("unit p2-b");
    expect(html).toContain("1時間30分前");
    expect(html).toContain('action="/inbox"');
    expect(html).toContain(`name="decision_id" value="${D}"`);
    expect(html).toContain('value="decision_answer" data-testid="decision-answer" name="intent"');
    expect(html).toContain('value="decision_withdraw" data-testid="decision-withdraw" name="intent"');
    // daemon の決定（上限）は自由記述を出さない。
    const limit = render(createElement(DecisionItemCard, { item: { ...item, kind: "limit" } }));
    expect(limit).not.toContain('data-testid="decision-option-free"');
  });

  it("計画の承認: 理由・見取り図・同じ節点の決定と 3 つのボタン（送り先はタスクの action）", () => {
    const approval: Extract<AttentionItem, { type: "plan_approval" }> = {
      type: "plan_approval",
      at: "2026-09-29T00:00:00Z",
      decision_ids: [D],
      plan_id: "P",
      plan_version: 2,
      reasons: ["decisions:h1"],
      stages: [{ key: "s1", title: "Phase 1", review_human: true, units: ["a: 下調べ（leaf）"] }],
      summary: "人の決定 1 件",
      task: {
        id: T,
        title: "browser capability",
        status: "blocked",
        kind: "execute",
        actions: ["cancel", "plan_gate"],
      },
    };
    const html = render(createElement(PlanApprovalCard, { item: approval }));
    expect(html).toContain("計画 v2 の承認を待っています: 人の決定 1 件");
    expect(html).toContain("人の決定を含む（h1）");
    expect(html).toContain("（段階の後で人の確認）");
    expect(html).toContain(`href="#decision-${D}"`);
    expect(html).toContain(`action="/tasks/${T}"`);
    expect(html).toContain('name="intent" value="plan_gate"');
    for (const a of ["approve", "replan", "withdraw"]) {
      expect(html).toContain(`data-testid="plan-gate-${a}"`);
      expect(html).toMatch(new RegExp(`value="${a}" data-testid="plan-gate-${a}" name="plan_action"`));
    }
    // celeris が plan_gate を出さなければボタンを出さない（押せるかどうかは celeris が決める）。
    const noGate = render(
      createElement(PlanApprovalCard, { item: { ...approval, task: { ...approval.task, actions: ["cancel"] } } }),
    );
    expect(noGate).not.toContain('data-testid="plan-gate-form"');
  });

  it("タスク詳細の「実行の形」カードは承認待ちのとき状態と同じ 3 つのボタンを出し、分解の操作を隠す", () => {
    const t = taskFixture({ id: T, kind: "execute", status: "blocked" }) as Task;
    const execution = {
      metrics: {},
      phase: "awaiting_plan_approval",
      plan_approval: { plan_id: "P", reasons: ["review_human:phase-2"], summary: "人の確認を挟む" },
    } as unknown as ExecutionView;
    const html = render(createElement(ExecutionModeControl, { task: t, execution, actions: ["plan_gate"] }));
    expect(html).toContain('id="execution-mode"');
    expect(html).toContain("計画の承認待ち: 人の確認を挟む");
    expect(html).toContain("人の確認を挟む段階がある（phase-2）");
    expect(html).toContain('data-testid="plan-gate-approve"');
    expect(html).not.toContain('data-testid="execution-mode-form"');
  });
});
