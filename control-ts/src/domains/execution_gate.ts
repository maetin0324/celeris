// execution_gate 領域（ADR 2026-11-04-ts-execution-gate-domain）: crates/task-core/src/execution_gate/scorer.rs の
// `score` と同じ順・同じ文字列で Complexity Gate の規則表と強制規則を当てる。
// 対象外の規則・人の明示・深さの閾値の計算・source・shadow・depth・policy_version の刻印・event の書き込みは
// Rust に残す（ここに入れない）。入力の余計な欄・欠けた欄・型の違いは throw する。
// 例外文に title・objective の本文を入れない（記録の機密除去。ADR D6）。

export type ExecutionMode = "atomic" | "compound";
export type Level = "low" | "medium" | "high";

/** Rust の `TaskFeatures` と同じ 9 軸（宣言順。領域の file は互いに import しない）。 */
const AXES = [
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

export type TaskFeatures = Record<(typeof AXES)[number], Level>;

/** Rust の `ExecutionGateInput`（欄の順も同じ）。 */
export interface ExecutionGateInput {
  features: TaskFeatures;
  title: string;
  objective: string;
  acceptance_total: number;
  artifact_exists: number;
  has_human: boolean;
  has_command: boolean;
  multi_environment: boolean;
  recent_budget_exhausted_ratio: number | null;
  cos_hint_compound: boolean;
  max_turns: number;
  tree_child: boolean;
  threshold: number;
}

/** 出力の signal（欄の順は name・weight・detail）。 */
export interface GateSignal {
  name: string;
  weight: number;
  detail: string;
}

/** 出力の欄の順は Rust の `ExecutionGateOutput` の宣言順（mode・score・rule_id・signals）。 */
export interface ExecutionGateOutput {
  mode: ExecutionMode;
  score: number;
  rule_id: string;
  signals: GateSignal[];
}

// 題名・目的に出てくる工程語の種類（S1）。scorer.rs の PROCESS_WORD_GROUPS と同じ順。
const PROCESS_WORD_GROUPS: readonly (readonly string[])[] = [
  ["調査", "investigate", "survey"],
  ["設計", "design"],
  ["実装", "implement"],
  ["テスト", "test"],
  ["review", "レビュー"],
  ["release", "リリース", "deploy", "配送"],
];

const INPUT_FIELDS = [
  "features",
  "title",
  "objective",
  "acceptance_total",
  "artifact_exists",
  "has_human",
  "has_command",
  "multi_environment",
  "recent_budget_exhausted_ratio",
  "cos_hint_compound",
  "max_turns",
  "tree_child",
  "threshold",
] as const;

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function object(v: unknown, where: string, fields: readonly string[]): Record<string, unknown> {
  if (!isPlainObject(v)) throw new Error(`execution_gate: ${where} must be an object`);
  for (const key of Object.keys(v)) {
    if (!fields.includes(key)) throw new Error(`execution_gate: unknown field in ${where}`);
  }
  for (const key of fields) {
    if (!Object.hasOwn(v, key)) throw new Error(`execution_gate: missing field in ${where}: ${key}`);
  }
  return v;
}

function string(v: unknown, where: string): string {
  if (typeof v !== "string") throw new Error(`execution_gate: ${where} must be a string`);
  return v;
}

function boolean(v: unknown, where: string): boolean {
  if (typeof v !== "boolean") throw new Error(`execution_gate: ${where} must be a boolean`);
  return v;
}

function integer(v: unknown, where: string): number {
  if (typeof v !== "number" || !Number.isSafeInteger(v)) {
    throw new Error(`execution_gate: ${where} must be an integer`);
  }
  return v;
}

function count(v: unknown, where: string): number {
  const n = integer(v, where);
  if (n < 0) throw new Error(`execution_gate: ${where} must not be negative`);
  return n;
}

function level(v: unknown, where: string): Level {
  if (v !== "low" && v !== "medium" && v !== "high") {
    throw new Error(`execution_gate: ${where} must be a level`);
  }
  return v;
}

/** 入力を検査して ExecutionGateInput にする。未知の欄・欠けた欄を拒む（Option の欄も null を明示させる）。 */
export function parseInput(input: unknown): ExecutionGateInput {
  const o = object(input, "input", INPUT_FIELDS);
  const f = object(o.features, "features", AXES);
  const features = Object.fromEntries(AXES.map((a) => [a, level(f[a], `features.${a}`)])) as TaskFeatures;
  const ratio = o.recent_budget_exhausted_ratio;
  // 仕様の範囲は 0.0..=1.0（`ExecutionGateInputs`）。範囲の外は同じ丸めを保証できないので throw し、Rust の判定に任せる。
  if (ratio !== null && (typeof ratio !== "number" || !(ratio >= 0 && ratio <= 1))) {
    throw new Error("execution_gate: recent_budget_exhausted_ratio must be null or within 0..=1");
  }
  return {
    features,
    title: string(o.title, "title"),
    objective: string(o.objective, "objective"),
    acceptance_total: count(o.acceptance_total, "acceptance_total"),
    artifact_exists: count(o.artifact_exists, "artifact_exists"),
    has_human: boolean(o.has_human, "has_human"),
    has_command: boolean(o.has_command, "has_command"),
    multi_environment: boolean(o.multi_environment, "multi_environment"),
    recent_budget_exhausted_ratio: ratio,
    cos_hint_compound: boolean(o.cos_hint_compound, "cos_hint_compound"),
    max_turns: count(o.max_turns, "max_turns"),
    tree_child: boolean(o.tree_child, "tree_child"),
    threshold: integer(o.threshold, "threshold"),
  };
}

function countProcessStages(text: string): number {
  const lower = text.toLowerCase();
  return PROCESS_WORD_GROUPS.filter((group) => group.some((w) => lower.includes(w.toLowerCase()))).length;
}

/**
 * Rust の `{ratio:.2}` と同じ文字列にする。正確な値で丸める点は `toFixed` と同じだが、2 桁のちょうど中間を
 * Rust は偶数へ、`toFixed` は大きい方へ丸める。2 桁の中間は 1/8 の奇数倍だけである（(2d+1)/200 が 2 進で有限なのは
 * 25 | 2d+1 のときで、その値は m/8、m は奇数）。8 倍は指数を変えるだけなので正確に判定できる。
 */
export function fixed2(x: number): string {
  const eighths = x * 8;
  if (Number.isInteger(eighths) && eighths % 2 !== 0) {
    const lower = Math.floor(x * 100);
    return ((lower % 2 === 0 ? lower : lower + 1) / 100).toFixed(2);
  }
  return x.toFixed(2);
}

export function decide(input: ExecutionGateInput): ExecutionGateOutput {
  const f = input.features;
  let score = 0;
  const signals: GateSignal[] = [];
  const add = (name: string, weight: number, detail: string) => {
    score += weight;
    signals.push({ name, weight, detail });
  };
  if (f.context_size === "high") add("F1", 2, "context_size=high");
  else if (f.context_size === "medium") add("F1", 1, "context_size=medium");
  if (f.expected_length === "high") add("F2", 2, "expected_length=high");
  if (f.tool_intensity === "high") add("F3", 1, "tool_intensity=high");
  if (f.cross_cutting === "high") add("F4", 2, "cross_cutting=high");
  else if (f.cross_cutting === "medium") add("F4", 1, "cross_cutting=medium");
  if (f.judgment === "high" && f.tool_intensity !== "low") add("F5", 1, "judgment=high and tool_intensity>=medium");
  const stages = countProcessStages(`${input.title}\n${input.objective}`);
  if (stages >= 3) add("S1", 2, `${stages} process stages mentioned`);
  else if (stages === 2) add("S1", 1, "2 process stages mentioned");
  if (input.acceptance_total >= 6 || input.artifact_exists >= 3) {
    add("S2", 1, `acceptance=${input.acceptance_total}, artifact_exists=${input.artifact_exists}`);
  }
  if (input.has_human && input.has_command) add("S3", 1, "has both Check::Human and Check::Command");
  if (input.multi_environment) add("S4", 1, "multiple execution environments");
  // Rust の `chars().count()` と同じく code point で数える。
  const objectiveLen = Array.from(input.objective).length;
  if (objectiveLen > 2000) add("S5", 1, `objective length ${objectiveLen} > 2000`);
  const ratio = input.recent_budget_exhausted_ratio;
  if (ratio !== null && ratio >= 0.3) add("S6", 2, `recent budget_exhausted ratio ${fixed2(ratio)} >= 0.3`);
  if (input.cos_hint_compound) add("H", 2, "CoS hinted execution=compound");

  let mode: ExecutionMode;
  let rule_id: string;
  if (f.expected_length === "high" && f.cross_cutting === "high") {
    mode = "compound";
    rule_id = "compound/long-and-broad";
  } else if (input.max_turns <= 10 && objectiveLen < 400 && !input.tree_child) {
    mode = "atomic";
    rule_id = "atomic/small";
  } else if (score >= input.threshold) {
    mode = "compound";
    rule_id = "compound/score";
  } else {
    mode = "atomic";
    rule_id = "atomic/score";
  }
  return { mode, score, rule_id, signals };
}

export async function executionGate(input: unknown): Promise<ExecutionGateOutput> {
  return decide(parseInput(input));
}
