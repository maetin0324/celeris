// 人への決定（ADR-0079 D7、docs/api/v1 §3.125.13）の画面側の規則。答えを変えられるか、回答の履歴、
// revise の本文・失敗の文・結果の読み方をここに集め、部品は表示だけにする。
// revise は `POST /decisions/{id}/revise`（管理系）。最後の `DecisionAnswered` が有効な答え。

import { ApiError, apiGet, apiMutate } from "../../api/client";
import type {
  DecisionEffect,
  DecisionList,
  DecisionOutcome,
  DecisionRequest,
  DecisionView,
  EventRow,
  EventsPage,
} from "../../api/generated/types";
import { decisionKeys as keys } from "../../api/queries/keys";

/** 選択肢を使わず理由（note）だけで答えたときに記録される option。 */
export const FREE_TEXT_OPTION = "other";

export function taskDecisionsPath(taskId: string): string {
  return `/api/tasks/${encodeURIComponent(taskId)}/decisions`;
}

export function decisionKeys(taskId: string) {
  return keys.task(taskId);
}

export const allDecisionsKey = keys.all;

export function taskDecisionsQuery(taskId: string) {
  return {
    queryKey: decisionKeys(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<DecisionList>(taskDecisionsPath(taskId), signal),
  };
}

/** timeline の上限に依存せず、回答イベントを seq 順に最後まで読む。 */
export async function fetchAnswerEvents(taskId: string, signal?: AbortSignal): Promise<EventRow[]> {
  const rows: EventRow[] = [];
  let after = -1;
  while (true) {
    const page = await apiGet<EventsPage>(
      `/api/tasks/${encodeURIComponent(taskId)}/events?types=decision_answered&after_seq=${after}&limit=500`,
      signal,
    );
    rows.push(...page.items);
    if (!page.has_more) return rows;
    const last = page.items.at(-1);
    if (!last || last.seq <= after) throw new Error("回答の履歴を最後まで取得できませんでした。");
    after = last.seq;
  }
}

export function answerEventsQuery(taskId: string) {
  return {
    queryKey: keys.history(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => fetchAnswerEvents(taskId, signal),
  };
}

/** task 詳細の中の決定の位置（hash）。 */
export function decisionAnchor(decisionId: string): string {
  return `decision-${decisionId}`;
}

export function decisionHref(taskId: string, decisionId: string): string {
  return `/tasks/${encodeURIComponent(taskId)}#${decisionAnchor(decisionId)}`;
}

/** 受信箱の項目 id（daemon は `decision-<id>`）とチャットのカード id から決定の id を取り出す。 */
export function decisionIdFromItemId(itemId: string): string | null {
  const match = /^decision[-:](.+)$/.exec(itemId);
  return match?.[1] ? match[1] : null;
}

export const STATUS_LABELS: Record<DecisionView["decision"]["status"], string> = {
  open: "未回答",
  answered: "回答済み",
  withdrawn: "取り下げ済み",
};

export const EFFECT_LABELS: Record<DecisionEffect, string> = {
  resume: "止めていた仕事を再開",
  raise_once: "上限を一度だけ上げる",
  replan: "計画を立て直す",
  atomic: "分けずに 1 つで実行",
  withdraw: "取り下げ",
};

export const BY_LABELS: Record<string, string> = { human: "人", cos: "CoS", daemon: "daemon" };

export function byLabel(by: string): string {
  return BY_LABELS[by] ?? by;
}

export function optionLabel(decision: DecisionRequest, key: string): string {
  if (key === FREE_TEXT_OPTION && !decision.options.some((o) => o.key === key)) return "その他（理由に記述）";
  return decision.options.find((o) => o.key === key)?.label ?? key;
}

export type Revisability = { ok: true } | { ok: false; reason: string };

/** 答えを変えられるか。変えられないなら人の読める理由（daemon が最終的に 409 で決める）。 */
export function revisability(view: DecisionView): Revisability {
  const d = view.decision;
  if (d.status === "open") return { ok: false, reason: "まだ回答されていません。受信箱か判断の画面で答えてください。" };
  if (d.status === "withdrawn") return { ok: false, reason: "取り下げ済みの決定は答えを変えられません。" };
  if (d.kind !== "choice")
    return {
      ok: false,
      reason: "daemon の決定（回答の時点で効き目を当てたもの）は答えを変えられません。",
    };
  if (!d.answer) return { ok: false, reason: "回答の記録が見つかりません。" };
  return { ok: true };
}

export type HistoryEntry = { at: string | null; option: string; label: string; note: string | null; by: string };

/** 回答の履歴は時刻でなく記録順。再取得中も最新の回答を末尾に補う。 */
export function answerHistory(view: DecisionView, events: EventRow[] = []): HistoryEntry[] {
  const d = view.decision;
  const rows: HistoryEntry[] = events
    .filter((item) => item.event.type === "decision_answered" && item.event.id === d.id)
    .sort((a, b) => a.seq - b.seq)
    .flatMap((item) =>
      item.event.type === "decision_answered"
        ? [
            {
              at: item.ts,
              option: item.event.option,
              label: optionLabel(d, item.event.option),
              note: item.event.note ?? null,
              by: item.event.by,
            },
          ]
        : [],
    );
  const last = rows.at(-1);
  if (d.answer && (!last || (view.answered_at && Date.parse(view.answered_at) > Date.parse(last.at ?? ""))))
    rows.push({
      at: view.answered_at ?? null,
      option: d.answer.option,
      label: optionLabel(d, d.answer.option),
      note: d.answer.note ?? null,
      by: d.answer.by,
    });
  return rows;
}

export type ReviseInput = { option: string; note: string };

/** 送信できるか。同じ選択肢なら理由（note）の追記が要る。自由記述（other）は理由が必須。 */
export function reviseReady(view: DecisionView, input: ReviseInput): boolean {
  const note = input.note.trim();
  if (input.option === "") return false;
  if (note.length > NOTE_MAX) return false;
  if (input.option === FREE_TEXT_OPTION && !view.decision.options.some((o) => o.key === FREE_TEXT_OPTION))
    return note !== "";
  return input.option !== view.decision.answer?.option || note !== "";
}

export const NOTE_MAX = 2000;

export function reviseBody(view: DecisionView, input: ReviseInput): { option?: string; note?: string } {
  const note = input.note.trim();
  const freeText = input.option === FREE_TEXT_OPTION && !view.decision.options.some((o) => o.key === FREE_TEXT_OPTION);
  return {
    ...(freeText ? {} : { option: input.option }),
    ...(note ? { note } : {}),
  };
}

export function revisePath(decisionId: string): string {
  return `/api/decisions/${encodeURIComponent(decisionId)}/revise`;
}

export function reviseDecision(view: DecisionView, input: ReviseInput): Promise<DecisionOutcome> {
  return apiMutate<DecisionOutcome>("POST", revisePath(view.decision.id), reviseBody(view, input));
}

function bodyField(body: unknown, field: string): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const value = (body as Record<string, unknown>)[field];
  return typeof value === "string" ? value : undefined;
}

/** revise の失敗を人の読める文にする。`stale` は一覧を読み直すべきもの。 */
export function reviseFailure(error: unknown): { message: string; stale: boolean } {
  if (!(error instanceof ApiError)) return { message: "答えを変えられませんでした。", stale: false };
  if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
    return {
      message: "結果を確認できません。送り直す前に、決定の回答の履歴を読み直して変わったかを確かめてください。",
      stale: true,
    };
  if (error.status === 401 || error.status === 403)
    return { message: "答えを変えるには管理の権限が要ります。ログインし直してください。", stale: false };
  if (error.status === 404)
    return { message: "この決定は見つかりません（消えたか、木の実行が無効です）。", stale: true };
  if (error.status === 409) {
    const status = bodyField(error.body, "decision_status");
    const label = status && status in STATUS_LABELS ? STATUS_LABELS[status as keyof typeof STATUS_LABELS] : null;
    return {
      message: `この決定は今は答えを変えられません${label ? `（状態: ${label}）` : ""}。決定を出したタスクが終わっているか、状態が変わりました。最新の状態と回答の履歴を確認してください。`,
      stale: true,
    };
  }
  if (error.status === 400 || error.status === 422) {
    const detail = bodyField(error.body, "detail") ?? bodyField(error.body, "message");
    return { message: `選択肢か理由が受け付けられませんでした${detail ? `: ${detail}` : "。"}`, stale: false };
  }
  return {
    message: bodyField(error.body, "detail") ?? "答えを変えられませんでした。時間をおいて試してください。",
    stale: false,
  };
}

/** revise の結果の要約（画面に出す行）。 */
export function reviseSummary(outcome: DecisionOutcome): string[] {
  const d = outcome.decision.decision;
  const lines = [`答えを「${d.answer ? optionLabel(d, d.answer.option) : "?"}」に変えました。`];
  const effect =
    outcome.effect === "resume" && outcome.resumed.length === 0
      ? "新しい回答を後続の仕事で参照"
      : (EFFECT_LABELS[outcome.effect] ?? outcome.effect);
  lines.push(`効き目: ${effect}`);
  const notified = outcome.notified_children ?? [];
  lines.push(
    notified.length > 0
      ? `既に作られた子タスク ${notified.length} 件にコメントで新しい答えを届けました。`
      : "既に作られた子タスクへの通知はありません（これから作られる子・走る仕事が新しい答えを読みます）。",
  );
  if (outcome.resumed.length > 0) lines.push(`再開した unit: ${outcome.resumed.join(", ")}`);
  if (outcome.cancelled.length > 0) lines.push(`取り消した unit: ${outcome.cancelled.join(", ")}`);
  if (outcome.replan_requested) lines.push("計画の立て直しを依頼しました。");
  return lines;
}

/** 決定の id から詳細の行き先を引く。見つからなければ null。 */
export async function locateDecision(decisionId: string): Promise<string | null> {
  const list = await apiGet<DecisionList>("/api/decisions");
  const view = list.items.find((item) => item.decision.id === decisionId);
  return view ? decisionHref(view.task_id, decisionId) : null;
}
