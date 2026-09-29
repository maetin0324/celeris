import type {
  AttentionItem,
  Checkpoint,
  ExecutionMetrics,
  ExecutionPhase,
  ExecutionPlanOverview,
  ExecutionPlanVersionSummary,
  ExecutionView,
  ExecutionWorkUnitView,
  PhaseCheckpointView,
  PhaseGateAction,
  QuotaUse,
  RunEnd,
  WorkUnitKind,
  WorkUnitStatus,
} from "~/celeris/types";
import type { Tone } from "~/components/ui/tone";

/**
 * タスク詳細の「実行」節（celeris ADR-0072 D19/D20、`TaskDetail.execution`）の表示用の純粋関数。
 * celeris が記録・集計した値をそのまま並べるだけで、GUI 側では判定・集計を再計算しない。
 */

export const EXECUTION_SECTION_LABEL = "実行";

const NONE = "—";

export const EXECUTION_PHASE_LABEL: Record<ExecutionPhase, string> = {
  planning: "計画中",
  executing: "実行中",
  repairing: "修復中",
  verifying: "検証中",
  // celeris ADR-0074 D2.2（Phase F3 途中確認）。
  awaiting_human: "確認待ち",
  // celeris ADR-0079 D5（Phase R1b）: 子 task だけを待つ親（ready のまま、lease なし）。
  awaiting_children: "子 task の完了待ち",
  // celeris ADR-0079 D8（Phase R3b）: root の計画の承認待ち（画面の 3 つの操作は R4b）。
  awaiting_plan_approval: "計画の承認待ち",
};

export const EXECUTION_PHASE_TONE: Record<ExecutionPhase, Tone> = {
  planning: "info",
  executing: "primary",
  repairing: "warning",
  verifying: "teal",
  awaiting_human: "warning",
  awaiting_children: "info",
  awaiting_plan_approval: "warning",
};

export const WORK_UNIT_STATUS_TONE: Record<WorkUnitStatus, Tone> = {
  pending: "neutral",
  ready: "info",
  needs_continuation: "warning",
  running: "primary",
  done: "success",
  failed: "danger",
  blocked: "warning",
  superseded: "neutral",
  cancelled: "neutral",
};

export const WORK_UNIT_KIND_LABEL: Record<WorkUnitKind, string> = {
  investigate: "調査",
  design: "設計",
  implement: "実装",
  test: "テスト",
  release: "リリース",
  repair: "修復",
  integrate: "統合",
  // ADR-0079（Phase R1a）: plan/3 の子 task の unit（行は子 task の代理）。
  task: "子 task",
  other: "その他",
};

const RUN_END_LABEL: Record<RunEnd["type"], string> = {
  completed: "completed",
  yielded: "yielded",
  budget_exhausted: "budget_exhausted",
  question: "question",
  failed: "failed",
  harness_error: "harness_error",
  cancelled: "cancelled",
};

export const RUN_END_TONE: Record<RunEnd["type"], Tone> = {
  completed: "success",
  yielded: "info",
  budget_exhausted: "warning",
  question: "info",
  failed: "danger",
  harness_error: "danger",
  cancelled: "neutral",
};

/** run の終わり方のバッジ文言（`budget_exhausted` は種類も添える）。無ければ `null`（導入前の run）。 */
export function runEndLabel(end: RunEnd | null | undefined): string | null {
  if (!end) return null;
  if (end.type === "budget_exhausted") return `budget_exhausted(${end.kind})`;
  return RUN_END_LABEL[end.type];
}

export function runEndTone(end: RunEnd | null | undefined): Tone {
  return end ? RUN_END_TONE[end.type] : "neutral";
}

/** D20: 計画の無いタスクの 1 行要約。 */
export function directExecutionSummary(metrics: ExecutionMetrics): string {
  const runs = metrics.runs_by_role?.worker ?? 0;
  return `直接実行（Run ${runs} 回、continuation ${metrics.continuations} 回）`;
}

/**
 * 今どの WorkUnit を見せるか（celeris `next_work_unit` の優先順位と同じ考え方:
 * running → needs_continuation → ready → blocked。すべて無ければ最後の done）。
 */
export function currentWorkUnit(plan: ExecutionPlanOverview): ExecutionWorkUnitView | null {
  const byStatus = (status: WorkUnitStatus): ExecutionWorkUnitView | null =>
    [...plan.work_units].filter((w) => w.status === status).sort((a, b) => a.seq - b.seq)[0] ?? null;
  return (
    byStatus("running") ??
    byStatus("needs_continuation") ??
    byStatus("ready") ??
    byStatus("blocked") ??
    [...plan.work_units].sort((a, b) => b.seq - a.seq)[0] ??
    null
  );
}

/** D20 の見出し: 「3 / 6 WorkUnits 完了 · 現在: <title> · Run #N · <model> · <harness> · <status>」。 */
export function planSummaryLine(plan: ExecutionPlanOverview, metrics: ExecutionMetrics): string {
  const done = metrics.work_units_done;
  const total = plan.work_units.length;
  const base = `${done} / ${total} WorkUnits 完了`;
  const current = currentWorkUnit(plan);
  if (!current) return base;
  return [
    base,
    `現在: ${current.title}`,
    `Run #${current.runs + 1}`,
    current.model ?? NONE,
    current.harness ?? NONE,
    current.status,
  ].join(" · ");
}

/** checkpoint の折り畳みの見出し（1 行要約）。 */
export function checkpointSummary(cp: Checkpoint | null | undefined): string {
  if (!cp) return "checkpoint なし";
  return `${cp.next_action}（remaining ${(cp.remaining ?? []).length} 件・completed ${(cp.completed ?? []).length} 件）`;
}

/** 版の履歴の 1 行（版・出自・状態・理由）。 */
export function planVersionLabel(v: ExecutionPlanVersionSummary): string {
  const reason = v.reason ? ` — ${v.reason}` : "";
  return `v${v.version}（${v.origin} / ${v.status}）${reason}`;
}

// ---------------------------------------------------------------------------
// ADR-0074 D1（Phase F2b）: WU の並列実行（工程・WU のブランチ・同時に走っている run）。
// celeris が記録した値（`phase`/`branch`/`running_run_id`/`serialized_reason`）を並べるだけ。
// ---------------------------------------------------------------------------

/** WU の表の 1 まとまり（v2 は工程ごと、v1 は見出しの無い 1 つ）。 */
export interface WorkUnitGroup {
  /** 工程の key（見出しの無いまとまりは空文字列）。 */
  key: string;
  /** 見出しの文言（見出しの無いまとまりは空文字列）。 */
  label: string;
  units: ExecutionWorkUnitView[];
}

/** WU の表を工程（`plan.phases` の順）ごとにまとめる。工程の無い計画（v1）は見出しの無い 1 つ。 */
export function workUnitGroups(plan: ExecutionPlanOverview): WorkUnitGroup[] {
  const sorted = [...plan.work_units].sort((a, b) => a.seq - b.seq);
  const phases = plan.phases ?? [];
  if (phases.length === 0) return [{ key: "", label: "", units: sorted }];
  const groups: WorkUnitGroup[] = phases.map((p) => ({
    key: p.key,
    label: `工程 ${p.title}（${p.key}）`,
    units: sorted.filter((w) => w.phase === p.key),
  }));
  const known = new Set(phases.map((p) => p.key));
  const rest = sorted.filter((w) => !w.phase || !known.has(w.phase));
  if (rest.length > 0) groups.push({ key: "", label: "工程なし", units: rest });
  return groups;
}

/** 今走っている WU の run（`running_run_id` を持つ WU、seq 順）。 */
export function runningWorkUnitRuns(plan: ExecutionPlanOverview): { key: string; runId: string }[] {
  return [...plan.work_units]
    .sort((a, b) => a.seq - b.seq)
    .flatMap((w) => (w.running_run_id ? [{ key: w.key, runId: w.running_run_id }] : []));
}

/**
 * 並列実行の 1 行（並列 1 に倒した理由、または同時に走っている run の本数）。どちらでも無ければ `null`。
 */
export function parallelSummaryLine(plan: ExecutionPlanOverview): string | null {
  if (plan.serialized_reason) return `並列 1 で実行（${plan.serialized_reason}）`;
  const running = runningWorkUnitRuns(plan);
  if (running.length > 1) {
    return `同時に走っている run: ${running.length} 本（${running.map((r) => r.key).join(", ")}）`;
  }
  return null;
}

/** commit の短縮表示（先頭 12 桁）。 */
export function shortCommit(sha: string | null | undefined): string | null {
  return sha ? sha.slice(0, 12) : null;
}

/** repair WU の印（D16「repair WU の印」）。 */
export function isRepairWorkUnit(wu: ExecutionWorkUnitView): boolean {
  return wu.kind === "repair";
}

/** gate の判定の 1 行（無ければ `null`）。 */
export function gateModeLabel(execution: ExecutionView | null | undefined): string | null {
  const gate = execution?.gate;
  if (!gate) return null;
  const shadow = gate.shadow ? "（shadow）" : "";
  // celeris ADR-0079 D4 (1)（Phase R2a）: 木の子の gate は深さの閾値で判定し、shadow でも採用する。
  const tree = gate.depth != null && gate.depth >= 2 ? `（木の子: 深さ ${gate.depth}・閾値 ${gate.threshold}）` : "";
  return `${gate.mode}${shadow}${tree} — ${gate.rule_id}`;
}

// ---------------------------------------------------------------------------
// ADR-0074 D4（Phase F3 quota）: quota 消費が主指標、定価 USD は参考。
// ---------------------------------------------------------------------------

const QUOTA_WINDOW_LABEL: Record<QuotaUse["window"], string> = {
  five_hour: "5h",
  seven_day: "7d",
};

/**
 * D4.2 の method の内訳から、この行の確からしさを 1 語で表す。celeris が決めた `method_counts` を
 * そのまま読むだけで、GUI 側では判定しない（D4「観測と記録だけ」）。複数の method が混じっていれば
 * 一番弱いもの（`unknown` > `estimated` > `apportioned` > `measured`）を見せる。
 */
function quotaConfidenceLabel(row: QuotaUse): string {
  const counts = row.method_counts ?? {};
  const has = (method: string) => (counts[method] ?? 0) > 0;
  if (has("unknown")) return "一部不明";
  if (has("estimated")) return "推定";
  if (has("apportioned")) return "按分";
  if (has("free")) return "無料";
  if (has("measured")) return "実測";
  return "不明";
}

/**
 * D4.3/D4（GUI (j)）: quota が主表示。1 行 = 1 (source, account, window)。`used_pct` が無い
 * （`unknown` だけの行）は「不明」と書き、`0` とは書かない（unknown_is_never_zero と同じ規律）。
 */
export function quotaSummaryLines(metrics: ExecutionMetrics): string[] {
  const rows = metrics.quota ?? [];
  return rows.map((row) => {
    const label = row.account ? `${row.source}（${row.account}）` : row.source;
    const window = QUOTA_WINDOW_LABEL[row.window] ?? row.window;
    const pct = row.used_pct == null ? "不明" : `${row.used_pct.toFixed(1)}pt`;
    return `${label} ${window} ${pct}（${quotaConfidenceLabel(row)}）`;
  });
}

/**
 * D4.3/D4（GUI (j)）: 定価 USD は参考値。`cost_usd_complete === false`（単価不明のモデルが混ざる）
 * なら「不完全」を明示する（E6 report 問題 5 の再発防止）。`cost_usd` 自体が無ければ `null`。
 */
export function costReferenceLabel(metrics: ExecutionMetrics): string | null {
  if (metrics.cost_usd == null) return null;
  const amount = `$${metrics.cost_usd.toFixed(2)}`;
  return metrics.cost_usd_complete === false
    ? `参考 ${amount}（一部のモデルの単価が不明なため過小）`
    : `参考 ${amount}`;
}

/**
 * celeris ADR-0074 D2.3/D2.4（Phase F3 途中確認）: 途中報告の見出し（`工程「<title>」まで進みました`）。
 * 報告そのものは celeris が決定的に組み立てたもの。GUI は並べ替えも要約もしない。
 */
export function phaseCheckpointHeadline(cp: PhaseCheckpointView): string {
  const title = cp.report.phase_title || cp.report.phase;
  return `工程「${title}」まで進みました。続けるか・計画を立て直すか・取り下げるかを選んでください。`;
}

/** 次の工程の 1 行（無ければ「最終レビューへ」）。 */
export function phaseCheckpointNextLine(cp: PhaseCheckpointView): string {
  const next = cp.report.next_phase;
  if (!next) return "次: この工程が最後です（続けると最終レビューへ）。";
  const wus = cp.report.next_phase_work_units ?? [];
  return wus.length > 0 ? `次の工程: ${next}（${wus.join("、")}）` : `次の工程: ${next}`;
}

/** 途中報告の節（見出し・行）。空の節は出さない。 */
export function phaseCheckpointSections(cp: PhaseCheckpointView): { title: string; lines: string[] }[] {
  const r = cp.report;
  const sections: { title: string; lines: string[] }[] = [
    { title: "済んだ工程", lines: r.phases_done ?? [] },
    { title: "この工程の WU", lines: r.work_units ?? [] },
    { title: "統合", lines: r.integration ?? [] },
    { title: "差分", lines: r.diff_stat ?? [] },
    { title: "quota", lines: r.quota_summary ? [r.quota_summary] : [] },
    { title: "成果物", lines: r.artifact_paths ?? [] },
  ];
  return sections.filter((s) => s.lines.length > 0);
}

/** 3 つのボタンの表示名。 */
export const PHASE_GATE_ACTION_LABEL: Record<PhaseGateAction, string> = {
  continue: "続ける",
  replan: "計画を立て直す（replan）",
  withdraw: "取り下げる",
};

/**
 * celeris ADR-0074 D2.4（Phase F3 途中確認）: 受信箱の「工程の後で止まった」1 行。値は celeris のもの
 * （`phases_done` は止まった工程を含む数）。
 */
export function phaseCheckpointAttentionText(item: Extract<AttentionItem, { type: "phase_checkpoint" }>): string {
  const title = item.phase_title || item.phase;
  const progress = item.phases_total > 0 ? `（${item.phases_done}/${item.phases_total} 工程）` : "";
  const next = item.next_phase ? `次: ${item.next_phase}` : "次: 最終レビュー";
  return `工程「${title}」まで進みました${progress}。確認を待っています。${next}`;
}
