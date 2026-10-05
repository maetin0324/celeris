import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { EventsPage } from "../../api/generated/types";
import { browserRunBadge } from "./browser-model";
import { BrowserGatewayError, browserKeys } from "./browser-query";

// D3.1 live-events: Live View が無くても監視できるイベントの流れ。
// gateway に live `/read` の中継がまだ無いので、generic relay で読める task の `browser_updated`
// （relay と SSE が live_view_url を落とした scrub 済みの行）を、最後に見た seq（last_seen）から追って出す。

export type LiveEventItem = { seq: number; ts: string; text: string };

const MAX_ITEMS = 200;

/** この run の行だけを表示文にする。live_view_url などの値は文に入れない。 */
export function liveEventItems(page: Pick<EventsPage, "items">, taskId: string, runId: string): LiveEventItem[] {
  const out: LiveEventItem[] = [];
  for (const row of page.items) {
    if (row.task_id !== taskId || row.event.type !== "browser_updated") continue;
    const browser = (row.event as { browser?: { run_id?: string; state?: string } }).browser;
    if (!browser || browser.run_id !== runId) continue;
    const label = browserRunBadge({ state: (browser.state ?? "") as never });
    out.push({ seq: row.seq, ts: row.ts, text: `状態: ${label}` });
  }
  return out;
}

/** 新しい行を seq で重ねずに足し、古いものから捨てて上限に収める。 */
export function mergeLiveEvents(current: readonly LiveEventItem[], next: readonly LiveEventItem[]): LiveEventItem[] {
  const last = current.at(-1)?.seq ?? -1;
  const merged = [...current, ...next.filter((item) => item.seq > last)];
  return merged.slice(Math.max(0, merged.length - MAX_ITEMS));
}

export function LiveEvents({ taskId, runId }: { taskId: string; runId: string }) {
  const lastSeen = useRef(-1);
  const [items, setItems] = useState<LiveEventItem[]>([]);
  const page = useQuery({
    queryKey: [...browserKeys.all, "events", taskId, runId],
    queryFn: async ({ signal }) => {
      const path = `/api/tasks/${encodeURIComponent(taskId)}/events?types=browser_updated&after_seq=${lastSeen.current}&limit=200`;
      const response = await fetch(path, { credentials: "same-origin", cache: "no-store", signal });
      const value = (await response.json()) as EventsPage & { code?: string };
      if (!response.ok) throw new BrowserGatewayError(response.status, value.code ?? "request_failed");
      const last = value.items.at(-1)?.seq;
      if (typeof last === "number" && last > lastSeen.current) lastSeen.current = last;
      return liveEventItems(value, taskId, runId);
    },
    refetchInterval: 5000,
  });

  useEffect(() => {
    if (page.data && page.data.length > 0) setItems((current) => mergeLiveEvents(current, page.data));
  }, [page.data]);

  return (
    <section aria-labelledby="browser-live-events" className="flex flex-col gap-2" data-testid="browser-live-events">
      <h2 id="browser-live-events" tabIndex={-1} className="text-section font-semibold">
        イベント
      </h2>
      {page.isError ? (
        <p role="alert" className="text-label text-danger-foreground">
          イベントを取得できません。少し待つと再試行します。
        </p>
      ) : null}
      {items.length === 0 ? (
        <p className="text-label text-muted-foreground">
          {page.isPending ? "読み込んでいます。" : "まだイベントはありません。"}
        </p>
      ) : (
        <ol className="flex flex-col gap-1 text-label" aria-label="ブラウザのイベント（新しい順）">
          {[...items].reverse().map((item) => (
            <li key={item.seq} className="flex flex-wrap gap-x-3 border-b border-border py-1">
              <span className="font-mono text-muted-foreground">#{item.seq}</span>
              <span>{item.text}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
