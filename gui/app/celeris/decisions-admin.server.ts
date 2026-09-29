import type { ActionError, DecisionActionOutcome, TaskPlanGateOutcome } from "./action-types";
import { toActionError } from "./actions.server";
import type { CelerisClient } from "./client.server";
import { formString } from "./forms";
import type {
  DecisionAnswerBody,
  DecisionOutcome,
  DecisionWithdrawBody,
  PlanGateAction,
  PlanGateRequest,
  TransitionResult,
} from "./types";

/**
 * celeris ADR-0079 D7 / D8（Phase R4b）: 決定の要求への回答・取り下げと、root の計画の承認（**管理系**）。
 * `tasks-admin.server.ts` の `phaseGateTask` と同じ作り: フォームの値を本文に写すだけで **GUI は検証しない**
 * （選択肢の外・空の replan の指示は celeris が 422、状態が変わっていれば 409 を返し、その文言を画面に出す）。
 * 知らない操作・id の無いフォームは送らずに 400 相当の失敗にする（フォームの改ざん）。
 */

function badRequest(detail: string): ActionError {
  return { status: 400, code: "bad_request", detail, conflict: false, fields: {}, messages: [] };
}

/** 空白だけの欄は送らない（celeris の既定 = 無し）。 */
function trimmed(form: FormData, name: string): string | null {
  const v = formString(form, name);
  return v !== null && v.trim() !== "" ? v : null;
}

/**
 * `POST /decisions/{id}/answer`。フォームの `decision_id`・`option`（空 = 自由記述。`kind = choice` だけ celeris が
 * 受け付け、`other` と記録する）・`note`（任意）。
 */
export async function answerDecision(
  client: CelerisClient,
  form: FormData,
  signal?: AbortSignal,
): Promise<DecisionActionOutcome> {
  const decisionId = formString(form, "decision_id");
  if (decisionId === null) {
    return { ok: false, op: "decision_answer", decisionId: "", error: badRequest("decision_id is required") };
  }
  const body: DecisionAnswerBody = {};
  const option = formString(form, "option");
  if (option !== null) body.option = option;
  const note = trimmed(form, "note");
  if (note !== null) body.note = note;
  try {
    const result = await client.post<DecisionOutcome>(`/decisions/${encodeURIComponent(decisionId)}/answer`, body, {
      signal,
    });
    return { ok: true, op: "decision_answer", decisionId, result };
  } catch (e) {
    return { ok: false, op: "decision_answer", decisionId, error: toActionError(e) };
  }
}

/** `POST /decisions/{id}/withdraw`。フォームの `note` を取り下げの理由（`reason`）として送る（任意）。 */
export async function withdrawDecision(
  client: CelerisClient,
  form: FormData,
  signal?: AbortSignal,
): Promise<DecisionActionOutcome> {
  const decisionId = formString(form, "decision_id");
  if (decisionId === null) {
    return { ok: false, op: "decision_withdraw", decisionId: "", error: badRequest("decision_id is required") };
  }
  const body: DecisionWithdrawBody = {};
  const reason = trimmed(form, "note");
  if (reason !== null) body.reason = reason;
  try {
    const result = await client.post<DecisionOutcome>(`/decisions/${encodeURIComponent(decisionId)}/withdraw`, body, {
      signal,
    });
    return { ok: true, op: "decision_withdraw", decisionId, result };
  } catch (e) {
    return { ok: false, op: "decision_withdraw", decisionId, error: toActionError(e) };
  }
}

const PLAN_GATE_ACTIONS: readonly PlanGateAction[] = ["approve", "replan", "withdraw"];

/**
 * `POST /tasks/{id}/execution/plan-gate`。フォームの `plan_action`（approve / replan / withdraw）と `note`
 * （approve では任意、replan では必須 = 空なら celeris が 422）。
 */
export async function planGateTask(
  client: CelerisClient,
  taskId: string,
  form: FormData,
  signal?: AbortSignal,
): Promise<TaskPlanGateOutcome> {
  const action = formString(form, "plan_action");
  if (action === null || !(PLAN_GATE_ACTIONS as readonly string[]).includes(action)) {
    return { ok: false, op: "plan_gate", taskId, error: badRequest(`unknown plan_action: ${String(action)}`) };
  }
  const body: PlanGateRequest = { action: action as PlanGateAction };
  const note = formString(form, "note");
  if (note !== null) body.note = note;
  try {
    const result = await client.post<TransitionResult>(
      `/tasks/${encodeURIComponent(taskId)}/execution/plan-gate`,
      body,
      { signal },
    );
    return { ok: true, op: "plan_gate", taskId, result };
  } catch (e) {
    return { ok: false, op: "plan_gate", taskId, error: toActionError(e) };
  }
}
