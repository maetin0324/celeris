import type { LlmCelerisTierView, ProviderView, ResolvedLlmSource } from "../../api/generated/types";

/**
 * providers 画面の表示用（ADR-0132 D6）。`Providers.items[]` は adapter / harness の実行枠で、
 * モデルの供給元（LLM source）は `GET /api/llm/sources` を正本に別の節で示す。
 * celeris の選択規則は画面で再計算しない。返された値を言葉にするだけ。
 */

/** adapter / harness の一語（未知の adapter はそのまま）。 */
export function adapterLabel(adapter: string): string {
  const known: Record<string, string> = {
    "claude-code": "claude-code（Claude Code harness）",
    codex: "codex（Codex harness）",
    acp: "acp（opencode などの ACP harness）",
    paperqa: "paperqa（文献調査の道具）",
    langmem: "langmem（知識整理の道具）",
    "local-deep-research": "ldr（Web 調査の道具）",
    fake: "fake（試験用）",
  };
  return known[adapter] ?? adapter;
}

export interface ProviderLlmSourceDisplay {
  label: string;
  note: string | null;
  origin: string;
}

const ORIGIN_LABEL: Record<ResolvedLlmSource["origin"], string> = {
  explicit: "設定で明示",
  derived: "旧設定から推定",
};

/**
 * 実行枠の `llm_source` 参照（`celeris` / `claude_oauth` / `codex_oauth` / `openai_compatible:<id>` /
 * `none` / `unknown`）を表示用にする。`celeris` は実行時に proxy が選ぶ抽象モデルで、固定の Qwen ではない。
 */
export function providerLlmSourceDisplay(ref: ResolvedLlmSource | null | undefined): ProviderLlmSourceDisplay {
  if (!ref) return { label: "不明", note: "この celeris は llm_source を返していません", origin: "" };
  const origin = ORIGIN_LABEL[ref.origin] ?? "";
  const source = ref.source;
  if (source === "celeris")
    return {
      label: "celeris/<tier>（proxy）",
      note: "実行時に proxy が tier ごとに供給元を選びます（Qwen を使うのは cheap だけ）",
      origin,
    };
  if (source === "claude_oauth") return { label: "Claude OAuth", note: null, origin };
  if (source === "codex_oauth") return { label: "Codex OAuth", note: null, origin };
  if (source === "none") return { label: "なし", note: "LLM を使わない実行枠です", origin };
  if (source.startsWith("openai_compatible:")) {
    const id = source.slice("openai_compatible:".length);
    return { label: `OpenAI 互換: ${id}`, note: "この供給元に直結します（Qwen 直結は cheap だけ）", origin };
  }
  return { label: "不明", note: "設定から供給元を推定できません", origin };
}

/**
 * 実行枠が使う抽象モデル。`llm_source` が `celeris` のときだけ出す。model が `celeris/<tier>` に
 * 固定されていればそれを、無ければ受ける tier ごとの `celeris/<tier>` を返す。
 */
export function celerisModelsFor(item: Pick<ProviderView, "llm_source" | "model" | "tiers">): string[] {
  if (item.llm_source?.source !== "celeris") return [];
  if (item.model?.startsWith("celeris/")) return [item.model];
  return item.tiers.map((tier) => `celeris/${tier}`);
}

/** `/llm/sources` の `kind` を一語にする（未知の値はそのまま）。 */
export function sourceKindLabel(kind: string): string {
  const known: Record<string, string> = {
    "claude-oauth": "Claude OAuth",
    "codex-oauth": "Codex OAuth",
    "openai-compatible": "OpenAI 互換",
  };
  return known[kind] ?? kind;
}

/** OpenAI 互換源（Qwen 等）に添える注記。oauth のプールには付けない。 */
export function sourceTierScopeNote(kind: string): string | null {
  return kind === "openai-compatible" ? "celeris/cheap にだけ使われます（frontier / standard には使いません）" : null;
}

/** 供給元が今どの `celeris/<tier>` の解決先か（`celeris_tiers[].resolves_to` を引き当てるだけ）。 */
export function tiersResolvingTo(sourceId: string, tiers: readonly LlmCelerisTierView[] | null | undefined): string[] {
  return (tiers ?? []).filter((t) => t.resolves_to === sourceId).map((t) => `celeris/${t.tier}`);
}
