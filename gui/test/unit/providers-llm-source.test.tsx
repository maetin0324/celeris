import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LlmSourcesView, ProviderView } from "~/celeris/types";
import { LlmSourcesOverview, ProviderLlmSourceItem } from "~/routes/providers";

// ADR-0132 D6: providers 画面は adapter / harness の実行枠と LLM source を別に見せ、
// celeris/<tier> を固定の Qwen と誤表示しない。Qwen は cheap にだけ使われると示す。
const baseProvider: ProviderView = {
  id: "paperqa-qwen",
  adapter: "paperqa",
  tiers: ["frontier", "standard", "cheap"],
  concurrency: 1,
  model: null,
  env_keys: [],
  in_use: 0,
  cooldown: null,
  stats: {
    runs: 0,
    done: 0,
    question: 0,
    error: 0,
    requeue: 0,
    lease_expired: 0,
    input_tokens: 0,
    output_tokens: 0,
    by_day: [],
  },
  kind: "adapter",
  llm_source: { source: "celeris", origin: "derived" },
};

const sources: LlmSourcesView = {
  celeris_tiers: [
    { tier: "frontier", resolves_to: "claude-oauth" },
    { tier: "standard", resolves_to: "claude-oauth" },
    { tier: "cheap", resolves_to: "openai-compatible:qwen" },
  ],
  sources: [
    {
      id: "claude-oauth",
      kind: "claude-oauth",
      enabled: true,
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
};

describe("ProviderLlmSourceItem", () => {
  it("shows a legacy -qwen id that uses celeris/<tier> as a proxy choice, not as Qwen", () => {
    const html = renderToStaticMarkup(<ProviderLlmSourceItem item={baseProvider} />);
    expect(html).toContain('data-source-ref="celeris"');
    expect(html).toContain("celeris/&lt;tier&gt;（proxy）");
    expect(html).toContain("旧設定から推定");
    expect(html).toContain("Qwen を使うのは cheap だけ");
  });

  it("shows the oauth source of a claude-code row as explicit", () => {
    const html = renderToStaticMarkup(
      <ProviderLlmSourceItem
        item={{
          ...baseProvider,
          id: "claude-pool",
          adapter: "claude-code",
          llm_source: { source: "claude_oauth", origin: "explicit" },
        }}
      />,
    );
    expect(html).toContain("Claude OAuth");
    expect(html).toContain("設定で明示");
  });

  it("says unknown when an old celeris does not report llm_source", () => {
    const html = renderToStaticMarkup(<ProviderLlmSourceItem item={{ ...baseProvider, llm_source: undefined }} />);
    expect(html).toContain("不明");
    expect(html).not.toContain("Qwen");
  });
});

describe("LlmSourcesOverview", () => {
  it("lists LLM sources separately with kind, id, status and resolved tiers", () => {
    const html = renderToStaticMarkup(<LlmSourcesOverview llmSources={sources} unavailable={false} />);
    expect(html).toContain('data-testid="provider-llm-sources"');
    expect(html.match(/data-testid="provider-llm-source-row"/g)?.length).toBe(2);
    expect(html).toContain('data-source-id="openai-compatible:qwen"');
    expect(html).toContain("OpenAI 互換");
    expect(html).toContain("Claude OAuth");
    expect(html).toContain("celeris/frontier, celeris/standard");
    expect(html).toContain("celeris/cheap にだけ使われます");
    expect(html).toContain("reachable");
  });

  it("does not claim frontier / standard can resolve to Qwen", () => {
    const html = renderToStaticMarkup(<LlmSourcesOverview llmSources={sources} unavailable={false} />);
    const qwenCard = html.slice(html.indexOf('data-source-id="openai-compatible:qwen"'));
    expect(qwenCard).not.toContain("celeris/frontier");
    expect(qwenCard).not.toContain("celeris/standard");
  });

  it("explains a missing [llm_proxy]", () => {
    const html = renderToStaticMarkup(<LlmSourcesOverview llmSources={null} unavailable={true} />);
    expect(html).toContain("[llm_proxy] が設定されていません");
  });
});
