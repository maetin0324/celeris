import type { LlmSourceFreshnessView, LlmSourceStateView } from "../../api/generated/types";

/**
 * 動的な source の状態・費用の表示用（ADR 2026-10-04-multi-objective-model-routing §6・§8）。
 * 請求（cash）と枠の機会費用（shadow・resource）は別の欄で、混ぜて一つの金額にしない。
 * 欠測（null）は「不明」と書き、0 円に見せない。選択は画面で再計算しない（返された値を言葉にするだけ）。
 */

/** 欠測の表示。 */
export const UNKNOWN = "不明";

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
