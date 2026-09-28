import { checkOwner } from "~/browser-owner.server";
import { activeBrowserRunIds, inAuthInterval, safeBrowserLiveUrl } from "~/lib/browser";
import { loadBrowserRuns, loadTaskBrowserWaits } from "./browser";
import type { CelerisClient } from "./client.server";
import type { BrowserRun, BrowserWait, TaskDetail } from "./types";

/**
 * ADR-0080 D6: `/browser/live/:taskId/:runId`。毎回 owner grant・task/run 対応・active・RUNNING・
 * 認証区間外を照合する。upstream の dashboard URL は応答・Location・ログに出さない。
 *
 * 固定版 dashboard の HTTP/WS/assets を読み取り専用で relay し、guard を通らない絶対 URL・token bootstrap・
 * 直接 WS 参照が残らないことはまだ確かめていない。ADR の「満たせない場合の安全な動作」に従い、guard を
 * 通った本人にも relay は開かず `live_view_relay_unavailable` を返す（リンク非表示だけで達成扱いにしない）。
 */
export { LIVE_VIEW_RELAY_AVAILABLE } from "~/lib/browser";

const ID_RE = /^[0-9A-Za-z_-]{1,64}$/;

function plain(status: number, code: string): Response {
  return new Response(`${code}\n`, {
    status,
    headers: {
      "Content-Type": "text/plain; charset=utf-8",
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

/** `GET /browser/live/:taskId/:runId`（他の method も同じ guard を通して拒否する）。 */
export async function runLiveViewRoute(
  client: CelerisClient,
  request: Request,
  taskId: string | undefined,
  runId: string | undefined,
): Promise<Response> {
  const owner = await checkOwner(request);
  if (!owner.ok) return plain(owner.status, owner.code);
  if (request.method.toUpperCase() !== "GET") return plain(405, "method_not_allowed");
  // WebSocket upgrade も同じ guard の後で拒否する（読み取り専用 relay が未対応）。
  if (request.headers.get("upgrade")) return plain(501, "live_view_relay_unavailable");
  if (!taskId || !runId || !ID_RE.test(taskId) || !ID_RE.test(runId)) return plain(404, "not_found");
  let detail: TaskDetail;
  let runs: BrowserRun[];
  let waits: BrowserWait[];
  try {
    [detail, runs, waits] = await Promise.all([
      client.get<TaskDetail>(`/tasks/${encodeURIComponent(taskId)}`, { signal: request.signal }),
      loadBrowserRuns(client, taskId, request.signal),
      loadTaskBrowserWaits(client, taskId, request.signal),
    ]);
  } catch {
    return plain(404, "not_found");
  }
  const run = runs.find((r) => r.run_id === runId && r.task_id === taskId);
  if (!run) return plain(404, "not_found");
  const active = activeBrowserRunIds(detail.runs, detail.task.status).includes(runId);
  if (run.state !== "RUNNING" || !active) return plain(409, "not_running");
  if (!safeBrowserLiveUrl(run.live_view_url)) return plain(404, "not_configured");
  if (inAuthInterval(waits, runId)) return plain(409, "auth_interval");
  // LIVE_VIEW_RELAY_AVAILABLE が false の間は、guard を通った本人にも relay を開かない。
  return plain(503, "live_view_relay_unavailable");
}
