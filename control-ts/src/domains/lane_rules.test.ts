import { describe, expect, it } from "vitest";
import "../main";
import { AXES, type Level, type TaskFeatures, laneRules, matchRule } from "./lane_rules";

const ctx = { token: "01M6E6HEH7CPZX9KX4Y2SAZKTG" };

// 規則の判定に効かない軸は medium に置き、どの規則にも当たらない基準の組を作る。
const base: TaskFeatures = {
  judgment: "medium",
  ambiguity: "medium",
  verifiability: "medium",
  reversibility: "medium",
  consequence: "medium",
  context_size: "medium",
  tool_intensity: "medium",
  expected_length: "medium",
  cross_cutting: "medium",
};
const f = (over: Partial<Record<keyof TaskFeatures, Level>>): TaskFeatures => ({ ...base, ...over });
const ruleOf = (features: TaskFeatures) => matchRule(features).rule_id;

describe("lane_rules boundaries", () => {
  it("frontier/judgment-under-uncertainty: exact and one axis off", () => {
    expect(ruleOf(f({ judgment: "high", ambiguity: "high" }))).toBe("frontier/judgment-under-uncertainty");
    expect(ruleOf(f({ judgment: "high", verifiability: "low" }))).toBe("frontier/judgment-under-uncertainty");
    expect(ruleOf(f({ judgment: "medium", ambiguity: "high" }))).toBe("standard/default");
    expect(ruleOf(f({ judgment: "high", ambiguity: "medium" }))).toBe("standard/default");
  });

  it("frontier/costly-and-unverifiable: exact and one axis off", () => {
    expect(ruleOf(f({ consequence: "high", verifiability: "low" }))).toBe("frontier/costly-and-unverifiable");
    expect(ruleOf(f({ consequence: "medium", verifiability: "low" }))).toBe("standard/default");
    expect(ruleOf(f({ consequence: "high", verifiability: "medium" }))).toBe("standard/default");
  });

  it("frontier/broad-judgment: exact and one axis off", () => {
    expect(ruleOf(f({ judgment: "high", cross_cutting: "high" }))).toBe("frontier/broad-judgment");
    expect(ruleOf(f({ judgment: "medium", cross_cutting: "high" }))).toBe("standard/default");
    expect(ruleOf(f({ judgment: "high", cross_cutting: "medium" }))).toBe("standard/default");
  });

  it("cheap/mechanical-verifiable-reversible: exact and each axis off", () => {
    const cheap = { judgment: "low", ambiguity: "low", verifiability: "high", reversibility: "high" } as const;
    expect(matchRule(f({ ...cheap, consequence: "medium" }))).toEqual({
      rule_id: "cheap/mechanical-verifiable-reversible",
      proposed: "cheap",
      condition:
        "judgment=low and ambiguity=low and verifiability=high and reversibility=high and consequence!=high",
    });
    expect(ruleOf(f({ ...cheap, consequence: "low" }))).toBe("cheap/mechanical-verifiable-reversible");
    expect(ruleOf(f({ ...cheap, consequence: "high" }))).toBe("standard/default");
    for (const axis of ["judgment", "ambiguity", "verifiability", "reversibility"] as const) {
      expect(ruleOf(f({ ...cheap, [axis]: "medium" }))).toBe("standard/default");
    }
  });

  it("earlier rules win (order matches Rust RULES)", () => {
    // 3 つの frontier 規則を全部満たす組は最初の規則になる。
    const all = f({ judgment: "high", ambiguity: "high", verifiability: "low", consequence: "high", cross_cutting: "high" });
    expect(ruleOf(all)).toBe("frontier/judgment-under-uncertainty");
    // costly と broad の両方なら costly。
    expect(ruleOf(f({ judgment: "high", consequence: "high", verifiability: "low" }))).toBe(
      "frontier/judgment-under-uncertainty",
    );
    expect(ruleOf(f({ judgment: "high", cross_cutting: "high", consequence: "high", verifiability: "low" }))).toBe(
      "frontier/judgment-under-uncertainty",
    );
  });

  it("default rule output", () => {
    expect(matchRule(base)).toEqual({
      rule_id: "standard/default",
      proposed: "standard",
      condition: "no frontier/cheap rule matched",
    });
  });
});

describe("lane_rules input errors", () => {
  const call = (input: unknown) => globalThis.__celerisDispatch("lane_rules", input, ctx);

  it("rejects an unknown level", async () => {
    await expect(call({ features: { ...base, judgment: "very_high" } })).rejects.toThrow(
      'lane_rules: unknown level for judgment: "very_high"',
    );
    await expect(call({ features: { ...base, ambiguity: "High" } })).rejects.toThrow("unknown level for ambiguity");
    await expect(call({ features: { ...base, consequence: null } })).rejects.toThrow("unknown level for consequence");
  });

  it("rejects a missing axis", async () => {
    for (const axis of AXES) {
      const { [axis]: _drop, ...rest } = base;
      await expect(call({ features: rest })).rejects.toThrow(`lane_rules: missing field in features: ${axis}`);
    }
    await expect(call({})).rejects.toThrow("missing or non-object field: features");
    await expect(call(null)).rejects.toThrow("input must be an object");
  });

  it("rejects an extra field", async () => {
    await expect(call({ features: { ...base, lane: "frontier" } })).rejects.toThrow(
      "lane_rules: unknown field in features: lane",
    );
    await expect(call({ features: base, tier: "cheap" })).rejects.toThrow("lane_rules: unknown field in input: tier");
  });

  it("answers through the dispatcher with the three fields only", async () => {
    const out = await call({ features: base });
    expect(Object.keys(out as object)).toEqual(["rule_id", "proposed", "condition"]);
    expect(await laneRules({ features: base })).toEqual(out);
  });
});
