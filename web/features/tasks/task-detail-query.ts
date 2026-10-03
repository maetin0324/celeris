import { apiGet } from "../../api/client";
import type { TaskDetail, Timeline } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";

// /tasks/:id の枠と overview・timeline の query（P3-08）。
// key は keys.ts の taskKeys.detail / taskKeys.timeline をそのまま使う（バッジ用と別の key を作らない）。
// 表示は fetch を待たない。loader は置かず、client の query で取得する（S1 遷移の独立）。

export function taskDetailPath(taskId: string): string {
  return `/api/tasks/${encodeURIComponent(taskId)}`;
}

export function taskTimelinePath(taskId: string): string {
  return `/api/tasks/${encodeURIComponent(taskId)}/timeline`;
}

export function taskDetailQueryKey(taskId: string) {
  return taskKeys.detail(taskId);
}

export function taskTimelineQueryKey(taskId: string) {
  return taskKeys.timeline(taskId);
}

export function taskDetailQuery(taskId: string) {
  return {
    queryKey: taskDetailQueryKey(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<TaskDetail>(taskDetailPath(taskId), signal),
  };
}

export function taskTimelineQuery(taskId: string) {
  return {
    queryKey: taskTimelineQueryKey(taskId),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Timeline>(taskTimelinePath(taskId), signal),
  };
}
