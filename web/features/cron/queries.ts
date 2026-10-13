import { apiGet } from "../../api/client";
import type { CronJobList, CronJobRunList, CronJobView } from "../../api/generated/types";

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
