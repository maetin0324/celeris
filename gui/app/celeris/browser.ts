import type { CelerisClient } from "./client.server";
import { CelerisError } from "./errors";
import type { BrowserRun, BrowserWait, BrowserWaitList, EventsPage } from "./types";

/** Dedicated lifecycle feed: generic progress and user-selected event filters are never trusted. */
export async function loadBrowserRuns(
  client: CelerisClient,
  taskId: string,
  signal: AbortSignal,
): Promise<BrowserRun[]> {
  const latest = new Map<string, BrowserRun>();
  let afterSeq = -1;
  // Bound history reads; incomplete or unavailable history must not expose stale live links.
  for (let page = 0; page < 20; page++) {
    let events: EventsPage;
    try {
      events = await client.get<EventsPage>(`/tasks/${taskId}/events`, {
        query: { types: "browser_updated", after_seq: afterSeq, limit: 5000 },
        signal,
      });
    } catch (error) {
      // N-1 servers predate this event. Only that explicit incompatibility means no capability.
      if (
        error instanceof CelerisError &&
        error.status === 400 &&
        error.detail === "unknown event type `browser_updated`"
      ) {
        return [];
      }
      throw error;
    }
    for (const row of events.items) {
      if (row.task_id !== taskId || row.seq <= afterSeq || row.event.type !== "browser_updated") continue;
      if (row.event.browser.task_id !== taskId) continue;
      latest.set(row.event.browser.run_id, row.event.browser);
    }
    if (!events.has_more) return [...latest.values()];
    const next = events.items.at(-1)?.seq;
    if (next === undefined || next <= afterSeq) throw new Error("Incomplete browser lifecycle history");
    afterSeq = next;
  }
  throw new Error("Browser lifecycle history exceeds page limit");
}

/** ADR-0080 D5: task の wait 一覧（非秘密）。この API を持たない N-1 の celeris（404/400）は空にする。 */
export async function loadTaskBrowserWaits(
  client: CelerisClient,
  taskId: string,
  signal: AbortSignal,
): Promise<BrowserWait[]> {
  try {
    const list = await client.get<BrowserWaitList>(`/tasks/${encodeURIComponent(taskId)}/browser/waits`, { signal });
    return list.items.filter((w) => w.task_id === taskId);
  } catch (e) {
    if (e instanceof CelerisError && (e.status === 404 || e.status === 400 || e.status === 405)) return [];
    throw e;
  }
}

/** 人の操作を待っている wait か。 */
export function isOpenBrowserWait(w: BrowserWait): boolean {
  return w.state === "pending";
}
