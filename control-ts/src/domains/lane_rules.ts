// lane_rules 領域（境界 ADR 2026-11-02 付記 S2-2）: crates/task-core/src/model_policy.rs の
// `match_rule` と同じ順・同じ文字列で規則表を上から当てる。
// LaneCeiling の clamp・人の明示・System の扱い・reasons は Rust に残す（ここに入れない）。
// 入力の未知の値・欠けた軸・余計な欄は throw する。Rust は TS の結果を使わず参照実装で答える。

export type Level = "low" | "medium" | "high";
export type Tier = "frontier" | "standard" | "cheap";

/** Rust の `TaskFeatures` と同じ 9 軸（宣言順）。 */
export const AXES = [
  "judgment",
  "ambiguity",
  "verifiability",
  "reversibility",
  "consequence",
  "context_size",
  "tool_intensity",
  "expected_length",
  "cross_cutting",
] as const;

export type Axis = (typeof AXES)[number];
export type TaskFeatures = Record<Axis, Level>;

export interface LaneRulesInput {
  features: TaskFeatures;
}

/** 出力の欄の順は Rust の `LaneRulesOutput` の宣言順（rule_id・proposed・condition）。 */
export interface LaneRulesOutput {
  rule_id: string;
  proposed: Tier;
  condition: string;
}

interface LaneRule {
  id: string;
  lane: Tier;
  condition: string;
  when: (f: TaskFeatures) => boolean;
}

// model_policy.rs の RULES と同じ順。最後の standard/default は必ず当たる。
export const RULES: readonly LaneRule[] = [
  {
    id: "frontier/judgment-under-uncertainty",
    lane: "frontier",
    condition: "judgment=high and (ambiguity=high or verifiability=low)",
    when: (f) => f.judgment === "high" && (f.ambiguity === "high" || f.verifiability === "low"),
  },
  {
    id: "frontier/costly-and-unverifiable",
    lane: "frontier",
    condition: "consequence=high and verifiability=low",
    when: (f) => f.consequence === "high" && f.verifiability === "low",
  },
  {
    id: "frontier/broad-judgment",
    lane: "frontier",
    condition: "judgment=high and cross_cutting=high",
    when: (f) => f.judgment === "high" && f.cross_cutting === "high",
  },
  {
    id: "cheap/mechanical-verifiable-reversible",
    lane: "cheap",
    condition:
      "judgment=low and ambiguity=low and verifiability=high and reversibility=high and consequence!=high",
    when: (f) =>
      f.judgment === "low" &&
      f.ambiguity === "low" &&
      f.verifiability === "high" &&
      f.reversibility === "high" &&
      f.consequence !== "high",
  },
  {
    id: "standard/default",
    lane: "standard",
    condition: "no frontier/cheap rule matched",
    when: () => true,
  },
];

const LEVELS: ReadonlySet<string> = new Set(["low", "medium", "high"]);

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 入力を検査して TaskFeatures にする。Rust の deny_unknown_fields と同じく未知・欠損・余計を拒む。 */
export function parseInput(input: unknown): LaneRulesInput {
  if (!isPlainObject(input)) throw new Error("lane_rules: input must be an object");
  for (const key of Object.keys(input)) {
    if (key !== "features") throw new Error(`lane_rules: unknown field in input: ${key}`);
  }
  const raw = input.features;
  if (!isPlainObject(raw)) throw new Error("lane_rules: missing or non-object field: features");
  const allowed: ReadonlySet<string> = new Set(AXES);
  for (const key of Object.keys(raw)) {
    if (!allowed.has(key)) throw new Error(`lane_rules: unknown field in features: ${key}`);
  }
  const features = {} as TaskFeatures;
  for (const axis of AXES) {
    if (!Object.hasOwn(raw, axis)) throw new Error(`lane_rules: missing field in features: ${axis}`);
    const value = raw[axis];
    if (typeof value !== "string" || !LEVELS.has(value)) {
      throw new Error(`lane_rules: unknown level for ${axis}: ${JSON.stringify(value)}`);
    }
    features[axis] = value as Level;
  }
  return { features };
}

/** 規則表を上から当て、最初に当たった規則を返す（Rust の `match_rule`）。 */
export function matchRule(features: TaskFeatures): LaneRulesOutput {
  for (const rule of RULES) {
    if (rule.when(features)) return { rule_id: rule.id, proposed: rule.lane, condition: rule.condition };
  }
  // RULES の最後は必ず当たるので到達しない。Rust の unwrap_or と同じ値を返す。
  return { rule_id: "standard/default", proposed: "standard", condition: "no rule matched" };
}

export async function laneRules(input: unknown): Promise<LaneRulesOutput> {
  return matchRule(parseInput(input).features);
}
