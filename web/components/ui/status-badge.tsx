import type { RunOutcomeKind, Status, WorkUnitStatus } from "../../api/generated/types";
import { Badge, type BadgeTone } from "./badge";

// Celeris の状態語（task の Status・WorkUnit の WorkUnitStatus・run の RunOutcomeKind）。
// 写像表を Record にして、API の enum に値が増えたら typecheck で気づけるようにする。
export type CelerisStatus = Status | WorkUnitStatus | RunOutcomeKind;

export const statusTone: Readonly<Record<CelerisStatus, BadgeTone>> = {
  // task（Status）
  draft: "neutral",
  ready: "info",
  running: "running",
  blocked: "warning",
  reviewing: "info",
  done: "success",
  failed: "danger",
  cancelled: "neutral",
  // WorkUnit（WorkUnitStatus）
  pending: "neutral",
  needs_continuation: "warning",
  superseded: "neutral",
  // run の結果（RunOutcomeKind）
  question: "warning",
  error: "danger",
  requeue: "info",
  lease_expired: "warning",
  interrupted: "warning",
  continued: "info",
};

// 可視ラベル。色だけで意味を伝えないため、badge には必ずこの文字列（未知なら原文）を出す。
export const statusLabel: Readonly<Record<CelerisStatus, string>> = {
  draft: "下書き",
  ready: "実行待ち",
  running: "実行中",
  blocked: "停止中",
  reviewing: "レビュー中",
  done: "完了",
  failed: "失敗",
  cancelled: "中止",
  pending: "未着手",
  needs_continuation: "続きが必要",
  superseded: "置き換え済み",
  question: "質問",
  error: "エラー",
  requeue: "再投入",
  lease_expired: "期限切れ",
  interrupted: "中断",
  continued: "継続",
};

function isKnownStatus(status: string): status is CelerisStatus {
  return Object.hasOwn(statusTone, status);
}

/** 状態語を tone と可視ラベルへ写す。未知の状態は neutral で原文をそのまま返す。 */
export function statusView(status: string): { tone: BadgeTone; label: string } {
  if (isKnownStatus(status)) return { tone: statusTone[status], label: statusLabel[status] };
  return { tone: "neutral", label: status };
}

export function StatusBadge({ status, className }: { status: string; className?: string }) {
  const { tone, label } = statusView(status);
  return (
    <Badge tone={tone} data-status={status} className={className}>
      {tone === "running" ? (
        // 実行中の印。形（切れ目のある輪）と文字で認識でき、reduced-motion では回転を止める。
        <span
          aria-hidden="true"
          data-slot="status-mark"
          className="size-2 shrink-0 rounded-full border-2 border-current border-t-transparent motion-safe:animate-spin"
        />
      ) : null}
      {label}
    </Badge>
  );
}
