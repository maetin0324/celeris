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
