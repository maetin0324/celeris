/**
 * `/providers` の「model routing catalog」節（ADR 2026-10-04-multi-objective-model-routing §9・§10 Phase 1）で使う純関数。
 * `GET /llm/routing/catalog`（`RoutingCatalogView`）の値を表示用に整形するだけで、選択・費用・品質の
 * 計算はしない（選ぶのは celeris の policy。gui/CLAUDE.md の禁止事項）。
 * model（`models[]`）と deployment（`deployments[]` = source と model の対応）は別の物として出す。
 * 欠測（`null`・空配列）は「不明」とし、0 円・品質保証のように見せない（値を捏造しない。ADR-0024 D3 と同じ規律）。
 */

import type { Billing, CatalogDeploymentView, ContextLimits, QualityIndex, TokenPricing } from "~/celeris/types";

/** 欠測の表示語。 */
export const UNKNOWN_LABEL = "不明";

function usd(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return UNKNOWN_LABEL;
  return `$${value}`;
}

/**
 * 100 万 token あたりの単価を「入力 / 出力」の一行に。価格表が無い（`null`）・欄が欠けるものは「不明」。
 * 0 は celeris が 0 と返したときだけ `$0` と出す（欠測を 0 円にしない）。
 */
export function pricingLabel(pricing: TokenPricing | null | undefined): string {
  if (!pricing) return UNKNOWN_LABEL;
  const input = usd(pricing.input_usd_per_million);
  const output = usd(pricing.output_usd_per_million);
  if (input === UNKNOWN_LABEL && output === UNKNOWN_LABEL) return UNKNOWN_LABEL;
  return `入力 ${input} / 出力 ${output}（100 万 token あたり）`;
}

/** 品質指標の一覧を一行に。指標が無い（`null`・空）は「不明」（未評価を品質保証に見せない）。 */
export function qualityLabel(quality: readonly QualityIndex[] | null | undefined): string {
  if (!quality || quality.length === 0) return UNKNOWN_LABEL;
  return quality.map((q) => `${q.domain} ${q.index}（${q.evaluation_version}）`).join(", ");
}

/** context limit を一行に。全部欠けていれば「不明」。 */
export function contextLimitsLabel(limits: ContextLimits | null | undefined): string {
  const parts: string[] = [];
  if (limits?.total != null) parts.push(`合計 ${limits.total}`);
  if (limits?.input != null) parts.push(`入力 ${limits.input}`);
  if (limits?.output != null) parts.push(`出力 ${limits.output}`);
  return parts.length > 0 ? `${parts.join(" / ")} token` : UNKNOWN_LABEL;
}

/** 能力の対応（`true`/`false`/`null`）を一語に。`null` は「不明」（未対応とも対応とも言わない）。 */
export function capabilityLabel(value: boolean | null | undefined): string {
  if (value === true) return "対応";
  if (value === false) return "非対応";
  return UNKNOWN_LABEL;
}

/** 課金種別の一語。subscription でも単価は別（0 円とは言わない）。 */
export function billingLabel(billing: Billing): string {
  const known: Record<Billing, string> = {
    subscription: "サブスクリプション",
    metered_api: "従量 API",
    self_hosted: "自前の host",
  };
  return known[billing] ?? billing;
}

/**
 * deployment の単価の表示。上書きがあればそれ、無ければ model の単価を参照すると明示する
 * （model の単価をここで写して合算しない。どれを使うかは celeris が決める）。
 */
export function deploymentPricingLabel(deployment: Pick<CatalogDeploymentView, "price_override">): string {
  if (deployment.price_override) return pricingLabel(deployment.price_override);
  return "上書きなし（model の単価に従う）";
}
