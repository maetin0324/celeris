import type {
  Billing,
  CatalogDeploymentView,
  CatalogModelView,
  ContextLimits,
  QualityIndex,
  RoutingMode,
  TokenPricing,
} from "../../api/generated/types";

/**
 * routing catalog（`GET /api/llm/routing/catalog`）の表示用。ADR 2026-10-04-multi-objective-model-routing §9。
 * model（能力・品質・価格の正本）と deployment（source × model の配置）を混同しない。
 * 欠測（null）は「不明」と書き、0 円や品質保証に見せない。選択は画面で再計算しない。
 */
export const UNKNOWN = "不明";

export function routingModeLabel(mode: RoutingMode): string {
  const known: Record<RoutingMode, string> = {
    legacy: "legacy（従来の選択）",
    shadow: "shadow（比較の記録だけ）",
    enforce: "enforce（policy で選択）",
  };
  return known[mode] ?? mode;
}

export function billingLabel(billing: Billing): string {
  const known: Record<Billing, string> = {
    subscription: "定額（subscription）",
    metered_api: "従量（API）",
    self_hosted: "自前の計算機（self hosted）",
  };
  return known[billing] ?? billing;
}

/** 能力の有無。null は設定に無い（未確認）なので「不明」。 */
export function capabilityLabel(value: boolean | null | undefined): string {
  if (value === true) return "あり";
  if (value === false) return "なし";
  return UNKNOWN;
}

function usd(value: number | null | undefined): string {
  return value == null ? UNKNOWN : `$${value}`;
}

/** 100 万 token あたりの価格。pricing が無ければ「不明」で、0 円とは書かない。 */
export function pricingLabel(pricing: TokenPricing | null | undefined): string {
  if (!pricing) return `${UNKNOWN}（価格の情報なし。0 円ではありません）`;
  const parts = [
    `入力 ${usd(pricing.input_usd_per_million)}`,
    `出力 ${usd(pricing.output_usd_per_million)}`,
    `cache 入力 ${usd(pricing.cached_input_usd_per_million)}`,
  ];
  const asOf = pricing.as_of ? `、${pricing.as_of} 時点` : "";
  return `${parts.join(" / ")}（100 万 token あたり。出典 ${pricing.provenance}${asOf}）`;
}

/** 品質の指標。無ければ「不明」で、品質を保証しない。 */
export function qualityLabel(quality: readonly QualityIndex[] | null | undefined): string {
  if (!quality || quality.length === 0) return `${UNKNOWN}（評価なし。品質は保証されません）`;
  return quality
    .map(
      (q) =>
        `${q.domain} ${q.index}（${q.evaluation_version}、出典 ${q.provenance}${q.samples == null ? "" : `、${q.samples} 件`}）`,
    )
    .join(" / ");
}

export function contextLabel(limits: ContextLimits): string {
  const n = (v: number | null | undefined) => (v == null ? UNKNOWN : `${v}`);
  return `合計 ${n(limits.total)} / 入力 ${n(limits.input)} / 出力 ${n(limits.output)} token`;
}

export function capabilitiesLabel(model: CatalogModelView): string {
  const c = model.capabilities;
  const efforts = c.reasoning_efforts == null ? UNKNOWN : c.reasoning_efforts.join(", ") || "なし";
  return [
    `tools ${capabilityLabel(c.tools)}`,
    `構造化出力 ${capabilityLabel(c.structured_output)}`,
    `画像 ${capabilityLabel(c.vision)}`,
    `stream ${capabilityLabel(c.streaming)}`,
    `推論の段 ${efforts}`,
  ].join(" / ");
}

/**
 * deployment の価格。deployment の上書きがあればそれ、無ければ model の価格、どちらも無ければ「不明」。
 * 出所（上書き／model／不明）を併記する。
 */
export function deploymentPricingLabel(
  deployment: Pick<CatalogDeploymentView, "price_override" | "model_profile_id">,
  models: readonly Pick<CatalogModelView, "id" | "pricing">[],
): string {
  if (deployment.price_override) return `${pricingLabel(deployment.price_override)}（この配置の上書き）`;
  const model = models.find((m) => m.id === deployment.model_profile_id);
  if (model?.pricing) return `${pricingLabel(model.pricing)}（model の価格）`;
  return pricingLabel(null);
}

/** catalog に無い model を指す deployment は、その旨を言う（model の情報を作らない）。 */
export function deploymentModelNote(
  deployment: Pick<CatalogDeploymentView, "model_profile_id">,
  models: readonly Pick<CatalogModelView, "id">[],
): string | null {
  return models.some((m) => m.id === deployment.model_profile_id)
    ? null
    : `model ${deployment.model_profile_id} は catalog にありません（能力・品質は不明）`;
}
