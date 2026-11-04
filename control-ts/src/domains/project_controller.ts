// project_controller 領域（ADR 2026-11-04-project-controller D7・A2）: 案件の snapshot（Rust の ProjectSnapshot）から
// 途中目標（Outcome）の gap を計算し、次の行動 1 つを proposal（Rust の ProjectProposal）で返す。
// TS は DB も system も書かない。起票・CoS の入力・受信箱・記録は Rust が proposal を検証してから行う。
// 参照実装は無い（旧 Rust の判定は存在しない）。方針の期待値は project_controller.test.ts、Rust が守らせる不変条件は
// fixtures/project_controller/invariants.jsonl にある。領域の file は互いに import しない（compare.mjs の loader の制約）。
// worker の報告・成果物の文は snapshot に無い。root の本文に転記するのは人の依頼文（request）と Outcome の文だけ（I11）。

export type ActionKind = "continue" | "replan" | "launch_root" | "wait" | "escalate" | "mark_milestone";

export interface Outcome {
  id: string;
  title: string;
  acceptance: string[];
  depends_on: string[];
  priority: number;
  source: string;
}

export interface SnapshotRoot {
  id: string;
  status: string;
  title: string;
  controller_key: string | null;
  outcome_ids: string[];
  acceptance: { text: string; kind: string; result: string }[];
  reviewer_passed: boolean;
  integrated: boolean;
  failure_class: string | null;
  attempts: number;
  open_waits: string[];
  leaf_done: number;
  leaf_total: number;
  continues_root_ids: string[];
}

export interface SnapshotDecision {
  id: string;
  kind: string;
  rule_id: string;
  effect_state: string;
  root_key: string | null;
  reason_code: string | null;
  outcome_id: string | null;
  request_sha256: string;
  progress_epoch: number;
  at: string;
}

export interface ProjectSnapshot {
  now: string;
  truncated: boolean;
  project: { id: string; title: string; status: string; request: string; request_sha256: string; auto_integrate: boolean };
  brief: unknown;
  outcomes: Outcome[];
  outcomes_invalid: boolean;
  roots: SnapshotRoot[];
  waits: { kind: string; id: string; task_id: string; age_secs: number }[];
  stagnation: { kind: string; key: string; since: string }[];
  wakes: { kind: string; ref: string; event_id: number }[];
  decisions: SnapshotDecision[];
  controller: { mode: string; phase: string; revision: number; human_epoch: number; retry_count: number; progress_epoch: number };
  limits: {
    max_open_roots: number;
    max_root_attempts_per_outcome: number;
    max_no_progress_decisions: number;
    cos_inputs_remaining_today: number;
    roots_remaining_today: number;
  };
  roadmap: unknown;
  authority: unknown;
}

export interface WaitingOn {
  kind: "human" | "task" | "external";
  target?: string;
  note: string;
}

export interface RootSpec {
  key: string;
  title: string;
  objective: string;
  acceptance: { text: string; check: { type: "reviewer" | "human" } }[];
  stages_hint: string[];
  continues_root_ids: string[];
  outcome_ids: string[];
}

export type ProjectAction =
  | { kind: "continue"; watch_root_ids: string[]; next_wake_after_secs: number }
  | { kind: "replan"; reason_code: string; question: string; context_refs: string[] }
  | { kind: "launch_root"; root: RootSpec }
  | { kind: "wait"; waiting_on: WaitingOn[]; next_wake_after_secs: number; phase: "waiting" | "achieved" }
  | { kind: "escalate"; category: string; question_key: string; summary: string; options: string[]; evidence_refs: string[] }
  | { kind: "mark_milestone"; outcome_id: string; evidence_refs: string[] };

export interface ProjectProposal {
  action: ProjectAction;
  gap: { reached: string[]; open: string[]; unknown: string[]; progress_epoch: number };
  explanation: { rule_id: string; signals: { name: string; detail: string }[] };
  based_on: { revision: number; human_epoch: number; request_sha256: string };
}

/** 方針の定数（TS だけで変えられる）。 */
export const POLICY = {
  /** 走っている root を見に来る間隔。 */
  watchSecs: 3600,
  /** 人・CoS の返事を待つ間隔。 */
  humanWaitSecs: 6 * 3600,
  /** 基盤の失敗の後に見直すまでの秒。 */
  infraBackoffSecs: 1800,
  /** Rust の上限（ROOT_TITLE_MAX・ROOT_OBJECTIVE_MAX）。 */
  titleMax: 200,
  objectiveMax: 8000,
} as const;

const TERMINAL = new Set(["done", "failed", "cancelled"]);

/** 失敗の分類（ADR §A3 の Classification）。判断の explanation の signal `classification` で Rust に渡る。 */
export type Classification = "worker_bug" | "spec_gap" | "infra" | "flaky" | "external" | "human_needed" | "unknown";

/**
 * root の失敗を分類する（A3）。snapshot の種類と数だけを見る（worker の報告・ログの文は読まない。I11）。
 * - 人の判断の待ちが開いている → human_needed
 * - review 不合格で決定的 check が落ちている → worker_bug、check は通ったが reviewer が落とした → spec_gap
 * - 基盤の失敗: 2 回目以降は flaky、初回は infra
 * - cancel → external（人か外部が止めた）
 */
export function classifyFailure(root: SnapshotRoot): Classification {
  if (root.open_waits.length > 0) return "human_needed";
  switch (root.failure_class) {
    case "review_failed":
      return root.acceptance.some((c) => c.kind === "command" && c.result === "fail") ? "worker_bug" : "spec_gap";
    case "infra":
      return root.attempts >= 2 ? "flaky" : "infra";
    case "cancelled":
      return "external";
    default:
      return "unknown";
  }
}

function fail(what: string): never {
  // 例外文に依頼文・題名を入れない。
  throw new Error(`project_controller input: ${what}`);
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 入力の形を確かめる（Rust の deny_unknown_fields と同じ欄の集合）。 */
export function parseInput(input: unknown): ProjectSnapshot {
  if (!isObj(input)) fail("not an object");
  const keys = [
    "now", "truncated", "project", "brief", "outcomes", "outcomes_invalid", "roots", "waits", "stagnation",
    "wakes", "decisions", "controller", "limits", "roadmap", "authority",
  ];
  for (const k of keys) if (!(k in input)) fail(`missing ${k}`);
  for (const k of Object.keys(input)) if (!keys.includes(k)) fail(`unknown field ${k}`);
  const s = input as unknown as ProjectSnapshot;
  if (!isObj(s.project) || typeof s.project.request_sha256 !== "string") fail("project");
  if (!isObj(s.controller) || typeof s.controller.revision !== "number" || typeof s.controller.human_epoch !== "number")
    fail("controller");
  if (!isObj(s.limits)) fail("limits");
  for (const k of ["outcomes", "roots", "waits", "decisions"] as const) if (!Array.isArray(s[k])) fail(k);
  return s;
}

function truncate(text: string, max: number): string {
  const cs = Array.from(text);
  return cs.length <= max ? text : cs.slice(0, max - 1).join("") + "…";
}

/** root が Outcome を満たしたか（D6）: done・合格・取り込み済みで、Outcome を名指すか acceptance を文字どおり含む。 */
function satisfies(root: SnapshotRoot, o: Outcome): boolean {
  if (root.status !== "done" || !root.reviewer_passed || !root.integrated) return false;
  if (root.outcome_ids.includes(o.id)) return true;
  const texts = new Set(root.acceptance.map((c) => c.text));
  return o.acceptance.every((a) => texts.has(a));
}

function keyOutcome(key: string | null): { id: string; n: number } | null {
  const m = key === null ? null : /^outcome:([a-z0-9_-]{1,32}):([1-9][0-9]*)$/.exec(key);
  return m === null ? null : { id: m[1]!, n: Number(m[2]) };
}

/** 次の行動を決める。純粋関数（時計・乱数・host を使わない）。 */
export function decide(s: ProjectSnapshot): ProjectProposal {
  const based_on = {
    revision: s.controller.revision,
    human_epoch: s.controller.human_epoch,
    request_sha256: s.project.request_sha256,
  };
  const sha = s.project.request_sha256;
  const reachedBy = new Map<string, SnapshotRoot>();
  for (const o of s.outcomes) {
    const r = s.roots.find((root) => satisfies(root, o));
    if (r !== undefined) reachedBy.set(o.id, r);
  }
  const reached = s.outcomes.filter((o) => reachedBy.has(o.id)).map((o) => o.id);
  const open = s.outcomes.filter((o) => !reachedBy.has(o.id)).map((o) => o.id);
  const passedRoots = s.roots.filter((r) => r.status === "done" && r.reviewer_passed && r.integrated).length;
  const gap = {
    reached,
    open,
    unknown: [] as string[],
    progress_epoch: Math.max(s.controller.progress_epoch, reached.length + passedRoots),
  };
  const out = (action: ProjectAction, rule_id: string, signals: [string, string][] = []): ProjectProposal => ({
    action,
    gap,
    explanation: { rule_id, signals: signals.slice(0, 16).map(([name, detail]) => ({ name, detail: truncate(detail, 256) })) },
    based_on,
  });
  const wait = (waiting_on: WaitingOn[], secs: number, rule: string, signals: [string, string][] = []) =>
    out({ kind: "wait", waiting_on, next_wake_after_secs: secs, phase: "waiting" }, rule, signals);
  const decided = (pred: (d: SnapshotDecision) => boolean) =>
    s.decisions.some((d) => d.effect_state !== "failed" && pred(d));
  const escalateOnce = (key: string, category: string, summary: string, rule: string, refs: string[]) =>
    decided((d) => d.kind === "escalate" && d.root_key === key)
      ? wait([{ kind: "human", note: truncate(summary, 256) }], POLICY.humanWaitSecs, `${rule}/waiting`, [["key", key]])
      : out(
          { kind: "escalate", category, question_key: key, summary, options: [], evidence_refs: refs },
          rule,
          [["key", key]],
        );

  // 1. 案件が動いていない（paused・cancelled 等）。起票しない。
  if (s.project.status !== "active") {
    return wait([{ kind: "human", note: `project is ${s.project.status}` }], POLICY.humanWaitSecs, "pc/project-not-active");
  }
  // 1b. 進捗の増えない効果つきの判断（replan・launch_root）が上限まで続いた: 人に 1 回だけ聞き、後は待つ
  //     （LLM の runaway・loop の停止。Rust も同じ上限で escalate に置き換える）。
  let streak = 0;
  for (const d of s.decisions) {
    if (d.progress_epoch !== gap.progress_epoch) break;
    if ((d.kind === "replan" || d.kind === "launch_root") && d.effect_state !== "failed" && d.effect_state !== "none") streak++;
  }
  if (s.limits.max_no_progress_decisions > 0 && streak >= s.limits.max_no_progress_decisions) {
    return escalateOnce(
      `no_progress:${gap.progress_epoch}`,
      "explicit_human",
      `案件の controller が進捗なしの判断を ${streak} 回続けた。続け方を人が決める必要がある`,
      "pc/no-progress",
      [],
    );
  }
  // CoS 入力（replan）の今日の残りが無ければ、replan の代わりに待つ。
  const replan = (reason_code: string, question: string, context_refs: string[], rule: string, signals: [string, string][] = []) =>
    s.limits.cos_inputs_remaining_today <= 0
      ? wait([{ kind: "external", note: "今日の CoS 入力の上限" }], POLICY.humanWaitSecs, `${rule}/daily-limit`, signals)
      : out({ kind: "replan", reason_code, question, context_refs }, rule, signals);
  // 2. Outcome block の形が誤り: 人に 1 回だけ知らせ、後は待つ。
  if (s.outcomes_invalid) {
    return escalateOnce(
      `outcomes_invalid:${sha.slice(0, 12)}`,
      "objective_change",
      "依頼文の celeris-outcomes block を読めない。人が依頼文の block を直す必要がある",
      "pc/outcomes-invalid",
      [],
    );
  }
  // 3. 途中目標が無い: 既存の案件 CoS run に計画を頼む（request の版ごとに 1 回）。
  if (s.outcomes.length === 0) {
    if (decided((d) => d.kind === "replan" && d.reason_code === "no_outcomes" && d.request_sha256 === sha)) {
      return wait([{ kind: "human", note: "途中目標の提案を待っている" }], POLICY.humanWaitSecs, "pc/no-outcomes/waiting");
    }
    return replan(
      "no_outcomes",
      "依頼文に途中目標（celeris-outcomes block）が無い。依頼を途中目標に分ける案を人に提案してほしい",
      [`project:${s.project.id}`],
      "pc/no-outcomes",
    );
  }
  // 4. 新しく到達した Outcome を記録する（記録が先。次の評価で続きを起票する）。
  for (const o of s.outcomes) {
    const root = reachedBy.get(o.id);
    if (root !== undefined && !decided((d) => d.kind === "mark_milestone" && d.outcome_id === o.id)) {
      return out({ kind: "mark_milestone", outcome_id: o.id, evidence_refs: [`task:${root.id}`] }, "pc/milestone-reached", [
        ["outcome", o.id],
        ["root", root.id],
      ]);
    }
  }
  // 5. 全部到達: 起票しない。
  if (open.length === 0) {
    return out({ kind: "wait", waiting_on: [], next_wake_after_secs: 0, phase: "achieved" }, "pc/achieved", [
      ["reached", reached.join(",")],
    ]);
  }
  // 6. 人の判断を待っている間は次を起票しない。
  if (s.waits.length > 0) {
    const w = s.waits[0]!;
    return wait(
      s.waits.slice(0, 16).map((x) => ({ kind: "human" as const, target: x.task_id, note: `${x.kind} ${x.id}` })),
      POLICY.humanWaitSecs,
      "pc/human-wait",
      [["wait", `${w.kind}:${w.id}`]],
    );
  }
  // 7. 進行中（未終端、または合格したが取り込み待ち）の root があれば見守る。tree・dispatcher の待ちを重複しない（I15）。
  const inFlight = s.roots.filter(
    (r) => !TERMINAL.has(r.status) || (r.status === "done" && r.reviewer_passed && !r.integrated),
  );
  if (inFlight.length > 0) {
    return out(
      { kind: "continue", watch_root_ids: inFlight.map((r) => r.id), next_wake_after_secs: POLICY.watchSecs },
      "pc/root-in-flight",
      inFlight.slice(0, 16).map((r) => ["root", `${r.id} ${r.status}${r.integrated ? "" : " not-integrated"}`]),
    );
  }
  // 8. 次の Outcome: 依存が全部到達したもののうち priority の大きい順、同じなら宣言順。
  const ready = s.outcomes
    .map((o, i) => ({ o, i }))
    .filter(({ o }) => !reachedBy.has(o.id) && o.depends_on.every((d) => reachedBy.has(d)))
    .sort((a, b) => b.o.priority - a.o.priority || a.i - b.i);
  if (ready.length === 0) {
    return escalateOnce(
      `outcomes_blocked:${sha.slice(0, 12)}`,
      "objective_change",
      "未達の途中目標の依存が満たせない",
      "pc/outcomes-blocked",
      [],
    );
  }
  const next = ready[0]!.o;
  const mine = s.roots
    .map((r) => ({ r, k: keyOutcome(r.controller_key) }))
    .filter((x) => x.k !== null && x.k.id === next.id)
    .sort((a, b) => a.k!.n - b.k!.n);
  const last = mine.at(-1);
  // 9. 直前の試行が失敗していれば、その分類で次を決める。
  if (last !== undefined && TERMINAL.has(last.r.status)) {
    const r = last.r;
    const key = r.controller_key!;
    const cls = classifyFailure(r);
    if (r.failure_class === "review_failed") {
      if (decided((d) => d.kind === "replan" && d.root_key === key)) {
        return wait([{ kind: "human", target: r.id, note: "review 不合格の後の計画を待っている" }], POLICY.humanWaitSecs, "pc/review-failed/waiting", [
          ["root", r.id],
          ["classification", cls],
        ]);
      }
      return replan(
        "review_failed",
        `途中目標 ${next.id} の root が review で不合格になった（分類 ${cls}）。続け方（修正の root・目標の見直し）を決めてほしい`,
        [`task:${r.id}`],
        "pc/review-failed",
        [
          ["root", r.id],
          ["classification", cls],
        ],
      );
    }
    if (r.failure_class === "infra") {
      return wait(
        [{ kind: "external", target: r.id, note: "基盤の失敗。再試行は dispatcher の retry に任せる" }],
        POLICY.infraBackoffSecs,
        "pc/infra-wait",
        [
          ["root", r.id],
          ["classification", cls],
        ],
      );
    }
    if (r.status !== "done") {
      return escalateOnce(`root_failed:${r.id}`, "explicit_human", `途中目標 ${next.id} の root が ${r.status} で終わった`, "pc/root-ended", [
        `task:${r.id}`,
      ]);
    }
  }
  const attempt = (last?.k?.n ?? 0) + 1;
  if (attempt > s.limits.max_root_attempts_per_outcome) {
    return escalateOnce(`attempts_exhausted:${next.id}`, "explicit_human", `途中目標 ${next.id} の root の試行が上限に達した`, "pc/attempts-exhausted", []);
  }
  if (s.limits.roots_remaining_today <= 0) {
    return wait([{ kind: "external", note: "今日の起票の上限" }], POLICY.humanWaitSecs, "pc/daily-limit");
  }
  // 10. 続きの root を起票する。既存の root の done は変えず、continues_root_ids で履歴を結ぶ（D3）。
  const continues = s.roots
    .filter((r) => r.status === "done" && r.reviewer_passed && next.depends_on.some((d) => satisfies(r, s.outcomes.find((o) => o.id === d)!)))
    .map((r) => r.id);
  if (last !== undefined) continues.push(last.r.id);
  const objective = [
    `案件「${s.project.title}」の途中目標 ${next.id}「${next.title}」を達成する。`,
    continues.length > 0 ? `この root は先の root ${continues.join(", ")} の成果の続き。先の成果は main にある。` : "",
    "案件の依頼（人の原文）:",
    s.project.request,
  ]
    .filter((x) => x !== "")
    .join("\n\n");
  return out(
    {
      kind: "launch_root",
      root: {
        key: `outcome:${next.id}:${attempt}`,
        title: truncate(`${s.project.title}: ${next.title}`, POLICY.titleMax),
        objective: truncate(objective, POLICY.objectiveMax),
        acceptance: next.acceptance.map((text) => ({ text, check: { type: "reviewer" as const } })),
        stages_hint: [],
        continues_root_ids: [...new Set(continues)],
        outcome_ids: [next.id],
      },
    },
    "pc/outcome-open",
    [
      ["outcome", next.id],
      ["attempt", String(attempt)],
    ],
  );
}

/** 領域の入口。 */
export function projectController(input: unknown): ProjectProposal {
  return decide(parseInput(input));
}
