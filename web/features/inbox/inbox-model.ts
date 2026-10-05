// 受信箱（ADR-0133、docs/api/v1/inbox-notifications.md）の画面側の規則。表示名・破壊的な選択・専用画面への誘導。
// 型は生成型をそのまま使い、写しを作らない。

import { ApiError } from "../../api/client";
import type { InboxItem, InboxKind, InboxOption } from "../../api/generated/types";

export const INBOX_KINDS: readonly InboxKind[] = [
  "decision",
  "plan_gate",
  "phase_gate",
  "authorization",
  "question",
  "acceptance_check",
  "draft_accept",
  "project_plan",
  "failed",
  "unroutable",
  "browser_wait",
  "cluster_login",
  "delivery_skipped",
  "integration_request",
  "knowledge_review",
];

export const KIND_LABELS: Record<InboxKind, string> = {
  decision: "決定",
  plan_gate: "計画の承認",
  phase_gate: "工程の承認",
  authorization: "認可",
  question: "質問",
  acceptance_check: "受け入れの確認",
  draft_accept: "下書きの受け入れ",
  project_plan: "案件の計画",
  failed: "失敗",
  unroutable: "担当が決まらない",
  browser_wait: "ブラウザ操作",
  cluster_login: "クラスタ接続",
  delivery_skipped: "配送の見送り",
  integration_request: "統合の依頼",
  knowledge_review: "知識の候補",
};

export function isInboxKind(value: unknown): value is InboxKind {
  return typeof value === "string" && (INBOX_KINDS as readonly string[]).includes(value);
}

/** 取り消しにくい選択。確認を挟む（FRONTEND_CONTRACT「破壊的操作の確認」）。 */
const DESTRUCTIVE_KEYS = new Set([
  "withdraw",
  "cancel",
  "reject",
  "deny",
  "denied",
  "abandon",
  "discard",
  "delete",
  "drop",
]);

export function isDestructive(option: InboxOption): boolean {
  return DESTRUCTIVE_KEYS.has(option.key);
}

/** 画面から答えられず、専用画面で操作する種類（answer が 409 native_action_required を返す）。 */
const NATIVE_KINDS = new Set<InboxKind>(["cluster_login", "delivery_skipped", "integration_request", "browser_wait"]);

export function needsNativeScreen(item: InboxItem): boolean {
  return NATIVE_KINDS.has(item.kind) || item.options.length === 0;
}

/** 専用画面の行き先。項目の links（web 内の path）を優先し、無ければ種類と task から決める。 */
export function nativeTarget(item: InboxItem): { href: string; label: string } {
  const link = item.links.find((row) => row.href.startsWith("/") && !row.href.startsWith("//"));
  if (link) return { href: link.href, label: link.label };
  if (item.kind === "cluster_login") return { href: "/clusters", label: "クラスタの画面" };
  if (item.task) return { href: `/tasks/${encodeURIComponent(item.task.id)}`, label: `タスク「${item.task.title}」` };
  return { href: "/tasks", label: "タスクの一覧" };
}

export type AnswerOutcome =
  | { ok: true; removed: boolean }
  | { ok: false; message: string; native?: boolean; stale?: boolean; field?: "note" };

function problemCode(body: unknown): string | undefined {
  if (!body || typeof body !== "object") return undefined;
  const record = body as Record<string, unknown>;
  const code = record.code ?? record.error;
  return typeof code === "string" ? code : undefined;
}

function problemDetail(body: unknown): string | undefined {
  if (typeof body === "string") return body;
  if (!body || typeof body !== "object") return undefined;
  const detail = (body as Record<string, unknown>).detail;
  return typeof detail === "string" ? detail : undefined;
}

/** answer の失敗を人の読める文と次の行き先にする。 */
export function answerFailure(error: unknown): Extract<AnswerOutcome, { ok: false }> {
  if (!(error instanceof ApiError)) return { ok: false, message: "答えを送れませんでした。" };
  if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
    return { ok: false, message: "結果を確認できません。一覧を取り直して状態を確かめてください。", stale: true };
  const code = problemCode(error.body);
  if (error.status === 409 && code === "native_action_required")
    return { ok: false, message: "この項目は専用の画面で操作します。", native: true };
  if (error.status === 409)
    return { ok: false, message: "状態が変わりました。最新の一覧を取り直しました。", stale: true };
  if (error.status === 404) return { ok: false, message: "この項目は既に答えられたか、失効しています。", stale: true };
  if (code === "note_required") return { ok: false, message: "この選択には理由（note）が要ります。", field: "note" };
  if (code === "invalid_option")
    return { ok: false, message: "選択肢が古くなっています。最新の一覧を取り直しました。", stale: true };
  return { ok: false, message: problemDetail(error.body) ?? "答えを送れませんでした。" };
}
