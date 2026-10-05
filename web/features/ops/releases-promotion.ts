import type { ReleaseItem } from "../../api/generated/types";

// 昇格の結果判定（P4-16）。POST の 202 は「起動した」だけで、成功ではない。
// 結果は GET /releases の行（promoting / promote_failed / promoted_at）が確定させる。
export type PromotionOutcome =
  | { state: "pending"; detail: string }
  | { state: "succeeded" }
  | { state: "failed"; error: string };

export type PromotionTrack = {
  sha12: string;
  /** 昇格を押した時点の promoted_at（これが変わったら成功）。 */
  baselinePromotedAt: string | null;
  /** 202 の started_at。無ければ（POST の応答を受けられなかった）押した時刻。 */
  startedAt: string;
};

export function judgePromotion(item: ReleaseItem | undefined, track: PromotionTrack): PromotionOutcome {
  if (!item) return { state: "pending", detail: "リリースの状態を確認しています" };
  if (item.promoting) return { state: "pending", detail: item.promote_last_line ?? "昇格を実行中です" };
  if (item.promote_failed && item.promote_failed.failed_at >= track.startedAt)
    return { state: "failed", error: item.promote_failed.error };
  if (item.promote_stale)
    return { state: "failed", error: "昇格が途中で止まりました（promote.log を確認してください）" };
  if (item.is_current && (item.promoted_at ?? null) !== track.baselinePromotedAt) return { state: "succeeded" };
  return { state: "pending", detail: "昇格の結果を待っています" };
}

/** 再取得の待ち時間。失敗が続くほど延ばす（gateway の入れ替え中は接続を拒否される）。 */
export function pollDelayMs(consecutiveFailures: number, baseMs = 1000, maxMs = 5000): number {
  if (consecutiveFailures <= 0) return baseMs;
  return Math.min(maxMs, baseMs * 2 ** consecutiveFailures);
}

export function canPromote(item: ReleaseItem): boolean {
  return item.gate_ok && !item.is_current && !item.promoting && !item.problem;
}

/** 前の版を再び昇格するのは巻き戻し。操作は同じ promote だが、人には別の動詞で示す。 */
export function isRollback(item: ReleaseItem): boolean {
  return item.is_previous && !item.is_current;
}

/** 操作の名前（動詞と対象）。一覧のボタンと確認の確定ボタンで同じ文言を使う。 */
export function promotionActionLabel(item: ReleaseItem): string {
  return isRollback(item) ? `${item.sha12} に巻き戻す` : `${item.sha12} を昇格する`;
}

/** 昇格の結果を StatusBadge の状態語へ写す（進行中 running・成功 done・失敗 failed）。 */
export function promotionStatus(outcome: PromotionOutcome): "running" | "done" | "failed" {
  if (outcome.state === "pending") return "running";
  return outcome.state === "succeeded" ? "done" : "failed";
}

export type PromotionConfirmCopy = {
  title: string;
  target: string;
  consequence: string;
  reversibility: string;
};

/** 確認表示の文言。対象版・現在版・影響（本番 daemon が引き継ぐ）を必ず入れる。 */
export function promotionConfirmCopy(item: ReleaseItem, current: string | null | undefined): PromotionConfirmCopy {
  const now = current ?? "なし";
  const rollback = isRollback(item);
  return {
    title: rollback ? `前の版 ${item.sha12} に巻き戻しますか` : `${item.sha12} を本番に昇格しますか`,
    target: `対象版 ${item.sha12}${item.ref ? `（ref ${item.ref}）` : ""}`,
    consequence: `現在版 ${now} から ${item.sha12} に切り替えます。影響: 本番の daemon が ${item.sha12} に引き継がれ、引き継ぎの間は画面と API が一時的に切れます。`,
    reversibility: current
      ? `元の版 ${current} をこの画面から再び昇格すれば戻せます。`
      : "元の版が無いため、この画面からは戻せません。",
  };
}
