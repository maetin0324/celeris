// チャット内カード（ADR 2026-10-05-cos-chat-home D5 の 6 項目目・D3「代答・取り消し・差し戻し」）の画面側の規則。
// 表示名・その場で答えられるか・代答を取り消せるか・失敗の読み方をここに集め、部品は表示だけにする。

import { ApiError } from "../../../api/client";
import type { ChatActor, ChatCard, ChatCardKind, OverrideMode, OverrideResponse } from "../../../api/generated/types";
import type { BadgeTone } from "../../../components/ui/badge";
import { statusView } from "../../../components/ui/status-badge";

export const CARD_KIND_LABELS: Record<ChatCardKind, string> = {
  task: "タスク",
  decision: "決定",
  question: "質問",
  approval: "認可",
  plan_gate: "計画の承認",
  notice: "知らせ",
  operation: "CoS の代答",
};

export const ACTOR_LABELS: Record<ChatActor, string> = {
  human: "人",
  cos: "CoS",
  system: "システム",
};

/** その場で答えられる種類。カードの id は受信箱の項目 id（triage の source_key）。 */
const ANSWER_KINDS = new Set<ChatCardKind>(["decision", "question", "approval", "plan_gate"]);

export function isAnswerKind(kind: ChatCardKind): boolean {
  return ANSWER_KINDS.has(kind);
}

/** 回答済み・置き換え済みなど、もう操作できない状態。 */
const CLOSED_STATES = new Set([
  "answered",
  "resolved",
  "observed",
  "superseded",
  "revoked",
  "returned",
  "needs_remediation",
  "cancelled",
  "expired",
  "withdrawn",
  "closed",
  "done",
  "failed",
]);

export function isClosedState(state: string): boolean {
  return CLOSED_STATES.has(state);
}

/** 人が答えるべき待ちとして数える状態。 */
const WAITING_STATES = new Set(["pending", "escalated"]);

/** 人待ちの件数（受信箱 thread の控えめな badge 用）。同じ card が何度出ても 1 件に数える。 */
export function pendingHumanCount(cards: Iterable<ChatCard>): number {
  const keys = new Set<string>();
  for (const card of cards) {
    if (isAnswerKind(card.kind) && WAITING_STATES.has(card.state)) keys.add(`${card.kind}:${card.id}`);
  }
  return keys.size;
}

/** CoS 代答（operation）を人が取消・差し戻しできるか。適用済みの CoS の操作だけ。 */
export function canOverride(card: ChatCard): boolean {
  return card.kind === "operation" && card.actor === "cos" && card.state === "applied" && overrideTarget(card) !== "";
}

export function overrideTarget(card: ChatCard): string {
  return card.operation_id ?? (card.kind === "operation" ? card.id : "");
}

const CARD_STATES: Record<string, { tone: BadgeTone; label: string }> = {
  pending: { tone: "warning", label: "人待ち" },
  escalated: { tone: "warning", label: "人の判断待ち" },
  running: { tone: "running", label: "対応中" },
  applied: { tone: "success", label: "適用済み" },
  answered: { tone: "success", label: "回答済み" },
  resolved: { tone: "success", label: "解決済み" },
  observed: { tone: "neutral", label: "確認のみ" },
  superseded: { tone: "neutral", label: "置き換え済み" },
  revoked: { tone: "neutral", label: "取り消し済み" },
  returned: { tone: "neutral", label: "差し戻し済み" },
  needs_remediation: { tone: "danger", label: "修正が必要" },
  expired: { tone: "neutral", label: "失効" },
};

export function cardStateView(state: string): { tone: BadgeTone; label: string } {
  return CARD_STATES[state] ?? statusView(state);
}

export const OVERRIDE_LABELS: Record<OverrideMode, string> = { revoke: "取消", return: "差し戻し" };

function problemCode(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const code = (body as Record<string, unknown>).code ?? (body as Record<string, unknown>).error;
  return typeof code === "string" ? code : undefined;
}

function problemDetail(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const detail = (body as Record<string, unknown>).detail;
  return typeof detail === "string" ? detail : undefined;
}

export type OverrideOutcome =
  | { ok: true; mode: OverrideMode; state: string; remediationTaskId: string | null; pausedTaskIds: string[] }
  | { ok: false; message: string; conflict: boolean; stale: boolean };

export function overrideSuccess(
  mode: OverrideMode,
  response: OverrideResponse,
): Extract<OverrideOutcome, { ok: true }> {
  return {
    ok: true,
    mode,
    state: response.state,
    remediationTaskId: response.remediation_task_id ?? null,
    pausedTaskIds: response.paused_task_ids,
  };
}

/** override の失敗を人の読める文にする。409 は人と CoS の競合（先に別の回答・取消が入った）。 */
export function overrideFailure(error: unknown): Extract<OverrideOutcome, { ok: false }> {
  if (!(error instanceof ApiError))
    return { ok: false, message: "操作を送れませんでした。", conflict: false, stale: false };
  if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
    return {
      ok: false,
      message: "結果を確認できません。会話を開き直して状態を確かめてください。",
      conflict: false,
      stale: true,
    };
  if (error.status === 409)
    return {
      ok: false,
      message:
        "人と CoS の操作が競合しました。先に別の回答・取消が入っています。最新の状態を確かめてから操作し直してください。",
      conflict: true,
      stale: true,
    };
  if (error.status === 403)
    return { ok: false, message: "この操作は人の認証でだけ行えます。", conflict: false, stale: false };
  if (error.status === 404) return { ok: false, message: "この代答は見つかりません。", conflict: false, stale: true };
  if (error.status === 422 && problemCode(error.body) === "validation")
    return { ok: false, message: problemDetail(error.body) ?? "理由が要ります。", conflict: false, stale: false };
  return {
    ok: false,
    message: problemDetail(error.body) ?? "操作を完了できませんでした。",
    conflict: false,
    stale: false,
  };
}

/** web 内の path だけを link にする（外部 URL・protocol 相対は出さない）。 */
export function internalHref(href: string): string | null {
  return href.startsWith("/") && !href.startsWith("//") ? href : null;
}

export function taskHref(taskId: string): string {
  return `/tasks/${encodeURIComponent(taskId)}`;
}
