import type { RunSummary } from "../../api/generated/types";
import type { RunLogEvent } from "./run-log";

// run ログ画面の header と各 event の見出しに出す値（純粋関数。表示部品は run-log-view.tsx）。

/** 末尾からこの px 以内なら「末尾にいる」とみなし、追記に合わせて自動で scroll する。 */
export const FOLLOW_THRESHOLD_PX = 24;

export function isNearBottom(el: { scrollTop: number; scrollHeight: number; clientHeight: number }): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight <= FOLLOW_THRESHOLD_PX;
}

/** 秒数を「3 分 5 秒」の形に。負や非数は undefined。 */
export function formatSeconds(totalSeconds: number): string | undefined {
  if (!Number.isFinite(totalSeconds) || totalSeconds < 0) return undefined;
  const s = Math.round(totalSeconds);
  if (s < 60) return `${s} 秒`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} 分 ${s % 60} 秒`;
  return `${Math.floor(m / 60)} 時間 ${m % 60} 分`;
}

/** 所要時間。終わった run は started_at〜finished_at、実行中は now までの経過（「経過」と分かる文言を付ける）。 */
export function runDuration(run: Pick<RunSummary, "started_at" | "finished_at">, nowMs: number): string | undefined {
  const start = Date.parse(run.started_at);
  if (Number.isNaN(start)) return undefined;
  if (run.finished_at) {
    const end = Date.parse(run.finished_at);
    return Number.isNaN(end) ? undefined : formatSeconds((end - start) / 1000);
  }
  const elapsed = formatSeconds((nowMs - start) / 1000);
  return elapsed ? `${elapsed}（経過）` : undefined;
}

/** header の StatusBadge に渡す状態語。終わった run は outcome、終わっていなければ running、分からなければ undefined。 */
export function runStatus(run: Pick<RunSummary, "finished_at" | "outcome">): string | undefined {
  if (!run.finished_at) return "running";
  return run.outcome ?? undefined;
}

/** harness の表示（adapter と、あれば model）。 */
export function runHarness(run: Pick<RunSummary, "adapter" | "model">): string {
  return run.model ? `${run.adapter}（${run.model}）` : run.adapter;
}

/** event の種類の可視ラベル。色だけで区別しないため、どの event にもこの文字を出す。 */
export function eventKindLabel(event: RunLogEvent): string {
  switch (event.kind) {
    case "message":
      return event.role === "assistant" ? "agent" : "user";
    case "thinking":
      return "思考";
    case "tool":
      return "tool";
    case "command":
      return "コマンド";
    case "file_change":
      return "変更";
    case "error":
      return "エラー";
    case "usage":
      return "集計";
    case "system":
      return "system";
    default:
      return "未分類";
  }
}
