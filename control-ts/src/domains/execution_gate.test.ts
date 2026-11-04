import { describe, expect, it } from "vitest";
import { decide, executionGate, type ExecutionGateInput, fixed2, parseInput } from "./execution_gate";

function base(): ExecutionGateInput {
  return {
    features: {
      judgment: "low",
      ambiguity: "low",
      verifiability: "low",
      reversibility: "low",
      consequence: "low",
      context_size: "low",
      tool_intensity: "low",
      expected_length: "low",
      cross_cutting: "low",
    },
    title: "t",
    objective: "o",
    acceptance_total: 1,
    artifact_exists: 0,
    has_human: false,
    has_command: true,
    multi_environment: false,
    recent_budget_exhausted_ratio: null,
    cos_hint_compound: false,
    max_turns: 60,
    tree_child: false,
    threshold: 5,
  };
}

// fixture の全件の比較は scripts/compare.test.mjs にある。
describe("execution_gate", () => {
  it("rounds an exact midpoint to even like Rust's {:.2}", () => {
    expect([0.375, 0.625, 0.875, 0.5, 0.305, 0.995, 1].map(fixed2)).toEqual([
      "0.38",
      "0.62",
      "0.88",
      "0.50",
      (0.305).toFixed(2),
      (0.995).toFixed(2),
      "1.00",
    ]);
  });

  it("applies the forced rules before the score", () => {
    const long = base();
    long.features.expected_length = "high";
    long.features.cross_cutting = "high";
    long.max_turns = 1;
    long.threshold = 100;
    expect(decide(long)).toMatchObject({ mode: "compound", rule_id: "compound/long-and-broad", score: 4 });
    const small = base();
    small.max_turns = 10;
    small.threshold = 0;
    expect(decide(small)).toMatchObject({ mode: "atomic", rule_id: "atomic/small" });
    small.tree_child = true;
    expect(decide(small)).toMatchObject({ mode: "compound", rule_id: "compound/score" });
  });

  it("uses the threshold it is given and never names a rule that only Rust may write", () => {
    const input = base();
    input.features.context_size = "high";
    input.features.cross_cutting = "high";
    input.threshold = 4;
    expect(decide(input)).toMatchObject({ mode: "compound", score: 4, rule_id: "compound/score" });
    input.threshold = 5;
    expect(decide(input)).toMatchObject({ mode: "atomic", score: 4, rule_id: "atomic/score" });
    expect(["human/explicit", "atomic/out-of-scope", "atomic/knowledge-curation"]).not.toContain(decide(input).rule_id);
  });

  it("counts code points like Rust's chars().count()", () => {
    const input = base();
    input.objective = "🚀".repeat(2001);
    expect(decide(input).signals.at(-1)).toEqual({ name: "S5", weight: 1, detail: "objective length 2001 > 2000" });
    input.objective = "🚀".repeat(399);
    input.max_turns = 10;
    expect(decide(input).rule_id).toBe("atomic/small");
  });

  it.each([
    ["missing field", (v: Record<string, unknown>) => delete v.threshold],
    ["extra field", (v: Record<string, unknown>) => (v.extra = 1)],
    ["wrong type", (v: Record<string, unknown>) => (v.max_turns = "60")],
    ["fractional count", (v: Record<string, unknown>) => (v.acceptance_total = 1.5)],
    ["negative count", (v: Record<string, unknown>) => (v.artifact_exists = -1)],
    ["ratio out of range", (v: Record<string, unknown>) => (v.recent_budget_exhausted_ratio = 1.5)],
    ["unknown level", (v: Record<string, unknown>) => (v.features = { ...(v.features as object), judgment: "extreme" })],
  ])("rejects a broken input: %s", async (_name, edit) => {
    const input = base() as unknown as Record<string, unknown>;
    edit(input);
    await expect(executionGate(input)).rejects.toThrow("execution_gate");
  });

  it("does not put the task text in an error message", () => {
    const input = base() as unknown as Record<string, unknown>;
    input.title = 1;
    input.objective = "SECRET-OBJECTIVE";
    expect(() => parseInput(input)).toThrow(/^(?!.*SECRET-OBJECTIVE)/);
    const extra = { ...base(), "SECRET-KEY": 1 };
    expect(() => parseInput(extra)).toThrow(/^(?!.*SECRET-KEY)/);
  });
});
