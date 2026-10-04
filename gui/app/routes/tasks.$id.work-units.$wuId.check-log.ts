import { type CelerisClient, getCelerisClient } from "~/celeris/client.server";
import { celerisErrorResponse } from "~/celeris/errors";
import type { WorkUnitCheckLog } from "~/celeris/types";
import type { Route } from "./+types/tasks.$id.work-units.$wuId.check-log";

/**
 * `/tasks/:id/work-units/:wuId/check-log`（resource route。コンポーネントは持たない）。
 * `GET /tasks/{id}/work-units/{wu_id}/check-log`（docs/api/v1/gui-api.md §3.126.19）をそのまま返す。
 * タスク詳細の WU の行（`~/components/ExecutionSection.tsx`）が、統合の検査の出力の末尾を開いたときだけ
 * `useFetcher().load()` で取りに行く（`tasks/:id/runs/:runId/events` と同じ作り）。
 */
export async function loadWorkUnitCheckLog(
  client: CelerisClient,
  taskId: string,
  wuId: string,
  request: Request,
): Promise<WorkUnitCheckLog> {
  const url = new URL(request.url);
  return client.get<WorkUnitCheckLog>(
    `/tasks/${encodeURIComponent(taskId)}/work-units/${encodeURIComponent(wuId)}/check-log`,
    {
      query: {
        index: url.searchParams.get("index") ?? undefined,
        bytes: url.searchParams.get("bytes") ?? undefined,
      },
      signal: request.signal,
    },
  );
}

export async function loader({ params, request }: Route.LoaderArgs): Promise<WorkUnitCheckLog> {
  try {
    return await loadWorkUnitCheckLog(getCelerisClient(), params.id, params.wuId, request);
  } catch (e) {
    throw celerisErrorResponse(e);
  }
}
