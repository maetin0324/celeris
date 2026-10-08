import type { ReactElement, ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { LlmSourcesView, ProviderView } from "../../api/generated/types";
import { mcpClientState } from "./mcp-clients";
import {
  buildProviderBody,
  checkResultLabel,
  deniedMessage,
  providerState,
  validateProviderForm,
} from "./providers-form";
import {
  celerisModelsFor,
  providerLlmSourceDisplay,
  sourceTierScopeNote,
  tiersResolvingTo,
} from "./providers-llm-source";
import {
  ConcurrencyNotice,
  LlmSourceList,
  ProviderCard,
  ProviderCreateForm,
  ProviderStatusTable,
  ProviderSummary,
  ProvidersSections,
} from "./providers-screen";
import { secretRows, secretUsageText } from "./secrets-section";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
}));

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
    expect(out).toContain("実行枠（1）</h2>");
    expect(out).toContain(">LLM source</h2>");
    expect(out.indexOf("実行枠（1）")).toBeLessThan(out.indexOf(">LLM source</h2>"));
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
    expect(out).toContain(">到達可</span>");
    expect(out).toContain("解決先: celeris/frontier, celeris/standard");
    expect(out).toContain("解決先: celeris/cheap");
    expect(tiersResolvingTo("openai-compatible:qwen", sources.celeris_tiers)).toEqual(["celeris/cheap"]);
    expect(sourceTierScopeNote("claude-oauth")).toBeNull();
    expect(sourceTierScopeNote("openai-compatible")).toContain("celeris/cheap にだけ");
  });
});

describe("providers 画面: 状態を文字で読ませる・form の検査・secret の値を出さない", () => {
  it("状態の写像は失敗→休止→利用制限→確認済み→未確認の順で、必ず文字を持つ", () => {
    expect(providerState({ last_check: { at: "t", result: "auth_failed" } }).label).toBe("認証失敗");
    expect(providerState({ last_check: { at: "t", result: "spawn_failed" } }).tone).toBe("danger");
    expect(
      providerState({ cooldown: { provider: "p", reason: "rate limit", until: "u" }, last_check: null }).label,
    ).toBe("休止中");
    expect(providerState({ last_check: { at: "t", result: "throttled" } }).label).toBe("利用制限中");
    expect(providerState({ last_check: { at: "t", result: "ok" } }).label).toBe("接続確認済み");
    expect(providerState({}).label).toBe("未確認");
    expect(checkResultLabel("ok")).toBe("正常");
  });

  it("状態の表は実行枠ごとに状態の文字の badge を出す", () => {
    const out = html(
      <ProviderStatusTable
        items={[
          provider({ id: "a", last_check: { at: "2026-09-30T00:00:00Z", result: "auth_failed" } }),
          provider({ id: "b" }),
        ]}
      />,
    );
    expect(out).toContain('aria-label="実行枠の状態"');
    expect(out).toContain('data-state="認証失敗"');
    expect(out).toContain(">認証失敗</span>");
    expect(out).toContain(">未確認</span>");
  });

  it("form の検査は項目ごとの文言を返す", () => {
    expect(validateProviderForm({ id: " " }, "create")).toEqual({ id: "id を入力してください。" });
    expect(validateProviderForm({}, "patch")).toEqual({});
    expect(deniedMessage("削除")).toContain("この操作を行う権限がありません");
  });

  it("secret は値を出さず、更新日時と使っている所だけを出す", () => {
    const rows = secretRows({
      id: "OPENAI_KEY",
      fingerprint: "fp-16",
      updated_at: "2026-09-30T00:00:00Z",
      used_by: [{ scope: "provider", name: "codex-main", env: "OPENAI_API_KEY" }],
    });
    const text = JSON.stringify(rows);
    expect(text).toContain("表示しません");
    expect(text).not.toContain("fp-16");
    expect(secretUsageText({ used_by: [] })).toBe("参照している設定はありません");
    expect(text).toContain("provider codex-main（OPENAI_API_KEY）");
  });

  it("MCP client の状態は失効を先に見る", () => {
    expect(mcpClientState({ revoked_at: "2026-09-30T00:00:00Z" }).label).toBe("失効");
    expect(mcpClientState({ revoked_at: null }).label).toBe("有効");
  });
});

describe("providers 画面: 同時実行数はアカウントで管理する（ADR 2026-10-08-account-concurrency-limits）", () => {
  it("送る body に concurrency を入れない", () => {
    const create = buildProviderBody({ id: " q ", adapter: "codex", model: " m ", tiers: ["cheap"] }, "create");
    expect(create).toEqual({ id: "q", adapter: "codex", model: "m", tiers: ["cheap"] });
    const patch = buildProviderBody({ model: "", tiers: [] }, "patch");
    expect(patch).toEqual({ tiers: [] });
    expect(Object.keys(create)).not.toContain("concurrency");
  });

  it("状態の表は in_use / concurrency を出さず、アカウント画面へ導く", () => {
    const out = html(<ProviderStatusTable items={[provider({ id: "a", concurrency: 7, in_use: 3 })]} />);
    expect(out).not.toContain("3 / 7");
    expect(out).not.toContain("上限 7");
    expect(out).toMatch(/data-field="concurrency"[^>]*>.*<a href="\/accounts"[^>]*>アカウントで管理<\/a>/s);
  });

  it("要約に concurrency を出さない", () => {
    const out = html(<ProviderSummary item={provider({ concurrency: 9 })} />);
    expect(out).not.toContain("concurrency");
    expect(out).toContain("設定: codex・段 standard");
  });

  it("実行枠の設定と追加の form に同時実行数の入力欄が無い", () => {
    const sender = { run: async () => [], pending: false, results: {} };
    const card = html(
      <ul>
        <ProviderCard
          item={provider({ id: "a", concurrency: 4 })}
          sender={sender}
          blocked={false}
          deniedId={undefined}
        />
      </ul>,
    );
    const create = html(<ProviderCreateForm sender={sender} blocked={false} deniedId={undefined} />);
    for (const out of [card, create]) {
      expect(out).not.toContain("同時実行数（concurrency）");
      expect(out).not.toMatch(/type="number"/);
      expect(out).not.toContain("concurrency 4");
      expect(out).toContain('href="/accounts"');
    }
    expect(card).toContain("モデル（model）");
    expect(create).toContain("新規 id");
  });

  it("案内はアカウント画面への導線を持つ", () => {
    const out = html(<ConcurrencyNotice />);
    expect(out).toContain('data-testid="concurrency-notice"');
    expect(out).toContain('href="/accounts"');
    expect(out).toContain("暗黙アカウント");
  });
});
