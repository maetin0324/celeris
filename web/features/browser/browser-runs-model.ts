import type { BrowserWait } from "../../api/generated/types";
import type { BadgeTone } from "../../components/ui/badge";
import { browserRunBadge, type LiveUnavailableReason, liveUnavailableText, safeLivePath } from "./browser-model";
import type { BrowserRunItem, ControlStatus } from "./browser-query";

// /browser 一覧の表示 model（ADR 2026-10-05-browser-department-web-live-view D3.1・D3.3）。
// 行は gateway の /browser/runs（live_view_url は gateway が落とす）と未決の waits・control の phase から作る。

export type LeaseBadge = { label: string; tone: BadgeTone };
export type LiveCell =
  | { available: true; label: string }
  | { available: false; reason: LiveUnavailableReason; label: string };

export type BrowserRunRow = {
  taskId: string;
  runId: string;
  sessionId: string;
  taskTitle: string | null;
  state: BrowserRunItem["state"];
  stateLabel: string;
  stateTone: BadgeTone;
  active: boolean;
  openWaits: number;
  lease: LeaseBadge | null;
  /** lease badge が無い実行中の run の理由。取得中（loading）か取得失敗（unavailable）。 */
  leasePending: "loading" | "unavailable" | null;
  live: LiveCell;
};

const ENDED: ReadonlySet<string> = new Set(["COMPLETED", "FAILED"]);

export function isActiveRun(run: Pick<BrowserRunItem, "state">): boolean {
  return !ENDED.has(run.state);
}

/** lease の状態 badge。終了した run と phase 不明は null（行に「—」を出す）。 */
export function leaseBadge(phase: ControlStatus["phase"] | null | undefined): LeaseBadge | null {
  switch (phase) {
    case "agent_running":
    case "pausing":
      return { label: "監視のみ", tone: "neutral" };
    case "human_control":
      return { label: "操作中", tone: "warning" };
    case "paused":
      return { label: "一時停止中（人の返却待ち）", tone: "warning" };
    case "stopped":
      return { label: "停止", tone: "neutral" };
    default:
      return null;
  }
}

function stateTone(state: BrowserRunItem["state"], phase: ControlStatus["phase"] | null | undefined): BadgeTone {
  if (phase === "paused") return "warning";
  if (state === "RUNNING") return "running";
  if (state === "COMPLETED") return "success";
  if (state === "FAILED") return "danger";
  return "warning";
}

const LIVE_REASONS = new Set<string>(Object.keys(liveUnavailableText));

/** live の可否と理由。gateway が理由を返せばそれを使い、無ければ run の状態・待ち・href から導く。 */
export function liveCell(run: BrowserRunItem, authWaitOpen: boolean): LiveCell {
  const unavailable = (reason: LiveUnavailableReason): LiveCell => ({
    available: false,
    reason,
    label: liveUnavailableText[reason],
  });
  if (run.live?.state === "disabled")
    return unavailable(
      LIVE_REASONS.has(run.live.reason) ? (run.live.reason as LiveUnavailableReason) : "relay_unavailable",
    );
  if (!isActiveRun(run)) return unavailable("not_running");
  if (authWaitOpen) return unavailable("auth_interval");
  const href = run.live?.state === "link" ? run.live.href : run.live_path;
  if (safeLivePath(href) === null) return unavailable("not_configured");
  return { available: true, label: "Live View を開けます" };
}

function rank(row: BrowserRunRow): number {
  if (row.lease?.label === "一時停止中（人の返却待ち）" || row.openWaits > 0) return 0;
  if (row.active) return 1;
  return 2;
}

/**
 * 一覧の行を作る。並びは「人の対応が要る（返却待ち・未決の待ち）」→「実行中」→「終了」、同じ群の中は
 * run id の降順（ULID なので新しい順）、同じ run id は task id 順。
 */
export function buildRunRows(
  runs: readonly BrowserRunItem[],
  waits: readonly BrowserWait[],
  options: {
    phases?: ReadonlyMap<string, ControlStatus["phase"]>;
    titles?: ReadonlyMap<string, string>;
    /** control の取得に失敗した run（`task/run`）。 */
    controlFailed?: ReadonlySet<string>;
  } = {},
): BrowserRunRow[] {
  const rows = runs.map((run): BrowserRunRow => {
    const key = `${run.task_id}/${run.run_id}`;
    const active = isActiveRun(run);
    const phase = active ? options.phases?.get(key) : undefined;
    const open = waits.filter((w) => w.state === "pending" && w.task_id === run.task_id && w.run_id === run.run_id);
    return {
      taskId: run.task_id,
      runId: run.run_id,
      sessionId: run.session_id,
      taskTitle: options.titles?.get(run.task_id) ?? null,
      state: run.state,
      stateLabel: browserRunBadge(run, phase),
      stateTone: stateTone(run.state, phase),
      active,
      openWaits: open.length,
      lease: active ? leaseBadge(phase) : null,
      leasePending: !active || leaseBadge(phase) ? null : options.controlFailed?.has(key) ? "unavailable" : "loading",
      live: liveCell(
        run,
        open.some((w) => w.reason === "waiting_for_auth"),
      ),
    };
  });
  return rows.sort(
    (a, b) =>
      rank(a) - rank(b) || (a.runId < b.runId ? 1 : a.runId > b.runId ? -1 : 0) || a.taskId.localeCompare(b.taskId),
  );
}
