// route_priority 領域（境界 ADR 2026-11-02 付記 S2-2・S2-10）: crates/task-core/src/llm_route.rs の
// `route_priority_decide` と同じ順で候補列を 1 本選ぶ:
// cos → その harness の execution class 列 → lane 列（lane が null なら空文字で引く）→ harness 全体の
// route_defaults → 空。選んだ列が空なら basis は none（下の列へは落ちない）。
// 並べ替え・重複除去はしない。候補が registry に在るかの確認・許可判定・frontier 列の組み立ては
// Rust に残す（ここに入れない）。入力の余計な欄・欠けた欄・型の違いは throw する。
// lane・class の照合は配列を前から === で探す（`__proto__` などの名前を object の key にしない）。

export type RoutePriorityBasis = "cos" | "execution_class" | "lane" | "harness" | "none";

/** Rust の `CompareCandidate`（欄の順は source・model）。 */
export interface CompareCandidate {
  source: string;
  model: string;
}

/** Rust の `LaneCandidates`。 */
export interface LaneCandidates {
  lane: string;
  candidates: CompareCandidate[];
}

/** Rust の `ClassCandidates`。 */
export interface ClassCandidates {
  execution_class: string;
  candidates: CompareCandidate[];
}

/** Rust の `RouteTable`（1 つの harness の候補列だけの写し）。 */
export interface RouteTable {
  route_defaults: CompareCandidate[];
  lane_route_defaults: LaneCandidates[];
  class_defaults: ClassCandidates[];
  cos_route_candidates: CompareCandidate[];
}

/** Rust の `RoutePriorityInput`。 */
export interface RoutePriorityInput {
  harness: string;
  lane: string | null;
  cos: boolean;
  execution_class: string | null;
  table: RouteTable;
}

/** 出力の欄の順は Rust の `RoutePriorityOutput` の宣言順（basis・candidates）。 */
export interface RoutePriorityOutput {
  basis: RoutePriorityBasis;
  candidates: CompareCandidate[];
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 欄の集合が `fields` と完全に同じ object かを確かめる（余計な欄・欠けた欄を拒む）。 */
function object(v: unknown, where: string, fields: readonly string[]): Record<string, unknown> {
  if (!isPlainObject(v)) throw new Error(`route_priority: ${where} must be an object`);
  for (const key of Object.keys(v)) {
    if (!fields.includes(key)) throw new Error(`route_priority: unknown field in ${where}: ${key}`);
  }
  for (const key of fields) {
    if (!Object.hasOwn(v, key)) throw new Error(`route_priority: missing field in ${where}: ${key}`);
  }
  return v;
}

function string(v: unknown, where: string): string {
  if (typeof v !== "string") throw new Error(`route_priority: ${where} must be a string`);
  return v;
}

function nullableString(v: unknown, where: string): string | null {
  return v === null ? null : string(v, `${where} (or null)`);
}

function array<T>(v: unknown, where: string, item: (x: unknown, at: string) => T): T[] {
  if (!Array.isArray(v)) throw new Error(`route_priority: ${where} must be an array`);
  return v.map((x, i) => item(x, `${where}[${i}]`));
}

function candidate(v: unknown, where: string): CompareCandidate {
  const o = object(v, where, ["source", "model"]);
  return { source: string(o.source, `${where}.source`), model: string(o.model, `${where}.model`) };
}

function candidates(v: unknown, where: string): CompareCandidate[] {
  return array(v, where, candidate);
}

/** 入力を検査して RoutePriorityInput にする。Rust の deny_unknown_fields と同じく未知・余計を拒み、
 *  欠けた欄も拒む（Option の欄も null を明示させる）。 */
export function parseInput(input: unknown): RoutePriorityInput {
  const o = object(input, "input", ["harness", "lane", "cos", "execution_class", "table"]);
  if (typeof o.cos !== "boolean") throw new Error("route_priority: input.cos must be a boolean");
  const t = object(o.table, "input.table", [
    "route_defaults",
    "lane_route_defaults",
    "class_defaults",
    "cos_route_candidates",
  ]);
  return {
    harness: string(o.harness, "input.harness"),
    lane: nullableString(o.lane, "input.lane"),
    cos: o.cos,
    execution_class: nullableString(o.execution_class, "input.execution_class"),
    table: {
      route_defaults: candidates(t.route_defaults, "table.route_defaults"),
      lane_route_defaults: array(t.lane_route_defaults, "table.lane_route_defaults", (x, at) => {
        const row = object(x, at, ["lane", "candidates"]);
        return { lane: string(row.lane, `${at}.lane`), candidates: candidates(row.candidates, `${at}.candidates`) };
      }),
      class_defaults: array(t.class_defaults, "table.class_defaults", (x, at) => {
        const row = object(x, at, ["execution_class", "candidates"]);
        return {
          execution_class: string(row.execution_class, `${at}.execution_class`),
          candidates: candidates(row.candidates, `${at}.candidates`),
        };
      }),
      cos_route_candidates: candidates(t.cos_route_candidates, "table.cos_route_candidates"),
    },
  };
}

/** basis を付ける前の列（Rust の `route_priority_column`）。 */
function column(input: RoutePriorityInput): RoutePriorityOutput {
  const { table } = input;
  if (input.cos) return { basis: "cos", candidates: table.cos_route_candidates };
  if (input.execution_class !== null) {
    const cls = input.execution_class;
    const row = table.class_defaults.find((r) => r.execution_class === cls);
    if (row !== undefined) return { basis: "execution_class", candidates: row.candidates };
  }
  const lane = input.lane ?? "";
  const laneRow = table.lane_route_defaults.find((r) => r.lane === lane);
  if (laneRow !== undefined) return { basis: "lane", candidates: laneRow.candidates };
  // Rust の写しは harness 全体の列が未宣言でも空配列にする。未宣言（none）と宣言済みの空（harness → none）は
  // どちらも下の規則で none になるので区別しない。
  return { basis: "harness", candidates: table.route_defaults };
}

/** 列を選び、空なら basis を none にする（付記 S2-10）。列は写すだけで並べ替え・重複除去をしない。 */
export function decide(input: RoutePriorityInput): RoutePriorityOutput {
  const { basis, candidates: list } = column(input);
  const out = list.map((c) => ({ source: c.source, model: c.model }));
  return { basis: out.length === 0 ? "none" : basis, candidates: out };
}

export async function routePriority(input: unknown): Promise<RoutePriorityOutput> {
  return decide(parseInput(input));
}
