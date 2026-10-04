import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LlmSourcesView, ProviderView } from "../../api/generated/types";
import {
  celerisModelsFor,
  providerLlmSourceDisplay,
  sourceTierScopeNote,
  tiersResolvingTo,
} from "./providers-llm-source";
import { LlmSourceList, ProviderSummary, ProvidersSections } from "./providers-screen";

const html = (el: ReactElement) => renderToStaticMarkup(el);
const stats = {
  by_day: [],
  done: 0,
  error: 0,
  input_tokens: 0,
  lease_expired: 0,
  output_tokens: 0,
  question: 0,
  requeue: 0,
  runs: 0,
};
const provider = (over: Partial<ProviderView>): ProviderView => ({
  id: "p",
  adapter: "codex",
  concurrency: 1,
  env_keys: [],
  tiers: ["standard"],
  stats,
  ...over,
});

const sources: LlmSourcesView = {
  sources: [
    {
      id: "claude-oauth",
      kind: "claude-oauth",
      enabled: true,
      reachable: true,
      accounts: [],
      last_hour_requests: 0,
      last_hour_prompt_tokens: 0,
      last_hour_completion_tokens: 0,
    },
    {
      id: "openai-compatible:qwen",
      kind: "openai-compatible",
      enabled: true,
      reachable: true,
      accounts: [],
      last_hour_requests: 0,
      last_hour_prompt_tokens: 0,
      last_hour_completion_tokens: 0,
    },
  ],
  celeris_tiers: [
    { tier: "frontier", resolves_to: "claude-oauth" },
    { tier: "standard", resolves_to: "claude-oauth" },
    { tier: "cheap", resolves_to: "openai-compatible:qwen" },
  ],
};

describe("providers 画面: LLM source と adapter/harness の区別（ADR-0132 D6）", () => {
  it("実行枠と LLM source を別の見出しで出す", () => {
    const out = html(
      <ProvidersSections
        count={1}
        adapters={
          <ProviderSummary
            item={provider({ adapter: "paperqa", llm_source: { source: "celeris", origin: "derived" } })}
          />
        }
        sources={<LlmSourceList data={sources} />}
      />,
    );
    expect(out).toContain("<h2");
    expect(out).toContain("adapter / harness の実行枠（1）");
    expect(out).toContain(">LLM source</h2>");
    expect(out.indexOf("adapter / harness の実行枠")).toBeLessThan(out.indexOf(">LLM source</h2>"));
    expect(out).toContain("paperqa（文献調査の道具）");
    expect(out).toContain("celeris/cheap にだけ使われ");
  });

  it("celeris を使う道具は celeris/<tier> を示し、固定の Qwen と書かない", () => {
    const item = provider({
      id: "paperqa-main",
      adapter: "paperqa",
      tiers: ["frontier", "standard", "cheap"],
      llm_source: { source: "celeris", origin: "derived" },
    });
    const out = html(<ProviderSummary item={item} />);
    expect(out).toContain("LLM source: celeris/&lt;tier&gt;（proxy）（旧設定から推定）");
    expect(out).toContain("使うモデル celeris/frontier, celeris/standard, celeris/cheap");
    expect(out).toContain("Qwen を使うのは cheap だけ");
    expect(out).not.toMatch(/Qwen 専用|常に Qwen/);
    expect(celerisModelsFor({ ...item, model: "celeris/cheap" })).toEqual(["celeris/cheap"]);
  });

  it("claude-code・codex は OAuth の LLM source、llm_source が無ければ不明", () => {
    expect(
      html(
        <ProviderSummary
          item={provider({ adapter: "claude-code", llm_source: { source: "claude_oauth", origin: "explicit" } })}
        />,
      ),
    ).toContain("LLM source: Claude OAuth（設定で明示）");
    expect(providerLlmSourceDisplay({ source: "codex_oauth", origin: "derived" }).label).toBe("Codex OAuth");
    expect(providerLlmSourceDisplay({ source: "openai_compatible:qwen", origin: "explicit" }).label).toBe(
      "OpenAI 互換: qwen",
    );
    expect(providerLlmSourceDisplay(undefined).label).toBe("不明");
    expect(celerisModelsFor(provider({ llm_source: { source: "codex_oauth", origin: "derived" } }))).toEqual([]);
  });

  it("LLM source 節は種類と解決先を示し、Qwen は cheap だけと書く", () => {
    const out = html(<LlmSourceList data={sources} />);
    expect(out).toContain("claude-oauth</span>（Claude OAuth）");
    expect(out).toContain("解決先: celeris/frontier, celeris/standard");
    expect(out).toContain("解決先: celeris/cheap");
    expect(tiersResolvingTo("openai-compatible:qwen", sources.celeris_tiers)).toEqual(["celeris/cheap"]);
    expect(sourceTierScopeNote("claude-oauth")).toBeNull();
    expect(sourceTierScopeNote("openai-compatible")).toContain("celeris/cheap にだけ");
  });
});
