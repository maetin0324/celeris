import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  capabilityLabel,
  contextLimitsLabel,
  deploymentPricingLabel,
  pricingLabel,
  qualityLabel,
} from "~/lib/routing-catalog";
import { RoutingCatalogOverview } from "~/routes/providers";
import { routingCatalogView } from "../mock-celeris/fixtures";

// ADR 2026-10-04-multi-objective-model-routing §9・§10 Phase 1: catalog は model と deployment を
// 別に見せ、欠測（null・空）を「不明」とし 0 円・品質保証に見せない。
const catalog = routingCatalogView();

describe("routing catalog labels", () => {
  it("routing_catalog_missing_metadata: missing price, quality, context and capability read as unknown", () => {
    expect(pricingLabel(null)).toBe("不明");
    expect(pricingLabel({ provenance: "legacy", input_usd_per_million: null, output_usd_per_million: null })).toBe(
      "不明",
    );
    expect(pricingLabel({ provenance: "legacy", input_usd_per_million: 0, output_usd_per_million: null })).toBe(
      "入力 $0 / 出力 不明（100 万 token あたり）",
    );
    expect(qualityLabel(null)).toBe("不明");
    expect(qualityLabel([])).toBe("不明");
    expect(contextLimitsLabel({ input: null, output: null, total: null })).toBe("不明");
    expect(capabilityLabel(null)).toBe("不明");
    expect(capabilityLabel(false)).toBe("非対応");
    expect(deploymentPricingLabel({ price_override: null })).toBe("上書きなし（model の単価に従う）");
  });
});

describe("RoutingCatalogOverview", () => {
  it("routing_catalog_missing_metadata: separates models from deployments and never shows unknown as free or guaranteed", () => {
    const html = renderToStaticMarkup(<RoutingCatalogOverview catalog={catalog} unavailable={false} />);
    // model と deployment は別のカード群。
    expect(html.match(/data-testid="routing-catalog-model"/g)?.length).toBe(2);
    expect(html.match(/data-testid="routing-catalog-deployment"/g)?.length).toBe(1);
    expect(html).toContain('data-deployment-id="local/qwen3-coder"');
    expect(html).toContain("openai_compatible:local");
    expect(html).toContain("自前の host");
    // 欠測は「不明」。0 円・品質の値を捏造しない。
    expect(html).toContain('data-testid="routing-catalog-model-pricing">不明<');
    expect(html).toContain('data-testid="routing-catalog-model-quality">不明<');
    expect(html).not.toContain("$0");
    expect(html).not.toMatch(/-pricing">[^<]*(0 円|無料)/);
    // 既知の値はそのまま。
    expect(html).toContain("入力 $3 / 出力 $15");
    expect(html).toContain("coding 0.8（eval-v1）");
    expect(html).toContain("[llm_proxy.models.cheap] is legacy");
  });

  it("shows an empty state when the catalog is unavailable", () => {
    const html = renderToStaticMarkup(<RoutingCatalogOverview catalog={null} unavailable={true} />);
    expect(html).toContain("[llm_proxy] が設定されていません");
  });
});
