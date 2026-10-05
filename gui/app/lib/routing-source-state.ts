import type {
  ActualSource,
  CandidateTrace,
  ExcludedReason,
  LlmSourceCostView,
  LlmSourceFreshnessView,
  LlmSourceStateView,
  RequestRoutingAudit,
  ScoreTrace,
} from "~/celeris/types";
import { formatDuration } from "./time-delta";

/**
 * 多目的 routing（ADR 2026-10-04-multi-objective-model-routing Phase 2）の表示用の純関数。
 * celeris が返した値（`LlmSourceStateView` / `CandidateTrace` / `RequestRoutingAudit`）を並べるだけで、
 * score・費用・除外の判断は GUI で再計算しない。値が無いもの（null・欠測）は「不明」と出し、
 * 0 円・満タン・品質保証には見せない。
 */

export const UNKNOWN_LABEL = "不明";

/** 0.0〜1.0 を % に。無い・非有限は「不明」。 */
export function percentLabel(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return UNKNOWN_LABEL;
  return `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`;
}

/** USD。無い・非有限は「不明」（0 で埋めない）。 */
export function usdLabel(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return UNKNOWN_LABEL;
  return `$${value < 0.01 && value > 0 ? value.toFixed(4) : value.toFixed(2)}`;
}

export function latencyLabel(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return UNKNOWN_LABEL;
  return `${Math.round(ms)} ms`;
}

const REACHABILITY_LABEL: Record<string, string> = { up: "到達", down: "不通", unknown: UNKNOWN_LABEL };

export function reachabilityLabel(reachability: string | null | undefined): string {
  return REACHABILITY_LABEL[reachability ?? "unknown"] ?? UNKNOWN_LABEL;
}

/**
 * 観測の鮮度。未観測は「未観測」、期限切れ（stale）は古い値として判定に使わないことを明示する。
 * `stale` を返すので、呼び手は警告色の判断だけをする。
 */
export function freshnessLabel(freshness: LlmSourceFreshnessView | null | undefined): {
  text: string;
  stale: boolean;
} {
  if (!freshness?.observed_at) return { text: "未観測（判定に使わない）", stale: true };
  const age = freshness.age_secs;
  const ageText = age == null || !Number.isFinite(age) ? UNKNOWN_LABEL : `${formatDuration(Math.max(0, age))} 前`;
  return freshness.stale
    ? { text: `${ageText}（古い・判定に使わない）`, stale: true }
    : { text: ageText, stale: false };
}

/**
 * reset までの残り。`nowMs` は注入する（時計に依存させない）。時刻が読めなければ「不明」、
 * 過ぎていれば「reset 時刻を過ぎています」（残量の値は古い可能性がある）。
 */
export function resetLabel(resetAt: string | null | undefined, nowMs: number): string {
  if (!resetAt) return UNKNOWN_LABEL;
  const at = Date.parse(resetAt);
  if (!Number.isFinite(at)) return UNKNOWN_LABEL;
  const remainSecs = Math.floor((at - nowMs) / 1000);
  if (remainSecs <= 0) return "reset 時刻を過ぎています";
  return `あと ${formatDuration(remainSecs)}`;
}

/** 費用の成分。請求（cash）と機会費用（shadow・resource）は別の行にする。 */
export interface CostRow {
  key: "cash" | "shadow" | "resource" | "effective";
  label: string;
  value: string;
  /** 機会費用（請求ではない）か。表示の区別に使う。 */
  opportunity: boolean;
}

export function costRows(cost: LlmSourceCostView | null | undefined): CostRow[] {
  return [
    { key: "cash", label: "請求（API の実料金）", value: usdLabel(cost?.billed.cash_usd), opportunity: false },
    {
      key: "shadow",
      label: "機会費用（subscription の残量）",
      value: usdLabel(cost?.opportunity.shadow_usd),
      opportunity: true,
    },
    {
      key: "resource",
      label: "機会費用（self-host の資源）",
      value: usdLabel(cost?.opportunity.resource_usd),
      opportunity: true,
    },
    { key: "effective", label: "実効費用（請求＋機会）", value: usdLabel(cost?.effective_usd), opportunity: false },
  ];
}

const UNKNOWN_FIELD_LABEL: Record<string, string> = {
  latency: "遅延",
  quota: "残量",
  quota_reset: "reset 時刻",
  pressure: "負荷",
  cash: "請求",
  shadow: "機会費用（残量）",
  resource: "機会費用（資源）",
  effective: "実効費用",
  observed_at: "観測時刻",
};

/** `LlmSourceStateView.unknown` の名前を人の語に（未知の名前はそのまま）。 */
export function unknownFieldLabels(names: readonly string[] | null | undefined): string[] {
  return (names ?? []).map((n) => UNKNOWN_FIELD_LABEL[n] ?? n);
}

/** 1 deployment の概要（到達性・遅延・残量・reset・負荷）。 */
export function deploymentSummary(state: LlmSourceStateView, nowMs: number) {
  return {
    reachability: reachabilityLabel(state.reachability),
    latency: latencyLabel(state.latency_ms),
    quota: percentLabel(state.quota_remaining),
    quotaReset: resetLabel(state.quota_reset_at, nowMs),
    pressure: percentLabel(state.pressure),
    freshness: freshnessLabel(state.freshness),
    unknown: unknownFieldLabels(state.unknown),
  };
}

const EXCLUDED_KIND_LABEL: Record<string, string> = {
  quota_exhausted: "残量切れ",
  cooldown: "cooldown 中",
  concurrency: "同時実行の上限",
  rate_limit: "レート制限",
  health_down: "到達できない（health）",
  circuit_open: "遮断中（circuit open）",
  disabled: "無効",
  quality_invalid: "品質の値が不正",
  quality_below_min: "品質が下限未満",
  invalid_estimate: "費用の見積もりが不正",
};

/** 除外理由を人の語に。除外されていなければ `null`。 */
export function excludedReasonLabel(reason: ExcludedReason | null | undefined): string | null {
  if (!reason) return null;
  switch (reason.kind) {
    case "constraint":
      return `制約（${reason.name}）`;
    case "unknown_required":
      return `${reason.field} が不明（必須の制約）`;
    case "other":
      return `その他（${reason.code}）`;
    default:
      return EXCLUDED_KIND_LABEL[reason.kind] ?? reason.kind;
  }
}

/**
 * 候補の状態（除外理由があれば理由、無ければ「採用候補」）。`excluded_reason` が無い旧 event では
 * 生の `excluded_reasons`（コード）をそのまま出す。
 */
export function candidateStatusLabel(candidate: CandidateTrace): { label: string; excluded: boolean } {
  const reason = excludedReasonLabel(candidate.excluded_reason);
  if (reason) return { label: reason, excluded: true };
  if (candidate.excluded_reasons.length > 0) return { label: candidate.excluded_reasons.join("・"), excluded: true };
  return { label: "採用候補", excluded: false };
}

/** score の内訳の 1 行。`value` は C/L/P が未知のとき 1 で計算された値（`unknownNote` で注記）。 */
export interface ScoreRow {
  key: "q" | "c" | "l" | "p";
  label: string;
  value: number;
  weight: number;
  unknownNote: string | null;
}

/** score = wq·Q − wc·C − wl·L − wp·P（ADR §4）の各項。C/L/P の未知は `*_unknown` の印で示す。 */
export function scoreRows(score: ScoreTrace | null | undefined): ScoreRow[] {
  if (!score) return [];
  const unknown = new Set(score.unknown ?? []);
  const note = (name: string) => (unknown.has(name) ? "不明（1 として計算）" : null);
  return [
    { key: "q", label: "品質 Q", value: score.q, weight: score.wq, unknownNote: null },
    { key: "c", label: "費用 C", value: score.c, weight: score.wc, unknownNote: note("cost_unknown") },
    { key: "l", label: "遅延 L", value: score.l, weight: score.wl, unknownNote: note("latency_unknown") },
    { key: "p", label: "負荷 P", value: score.p, weight: score.wp, unknownNote: note("pressure_unknown") },
  ];
}

/**
 * 要求の最終の source・model・account。proxy log に結べたときは log の実際の行き先を優先し、
 * 無ければ決定（trace）の値を使う。どちらも無ければ「不明」。
 */
export function finalSelection(request: RequestRoutingAudit): { source: string; model: string; account: string } {
  const log = request.log;
  const pick = (fromLog: string | null | undefined, fromTrace: string | null | undefined) =>
    fromLog ?? fromTrace ?? UNKNOWN_LABEL;
  return {
    source: pick(log?.source_id, request.trace.source_id),
    model: pick(log?.model, request.trace.model),
    account: pick(log?.account, request.trace.account_id),
  };
}

const INCOMPLETE_LABEL: Record<string, string> = {
  request_id_missing: "要求 ID が無い",
  request_log_missing: "proxy log に対応する要求が無い",
  request_log_mismatch: "proxy log と決定が食い違う",
};

export function incompleteReasonLabel(code: string | null | undefined): string | null {
  if (!code) return null;
  return INCOMPLETE_LABEL[code] ?? code;
}

/**
 * Phase 3: 要求が実際に使った source/model/account（出所 `from` を人の語にして併記する）。
 * secret を含まない ID だけ。出所は celeris が `proxy_log` / `request_attempts` / `proxy_trace` の
 * 順で決めている（proxy log と結べた → 試した source の最後 → proxy の決定）。
 */
const ACTUAL_FROM_LABEL: Record<string, string> = {
  proxy_log: "proxy log",
  request_attempts: "試した source の最後",
  proxy_trace: "proxy の決定",
};

export function actualSourceLine(actual: ActualSource): string {
  const from = ACTUAL_FROM_LABEL[actual.from] ?? actual.from;
  const parts = [
    actual.source_id ? `source ${actual.source_id}` : null,
    actual.model ? `model ${actual.model}` : null,
    actual.account ? `account ${actual.account}` : null,
  ].filter((p): p is string => p !== null);
  return `${parts.join(" / ") || UNKNOWN_LABEL}（${from}）`;
}

/** 監査が不完全なときの注記（完全なら `null`）。理由は整列済みの配列をそのまま人の語にする。 */
export function auditIncompleteNote(
  incomplete: boolean | null | undefined,
  reasons: readonly string[] | null | undefined,
): string | null {
  if (!incomplete) return null;
  const labels = (reasons ?? []).map((r) => incompleteReasonLabel(r) ?? r);
  return labels.length > 0 ? `監査が不完全です（${labels.join("・")}）` : "監査が不完全です";
}
