import { data } from "react-router";
import type { ActionError } from "./action-types";
import type { CelerisClient } from "./client.server";
import { toActionError } from "./errors";
import { formString } from "./forms";
import type { CronJobList, CronJobRunList, CronJobView, CronRunResult } from "./types";

export type CronIntent = "pause" | "resume" | "run";
export type CronActionResult =
  | { ok: true; intent: CronIntent; jobId: string; result: CronJobView | CronRunResult }
  | { ok: false; intent: CronIntent; jobId: string; error: ActionError };

export function cronJobPath(id: string): string {
  return `/cron-jobs/${encodeURIComponent(id)}`;
}

export async function loadCronJobs(client: CelerisClient, signal?: AbortSignal): Promise<CronJobList> {
  return client.get<CronJobList>("/cron-jobs", { signal });
}

export async function loadCronJobDetail(
  client: CelerisClient,
  id: string,
  signal?: AbortSignal,
): Promise<{ job: CronJobView; history: CronJobRunList }> {
  const path = cronJobPath(id);
  const [job, history] = await Promise.all([
    client.get<CronJobView>(path, { signal }),
    client.get<CronJobRunList>(`${path}/runs`, { signal }),
  ]);
  return { job, history };
}

export async function applyCronAction(
  client: CelerisClient,
  id: string,
  intent: CronIntent,
  signal?: AbortSignal,
): Promise<CronActionResult> {
  try {
    const result = await client.post<CronJobView | CronRunResult>(`${cronJobPath(id)}/${intent}`, {}, { signal });
    return { ok: true, intent, jobId: id, result };
  } catch (error) {
    return { ok: false, intent, jobId: id, error: toActionError(error) };
  }
}

export async function handleCronForm(client: CelerisClient, request: Request, routeId?: string) {
  const form = await request.formData();
  const id = routeId ?? formString(form, "id");
  const intent = form.get("intent");
  if (!id || (intent !== "pause" && intent !== "resume" && intent !== "run")) {
    throw data({ error: "invalid cron action" }, { status: 400 });
  }
  const result = await applyCronAction(client, id, intent, request.signal);
  return data(result, { status: result.ok ? 200 : result.error.status });
}
