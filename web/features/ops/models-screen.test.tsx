import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  groupBySource,
  type LlmModel,
  type LlmModelCatalog,
  ModelsView,
  modelSourceLabel,
  modelState,
} from "./models-screen";

const model = (over: Partial<LlmModel>): LlmModel => ({
  source: "opencode-go",
  model_id: "glm-5",
  display_name: "GLM 5",
  available: true,
  first_seen: "2026-10-01T00:00:00Z",
  last_seen: "2026-10-06T00:00:00Z",
  capabilities: {},
  override: null,
  routing: { tiers: [], deployments: [] },
  ...over,
});

const catalog: LlmModelCatalog = {
  items: [
    model({ model_id: "glm-5", routing: { tiers: ["standard"], deployments: ["opencode-go/glm-5"] } }),
    model({ model_id: "old-model", available: false, display_name: null }),
    model({ source: "claude-oauth", model_id: "claude-opus" }),
    model({
      source: "openai-compatible:qwen",
      model_id: "qwen3",
      override: { disabled: false, tier: "cheap", alias: null, note: null },
    }),
  ],
  last_discovery: [
    { source: "opencode-go", at: "2026-10-06T00:00:00Z", ok: true, error: null, count: 2 },
    { source: "claude-oauth", at: "2026-10-06T00:00:00Z", ok: false, error: "401 unauthorized", count: 0 },
  ],
};

const sender = { run: async () => [], pending: false, results: {} };

describe("modelSourceLabel / modelState / groupBySource", () => {
  it("source id を表示名にし、未知はそのまま出す", () => {
    expect(modelSourceLabel("claude-oauth")).toBe("Claude（subscription）");
    expect(modelSourceLabel("codex-oauth")).toBe("Codex（subscription）");
    expect(modelSourceLabel("opencode-go")).toBe("OpenCode Go（subscription）");
    expect(modelSourceLabel("openai-compatible:qwen")).toBe("self-host qwen");
    expect(modelSourceLabel("other")).toBe("other");
  });

  it("状態: 無効化 > 消失 > 利用可", () => {
    expect(modelState(model({})).label).toBe("利用可");
    expect(modelState(model({ available: false })).label).toBe("消失");
    expect(
      modelState(model({ available: false, override: { disabled: true, tier: null, alias: null, note: null } })).label,
    ).toBe("無効化");
  });

  it("source ごとにまとめ、発見の記録だけの source も残す", () => {
    const groups = groupBySource({
      ...catalog,
      last_discovery: [
        ...catalog.last_discovery,
        { source: "codex-oauth", at: "2026-10-06T00:00:00Z", ok: true, error: null, count: 0 },
      ],
    });
    expect(groups.map((g) => g.source)).toEqual([
      "opencode-go",
      "claude-oauth",
      "openai-compatible:qwen",
      "codex-oauth",
    ]);
    expect(groups[0]?.items).toHaveLength(2);
    expect(groups[3]?.items).toHaveLength(0);
  });
});

describe("ModelsView", () => {
  const out = renderToStaticMarkup(<ModelsView data={catalog} sender={sender} blocked={false} />);

  it("source ごとの節と発見の結果・件数・失敗理由を出す", () => {
    expect(out.match(/data-source="/g)).toHaveLength(3);
    expect(out).toContain("OpenCode Go（subscription）（2）");
    expect(out).toContain("self-host qwen");
    expect(out).toContain("成功 / 2 件");
    expect(out).toContain("失敗 / 0 件（401 unauthorized）");
    expect(out.match(/発見を実行/g)?.length).toBeGreaterThanOrEqual(3);
  });

  it("消失した行は語で示し、固定 tier と routing を出す", () => {
    expect(out).toContain('data-model-state="消失"');
    expect(out).toContain("text-muted-foreground");
    expect(out).toContain("固定 cheap");
    expect(out).toContain("tier: standard");
    expect(out).toContain("deployment: opencode-go/glm-5");
    expect(out).toContain("最終確認");
  });

  it("権限が無いときは発見と上書きの操作を止める", () => {
    const blocked = renderToStaticMarkup(<ModelsView data={catalog} sender={sender} blocked />);
    expect(blocked).toContain('disabled=""');
    expect(blocked.match(/disabled=""/g)).toHaveLength(3);
  });

  it("model が無ければ空の案内", () => {
    const empty = renderToStaticMarkup(
      <ModelsView data={{ items: [], last_discovery: [] }} sender={sender} blocked={false} />,
    );
    expect(empty).toContain("モデルがありません");
  });
});
