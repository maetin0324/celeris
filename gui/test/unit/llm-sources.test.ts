import { describe, expect, it } from "vitest";
import type { LlmSourceView } from "~/celeris/types";
import {
  allAccountsCoolingDown,
  CLAUDE_ALL_COOLDOWN_FALLBACK_NOTE,
  cooldownRemainingLabel,
  cooldownUntilTitle,
  formatRemaining,
  forwardStatusWord,
  isAccountCoolingDown,
  llmSourceOriginLabel,
  providerLlmSourceDisplay,
  sourceKindLabel,
  sourceLabel,
  sourceStatusWord,
  sourceTierScopeNote,
  tierLabel,
  tierResolutionLabel,
  tierResolutionReason,
  tierResolutionReasonLabel,
  tiersResolvingTo,
} from "~/lib/llm-sources";

describe("sourceLabel", () => {
  it("maps the two oauth pools to human labels", () => {
    expect(sourceLabel("claude-oauth")).toBe("Claude");
    expect(sourceLabel("codex-oauth")).toBe("Codex");
  });

  it("strips the openai-compatible: prefix", () => {
    expect(sourceLabel("openai-compatible:qwen")).toBe("qwen");
  });

  it("returns unknown ids unchanged", () => {
    expect(sourceLabel("something-else")).toBe("something-else");
    expect(sourceLabel("openai-compatible:")).toBe("openai-compatible:");
  });
});

describe("sourceStatusWord", () => {
  it("is disabled when the source itself is disabled, regardless of reachable", () => {
    expect(sourceStatusWord({ enabled: false, reachable: true })).toBe("disabled");
  });

  it("reflects the probe result for openai-compatible sources", () => {
    expect(sourceStatusWord({ enabled: true, reachable: true })).toBe("reachable");
    expect(sourceStatusWord({ enabled: true, reachable: false })).toBe("unreachable");
  });

  it("falls back to enabled for oauth pools (no reachable field)", () => {
    expect(sourceStatusWord({ enabled: true, reachable: undefined })).toBe("enabled");
    expect(sourceStatusWord({ enabled: true, reachable: null })).toBe("enabled");
  });

  it("never produces a badge with whitespace or more than 12 characters", () => {
    for (const input of [
      { enabled: false, reachable: true },
      { enabled: true, reachable: true },
      { enabled: true, reachable: false },
      { enabled: true, reachable: undefined },
    ]) {
      const word = sourceStatusWord(input);
      expect(word).not.toMatch(/\s/);
      expect(word.length).toBeLessThanOrEqual(12);
    }
  });
});

describe("formatRemaining", () => {
  it("renders a percentage rounded to the nearest integer", () => {
    expect(formatRemaining(0.62)).toBe("62%");
    expect(formatRemaining(1)).toBe("100%");
    expect(formatRemaining(0)).toBe("0%");
  });

  it("does not fabricate a value when it cannot be measured", () => {
    expect(formatRemaining(null)).toBe("不明");
    expect(formatRemaining(undefined)).toBe("不明");
    expect(formatRemaining(Number.NaN)).toBe("不明");
  });

  it("clamps out-of-range values instead of showing nonsense percentages", () => {
    expect(formatRemaining(1.5)).toBe("100%");
    expect(formatRemaining(-0.5)).toBe("0%");
  });
});

describe("tierResolutionLabel", () => {
  it("labels a resolved source", () => {
    expect(tierResolutionLabel("openai-compatible:qwen")).toBe("qwen");
    expect(tierResolutionLabel("claude-oauth")).toBe("Claude");
  });

  it("says there is no source when nothing resolved", () => {
    expect(tierResolutionLabel(null)).toBe("供給元なし");
    expect(tierResolutionLabel(undefined)).toBe("供給元なし");
  });
});

describe("tierLabel", () => {
  it("passes known tiers through and leaves unknown ones unchanged", () => {
    expect(tierLabel("frontier")).toBe("frontier");
    expect(tierLabel("cheap")).toBe("cheap");
    expect(tierLabel("mystery")).toBe("mystery");
  });
});

describe("cooldownRemainingLabel", () => {
  it("formats the remaining time when still cooling down", () => {
    expect(cooldownRemainingLabel(1_000, 940)).toBe("1分");
  });

  it("says it has expired once past the deadline", () => {
    expect(cooldownRemainingLabel(1_000, 1_000)).toBe("切れています");
    expect(cooldownRemainingLabel(1_000, 1_500)).toBe("切れています");
  });
});

describe("isAccountCoolingDown", () => {
  it("is true only while cooldown_until is in the future", () => {
    expect(isAccountCoolingDown({ cooldown_until: 1_000 }, 500)).toBe(true);
    expect(isAccountCoolingDown({ cooldown_until: 1_000 }, 1_000)).toBe(false);
    expect(isAccountCoolingDown({ cooldown_until: 1_000 }, 1_500)).toBe(false);
  });

  it("is false when there is no cooldown", () => {
    expect(isAccountCoolingDown({ cooldown_until: null }, 500)).toBe(false);
    expect(isAccountCoolingDown({ cooldown_until: undefined }, 500)).toBe(false);
  });
});

describe("forwardStatusWord", () => {
  it("reflects the observed reachability", () => {
    expect(forwardStatusWord({ up: true })).toBe("up");
    expect(forwardStatusWord({ up: false })).toBe("down");
  });

  it("does not fabricate a value when there is no observation yet", () => {
    expect(forwardStatusWord({ up: null })).toBe("unknown");
    expect(forwardStatusWord({ up: undefined })).toBe("unknown");
    expect(forwardStatusWord({})).toBe("unknown");
  });

  // ADR-0053 Phase 85: 転送（listener）はあるのに先方（target）が応答しないのは、転送そのものが
  // 無い（down）とは別の状態。celeris はこの状態では転送を再発行しない。
  it("distinguishes a present listener with an unhealthy target from a missing forward", () => {
    expect(forwardStatusWord({ up: false, listener: true, target_healthy: false })).toBe("unreachable");
    expect(forwardStatusWord({ up: true, listener: true, target_healthy: true })).toBe("up");
    expect(forwardStatusWord({ up: false, listener: false, target_healthy: false })).toBe("down");
  });

  it("never produces a badge with whitespace or more than 12 characters", () => {
    const cases: Array<{ up?: boolean | null; listener?: boolean | null; target_healthy?: boolean | null }> = [
      { up: true },
      { up: false },
      { up: null },
      { up: undefined },
      { up: false, listener: true, target_healthy: false },
    ];
    for (const forward of cases) {
      const word = forwardStatusWord(forward);
      expect(word).not.toMatch(/\s/);
      expect(word.length).toBeLessThanOrEqual(12);
    }
  });
});

/** `/accounts` の「LLM source」節（ADR-0055 ラウンド 11、Phase 86）: `celeris/<tier>` の解決理由。 */
describe("tierResolutionReason", () => {
  type Src = Pick<LlmSourceView, "id" | "kind" | "enabled" | "reachable" | "accounts">;

  const qwenReachable: Src = {
    id: "openai-compatible:qwen",
    kind: "openai-compatible",
    enabled: true,
    reachable: true,
    accounts: [],
  };
  const qwenUnreachable: Src = {
    id: "openai-compatible:qwen",
    kind: "openai-compatible",
    enabled: true,
    reachable: false,
    accounts: [],
  };
  const claudeNoCooldown: Src = {
    id: "claude-oauth",
    kind: "claude-oauth",
    enabled: true,
    accounts: [{ id: "a", logged_in: true, cooldown_until: null }],
  };
  const claudeCooling: Src = {
    id: "claude-oauth",
    kind: "claude-oauth",
    enabled: true,
    accounts: [{ id: "a", logged_in: true, cooldown_until: 2_000 }],
  };

  it("says there is no candidate when nothing resolved", () => {
    expect(tierResolutionReason("cheap", null, [], 1_000)).toBe("no-source");
    expect(tierResolutionReason("cheap", undefined, [claudeNoCooldown], 1_000)).toBe("no-source");
  });

  it("is free-first when the resolved source is the free relay (ADR-0053 D1(a))", () => {
    expect(tierResolutionReason("cheap", "openai-compatible:qwen", [qwenReachable, claudeNoCooldown], 1_000)).toBe(
      "free-first",
    );
  });

  it("is unreachable when an oauth pool won only because the free relay is down", () => {
    expect(tierResolutionReason("cheap", "claude-oauth", [qwenUnreachable, claudeNoCooldown], 1_000)).toBe(
      "unreachable",
    );
  });

  it("is cooldown when the pool has a cooling account and no unreachable free relay explains it", () => {
    expect(tierResolutionReason("cheap", "claude-oauth", [claudeCooling], 1_000)).toBe("cooldown");
  });

  it("is unknown for a plain oauth resolution (no free relay, no cooldown)", () => {
    expect(tierResolutionReason("cheap", "claude-oauth", [claudeNoCooldown], 1_000)).toBe("unknown");
  });

  it("is unknown when the resolved id is not among the known sources", () => {
    expect(tierResolutionReason("cheap", "codex-oauth", [claudeNoCooldown], 1_000)).toBe("unknown");
  });
});

/** ADR-0132 D3: Qwen を優先・倒れ先の理由にするのは cheap だけ。frontier / standard は Qwen を候補にしない。 */
describe("tierResolutionReason (cheap-only Qwen, ADR-0132)", () => {
  type Src = Pick<LlmSourceView, "id" | "kind" | "enabled" | "reachable" | "accounts">;
  const qwenDown: Src = {
    id: "openai-compatible:qwen",
    kind: "openai-compatible",
    enabled: true,
    reachable: false,
    accounts: [],
  };
  const qwenUp: Src = { ...qwenDown, reachable: true };
  const claude: Src = { id: "claude-oauth", kind: "claude-oauth", enabled: true, accounts: [] };

  it("never explains frontier / standard by the Qwen relay", () => {
    for (const tier of ["frontier", "standard"]) {
      expect(tierResolutionReason(tier, "claude-oauth", [qwenDown, claude], 1_000)).toBe("unknown");
      expect(tierResolutionReason(tier, "openai-compatible:qwen", [qwenUp, claude], 1_000)).toBe("unknown");
    }
  });

  it("explains cheap by Qwen first, and by the Claude / GPT fallback when Qwen is down", () => {
    expect(tierResolutionReason("cheap", "openai-compatible:qwen", [qwenUp, claude], 1_000)).toBe("free-first");
    expect(tierResolutionReason("cheap", "claude-oauth", [qwenDown, claude], 1_000)).toBe("unreachable");
  });

  it("scopes the Qwen labels to cheap", () => {
    expect(tierResolutionReasonLabel("free-first")).toContain("cheap");
    expect(tierResolutionReasonLabel("unreachable")).toContain("cheap");
    expect(tierResolutionReasonLabel("free-first")).not.toBe("無料の Qwen が使えるため優先しています");
  });

  it("says only celeris/cheap falls back to Qwen when every Claude account cools down", () => {
    expect(CLAUDE_ALL_COOLDOWN_FALLBACK_NOTE).toContain("Qwen を使うのは celeris/cheap だけ");
    expect(CLAUDE_ALL_COOLDOWN_FALLBACK_NOTE).toContain(
      "celeris/frontier と celeris/standard は Codex の GPT に倒れます",
    );
  });
});

describe("providerLlmSourceDisplay (ADR-0132 D6)", () => {
  it("shows celeris as a runtime choice by the proxy, not a fixed Qwen", () => {
    const d = providerLlmSourceDisplay({ source: "celeris", origin: "derived" });
    expect(d.label).toBe("celeris/<tier>（proxy）");
    expect(d.label).not.toContain("Qwen");
    expect(d.note).toContain("実行時に proxy");
    expect(d.note).toContain("cheap だけ");
    expect(d.sourceId).toBeNull();
    expect(llmSourceOriginLabel(d.origin)).toBe("旧設定から推定");
  });

  it("maps the oauth pools and an openai-compatible relay to /llm/sources ids", () => {
    expect(providerLlmSourceDisplay({ source: "claude_oauth", origin: "explicit" })).toMatchObject({
      label: "Claude OAuth",
      sourceId: "claude-oauth",
      origin: "explicit",
    });
    expect(providerLlmSourceDisplay({ source: "codex_oauth", origin: "derived" }).sourceId).toBe("codex-oauth");
    const qwen = providerLlmSourceDisplay({ source: "openai_compatible:qwen", origin: "derived" });
    expect(qwen.label).toBe("OpenAI 互換: qwen");
    expect(qwen.sourceId).toBe("openai-compatible:qwen");
    expect(qwen.note).toContain("cheap だけ");
  });

  it("does not guess Qwen for unknown or missing references", () => {
    expect(providerLlmSourceDisplay({ source: "unknown", origin: "derived" }).label).toBe("不明");
    expect(providerLlmSourceDisplay(null).label).toBe("不明");
    expect(providerLlmSourceDisplay(undefined).origin).toBeNull();
    expect(providerLlmSourceDisplay({ source: "none", origin: "explicit" }).label).toBe("なし");
  });
});

describe("source section helpers (ADR-0132 D6)", () => {
  it("labels kinds and scopes OpenAI-compatible sources to cheap", () => {
    expect(sourceKindLabel("claude-oauth")).toBe("Claude OAuth");
    expect(sourceKindLabel("openai-compatible")).toBe("OpenAI 互換");
    expect(sourceKindLabel("other")).toBe("other");
    expect(sourceTierScopeNote("openai-compatible")).toContain("celeris/cheap にだけ");
    expect(sourceTierScopeNote("claude-oauth")).toBeNull();
  });

  it("lists the tiers currently resolving to a source", () => {
    const tiers = [
      { tier: "frontier", resolves_to: "claude-oauth" },
      { tier: "standard", resolves_to: "claude-oauth" },
      { tier: "cheap", resolves_to: "openai-compatible:qwen" },
    ];
    expect(tiersResolvingTo("claude-oauth", tiers)).toEqual(["frontier", "standard"]);
    expect(tiersResolvingTo("openai-compatible:qwen", tiers)).toEqual(["cheap"]);
    expect(tiersResolvingTo("codex-oauth", undefined)).toEqual([]);
  });
});

describe("tierResolutionReasonLabel", () => {
  it("returns a non-empty one-line label for every reason", () => {
    for (const reason of ["free-first", "cooldown", "unreachable", "no-source", "unknown"] as const) {
      expect(tierResolutionReasonLabel(reason).length).toBeGreaterThan(0);
    }
  });
});

describe("cooldownUntilTitle", () => {
  it("renders the absolute RFC 3339 instant for a Unix-seconds cooldown_until", () => {
    expect(cooldownUntilTitle(0)).toBe("1970-01-01T00:00:00.000Z");
  });
});

describe("allAccountsCoolingDown", () => {
  it("is true only when every account is currently cooling down", () => {
    expect(allAccountsCoolingDown([{ cooldown_until: 2_000 }, { cooldown_until: 3_000 }], 1_000)).toBe(true);
  });

  it("is false when at least one account is not cooling down", () => {
    expect(allAccountsCoolingDown([{ cooldown_until: 2_000 }, { cooldown_until: null }], 1_000)).toBe(false);
  });

  it("is false when there are no accounts at all (nothing to warn about)", () => {
    expect(allAccountsCoolingDown([], 1_000)).toBe(false);
  });
});
