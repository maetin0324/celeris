import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import { routingCatalogFixture, validateFixture } from "../../e2e/support/fake-daemon.mjs";
import { deploymentModelNote, deploymentPricingLabel, pricingLabel, qualityLabel } from "./routing-catalog";
import { RoutingCatalogList } from "./routing-catalog-section";

describe("routing catalog（ADR 2026-10-04-multi-objective-model-routing §10 Phase 1）", () => {
  it("routing_catalog_missing_metadata: model と deployment を分け、欠測を不明と出して 0 円・品質保証に見せない", () => {
    expect(validateFixture(routingCatalogFixture, schema.properties.routing_catalog)).toEqual([]);
    const out = renderToStaticMarkup(<RoutingCatalogList data={routingCatalogFixture} />);

    // model と deployment は別の一覧で、deployment は source と model を別に持つ。
    expect(out).toContain('aria-label="model の一覧"');
    expect(out).toContain('aria-label="deployment の一覧"');
    expect(out.indexOf("model の一覧")).toBeLessThan(out.indexOf("deployment の一覧"));
    expect(out).toContain('aria-label="model qwen3-coder"');
    expect(out).toContain('aria-label="deployment openai-compatible:qwen/qwen3-coder"');
    expect(out).toContain("Qwen/Qwen3-Coder");
    expect(out).toContain("celeris/cheap");

    // 欠測の価格・品質・能力・context は「不明」。
    const qwen = out.slice(
      out.indexOf('aria-label="model qwen3-coder"'),
      out.indexOf('aria-label="deployment の一覧"'),
    );
    expect(qwen).toContain("不明（価格の情報なし。0 円ではありません）");
    expect(qwen).toContain("不明（評価なし。品質は保証されません）");
    expect(qwen).toContain("tools 不明");
    expect(qwen).toContain("合計 不明");
    expect(qwen).not.toMatch(/\$0|0 円(?!では)/);
    expect(qwen).toContain("text-amber-900");

    // self hosted の配置も価格が無ければ 0 円ではなく不明。
    const qwenDeployment = out.slice(out.indexOf('aria-label="deployment openai-compatible:qwen/qwen3-coder"'));
    expect(qwenDeployment).toContain("自前の計算機");
    expect(qwenDeployment).toContain("0 円ではありません");
    expect(qwenDeployment).not.toContain("$0");

    // 既知の値はそのまま、deployment は model の価格を出所付きで引く。
    expect(out).toContain("coding 0.9");
    expect(deploymentPricingLabel(routingCatalogFixture.deployments[0], routingCatalogFixture.models)).toContain(
      "入力 $15 / 出力 $75 / cache 入力 不明",
    );
    expect(deploymentPricingLabel(routingCatalogFixture.deployments[0], routingCatalogFixture.models)).toContain(
      "model の価格",
    );

    // 明示の 0 は既知の値（不明と区別）、空の品質は不明。catalog に無い model は情報を作らない。
    expect(pricingLabel({ input_usd_per_million: 0, output_usd_per_million: 0, provenance: "config" })).toContain(
      "入力 $0 / 出力 $0",
    );
    expect(qualityLabel([])).toContain("不明");
    expect(deploymentModelNote({ model_profile_id: "ghost" }, routingCatalogFixture.models)).toContain(
      "catalog にありません",
    );
  });
});
