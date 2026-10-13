import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import { llmSourcesFixture, validateFixture } from "../../e2e/support/fake-daemon.mjs";
import { costRows, freshnessLabel, sourceStateLines, usdLabel } from "./routing-state";
import { DeploymentStateList } from "./routing-state-view";

function fail(): never {
  throw new Error("fixture に deployment が無い");
}

describe("動的な source 状態・費用（ADR 2026-10-04-multi-objective-model-routing §6・§8 Phase 2）", () => {
  it("LLM sources の fixture は生成型（schema）に合う", () => {
    expect(validateFixture(llmSourcesFixture, schema.$defs.LlmSourcesView)).toEqual([]);
  });

  it("欠測の費用は 0 円に見せず「不明」、請求と機会費用は別の行で出す", () => {
    expect(usdLabel(null)).toBe("不明");
    expect(usdLabel(0)).toBe("$0");
    const rows = costRows({ cash: null, shadow: 0.5, resource: undefined, effective: null });
    expect(rows.map((row) => row.label)).toEqual([
      "請求（実料金）",
      "枠の機会費用（subscription の残量）",
      "自前計算の機会費用（GPU・待ち）",
      "実効（請求＋機会費用）",
    ]);
    expect(rows.map((row) => row.value)).toEqual(["不明", "$0.5", "不明", "不明"]);
  });

  it("鮮度: 観測が無いものは不明、期限切れは古いと書く", () => {
    expect(freshnessLabel({ observed_at: null, age_secs: null, expires_at: null, stale: true })).toBe(
      "不明（観測なし）",
    );
    expect(freshnessLabel({ observed_at: "2026-10-05T00:00:00Z", age_secs: 9000, expires_at: null, stale: true })).toBe(
      "古い（期限切れ、9000 秒前。再観測が要ります）",
    );
  });

  it("source 状態の行は残量・reset・欠測の欄を言う", () => {
    const lines = sourceStateLines(llmSourcesFixture.sources[0].deployments?.[0] ?? fail());
    expect(lines).toContain("残量 0.4、reset 2026-10-05T05:00:00Z");
    expect(lines).toContain("圧力 0.2");
    expect(lines).toContain("不明な欄: shadow_usd");
  });

  it("deployment の状態は費用を請求と機会費用に分けて描く", () => {
    const out = renderToStaticMarkup(<DeploymentStateList states={llmSourcesFixture.sources[1].deployments ?? []} />);
    expect(out).toContain('aria-label="deployment state openai-compatible:qwen/qwen3-coder"');
    expect(out).toContain("観測 不明（観測なし）");
    expect(out).toContain("請求（実料金）");
    expect(out).toContain("自前計算の機会費用（GPU・待ち）");
    expect(out).toContain("$0.01");
  });
});
