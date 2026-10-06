import type { ExcludedReason, LlmSourceFreshnessView, LlmSourceStateView, ScoreTrace } from "../../api/generated/types";
import { UNKNOWN } from "./routing-catalog";

/**
 * 動的な source の状態・費用・score・除外理由の表示用（ADR 2026-10-04-multi-objective-model-routing §6・§8）。
 * 請求（cash）と枠の機会費用（shadow・resource）は別の欄で、混ぜて一つの金額にしない。
 * 欠測（null）は「不明」と書き、0 円に見せない。選択や score は画面で再計算しない（返された値を言葉にするだけ）。
 */

/** USD。欠測は「不明」。 */
export function usdLabel(value: number | null | undefined): string {
  return value == null ? UNKNOWN : `$${value}`;
}

/** 費用の 4 成分。候補（CandidateTrace）と source 状態（LlmSourceCostView）を同じ形に並べて渡す。 */
export interface CostParts {
  cash: number | null | undefined;
  shadow: number | null | undefined;
  resource: number | null | undefined;
  effective: number | null | undefined;
}

/** 費用の行。請求と機会費用を別の行にし、実効は別に書く。 */
export function costRows(parts: CostParts): Array<{ label: string; value: string }> {
  return [
    { label: "請求（実料金）", value: usdLabel(parts.cash) },
    { label: "枠の機会費用（subscription の残量）", value: usdLabel(parts.shadow) },
    { label: "自前計算の機会費用（GPU・待ち）", value: usdLabel(parts.resource) },
    { label: "実効（請求＋機会費用）", value: usdLabel(parts.effective) },
  ];
}

/** 鮮度。観測が無ければ「不明」、期限切れは古いと書く（reset 時刻の経過だけで満タンとは見なさない）。 */
export function freshnessLabel(freshness: LlmSourceFreshnessView): string {
  if (!freshness.observed_at) return `${UNKNOWN}（観測なし）`;
  const age = freshness.age_secs == null ? "" : `、${freshness.age_secs} 秒前`;
  return freshness.stale ? `古い（期限切れ${age}。再観測が要ります）` : `新しい（${freshness.observed_at}${age}）`;
}

/** 到達性の一語。未知の値はそのまま出す。 */
export function reachabilityLabel(reachability: string): string {
  const known: Record<string, string> = { up: "到達可", down: "届かない", unknown: "未確認" };
  return known[reachability] ?? reachability;
}

/** 候補の除外理由を一語にする。構造化の理由が無ければ旧形の code 列を使う。 */
export function excludedLabel(reason: ExcludedReason | null | undefined, codes: readonly string[] = []): string {
  if (!reason) return codes.length > 0 ? `除外: ${codes.join(", ")}` : "除外";
  switch (reason.kind) {
    case "constraint":
      return `制約に合わない（${reason.name}）`;
    case "quota_exhausted":
      return "残量が枯渇している";
    case "cooldown":
      return "cooldown 中";
    case "concurrency":
      return "同時実行の上限に達している";
    case "rate_limit":
      return "速度制限に達している";
    case "health_down":
      return "到達できない（health down）";
    case "circuit_open":
      return "circuit が開いている";
    case "disabled":
      return "無効になっている";
    case "unknown_required":
      return `必要な値が不明（${reason.field}）`;
    case "quality_invalid":
      return "品質指数が不正";
    case "quality_below_min":
      return "品質が min_quality に届かない";
    case "invalid_estimate":
      return "費用・時間の見積もりが不正";
    case "other":
      return `その他（${reason.code}）`;
  }
}

/** score の内訳。計算されていなければ「不明」、不明の成分は名前で添える。 */
export function scoreLabel(breakdown: ScoreTrace | null | undefined, score?: number | null): string {
  if (!breakdown) return `score ${score == null ? UNKNOWN : score}（内訳なし）`;
  const terms = `品質 ${breakdown.q}×${breakdown.wq} − 費用 ${breakdown.c}×${breakdown.wc} − 遅延 ${breakdown.l}×${breakdown.wl} − 負荷 ${breakdown.p}×${breakdown.wp}`;
  const unknown = breakdown.unknown?.length ? `（不明: ${breakdown.unknown.join(", ")}）` : "";
  return `score ${breakdown.score}: ${terms}${unknown}`;
}

/** 監査の完全性。完全なら None。 */
export function auditIncompleteLabel(
  incomplete: boolean | null | undefined,
  reasons: readonly string[] = [],
): string | null {
  if (!incomplete) return null;
  return `監査が不完全です（${reasons.length > 0 ? reasons.join(", ") : "理由なし"}）。決定と proxy log が結べていません`;
}

/** source 状態の欄を言葉にする。費用は costRows で別に出すので、ここには含めない。 */
export function sourceStateLines(state: LlmSourceStateView): string[] {
  const quota = state.quota_remaining == null ? UNKNOWN : `${state.quota_remaining}`;
  const reset = state.quota_reset_at ? `、reset ${state.quota_reset_at}` : "";
  const lines = [
    `到達 ${reachabilityLabel(state.reachability)}`,
    `観測 ${freshnessLabel(state.freshness)}`,
    `残量 ${quota}${reset}`,
    `圧力 ${state.pressure == null ? UNKNOWN : state.pressure}`,
    `遅延 ${state.latency_ms == null ? `${UNKNOWN}` : `${state.latency_ms} ms`}`,
  ];
  if (state.unknown?.length) lines.push(`不明な欄: ${state.unknown.join(", ")}`);
  return lines;
}
