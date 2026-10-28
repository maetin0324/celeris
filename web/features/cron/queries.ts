import { apiGet } from "../../api/client";
import type { CronJobList, CronJobRunList, CronJobView } from "../../api/generated/types";
import { listMessages } from "../chat/data/client";

export const cronKey = ["cron-jobs"] as const;
export const cronPath = (id: string) => `/api/cron-jobs/${encodeURIComponent(id)}`;
export const cronListQuery = {
  queryKey: [...cronKey, "list"],
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<CronJobList>("/api/cron-jobs", signal),
  refetchInterval: 30_000,
};
export function cronDetailQuery(id: string) {
  return {
    queryKey: [...cronKey, id],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const [job, history] = await Promise.all([
        apiGet<CronJobView>(cronPath(id), signal),
        apiGet<CronJobRunList>(`${cronPath(id)}/runs`, signal),
      ]);
      return { job, history };
    },
    refetchInterval: 30_000,
  };
}

/** ADR 2026-10-28-cos-review-back-thread D6: the review thread's latest messages, read only (newest first). */
export const REVIEW_THREAD_PAGE = 50;
export function reviewThreadQuery(threadId: string) {
  return {
    queryKey: [...cronKey, "review-thread", threadId],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      const page = await listMessages(threadId, { limit: REVIEW_THREAD_PAGE }, signal);
      return [...page.items].sort((a, b) => b.seq - a.seq);
    },
    refetchInterval: 30_000,
  };
}
