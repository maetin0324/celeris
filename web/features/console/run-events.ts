import { apiGet } from "../../api/client";
import type { EventRow, EventsPage } from "../../api/generated/types";

// Console の progress の「すべて見る」（R27）。`GET /api/tasks/:id/runs/:runId/events` を after_seq で最後まで読む。

const PAGE = 200;
const MAX_PAGES = 100;

export type RunEventLine = { seq: number; at: string; label: string; error: boolean };

export function formatRunEvent(row: EventRow): RunEventLine {
  const e = row.event;
  if (e.type === "worker_progress") {
    const kind = e.kind ?? "status";
    const text = e.summary ?? e.msg;
    const label = kind === "tool_use" ? `tool: ${e.tool ?? "?"}${text ? ` — ${text}` : ""}` : text;
    return { seq: row.seq, at: row.ts, label, error: e.error ?? false };
  }
  if (e.type === "worker_started")
    return { seq: row.seq, at: row.ts, label: `run 開始（${e.adapter} / ${e.model}）`, error: false };
  if (e.type === "worker_finished")
    return { seq: row.seq, at: row.ts, label: `run 終了（${e.outcome}）`, error: false };
  return { seq: row.seq, at: row.ts, label: e.type, error: false };
}

export async function fetchRunEvents(taskId: string, runId: string, signal?: AbortSignal): Promise<RunEventLine[]> {
  const base = `/api/tasks/${encodeURIComponent(taskId)}/runs/${encodeURIComponent(runId)}/events`;
  const lines: RunEventLine[] = [];
  let after = 0;
  for (let i = 0; i < MAX_PAGES; i++) {
    const page = await apiGet<EventsPage>(`${base}?after_seq=${after}&limit=${PAGE}`, signal);
    for (const row of page.items) lines.push(formatRunEvent(row));
    const last = page.items.at(-1);
    if (!page.has_more || !last || last.seq <= after) break;
    after = last.seq;
  }
  return lines;
}
