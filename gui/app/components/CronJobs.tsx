import { type FetcherWithComponents, isRouteErrorResponse, Link } from "react-router";
import type { CronActionResult } from "~/celeris/cron.server";
import type { CelerisRouteErrorData } from "~/celeris/errors";
import type { CronJobRun, CronJobView } from "~/celeris/types";
import { LocalTime } from "~/components/LocalTime";
import { RouteRecovery } from "~/components/RouteRecovery";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { Alert, DataItem, Mono } from "~/components/ui/misc";
import { isTransientStatus } from "~/lib/recovery";
import { CelerisBanner } from "~/root";

export function CronActionFeedback({ result }: { result: CronActionResult | undefined }) {
  if (!result) return null;
  if (!result.ok) {
    return (
      <Alert tone="danger" title="操作できませんでした">
        {result.error.detail}
      </Alert>
    );
  }
  const message =
    result.intent === "pause"
      ? "一時停止しました"
      : result.intent === "resume"
        ? "再開しました"
        : "手動実行を受け付けました";
  return (
    <Alert tone="success" title={message}>
      {result.intent === "run" ? "結果と履歴を確認してください。" : null}
    </Alert>
  );
}

export function CronActions({ job, fetcher }: { job: CronJobView; fetcher: FetcherWithComponents<CronActionResult> }) {
  const busy = fetcher.state !== "idle";
  return (
    <div className="flex flex-wrap gap-2">
      <fetcher.Form method="post">
        <input type="hidden" name="id" value={job.id} />
        <input type="hidden" name="intent" value={job.enabled ? "pause" : "resume"} />
        <Button type="submit" size="sm" disabled={busy} data-testid="cron-toggle">
          {job.enabled ? "一時停止" : "再開"}
        </Button>
      </fetcher.Form>
      <fetcher.Form method="post">
        <input type="hidden" name="id" value={job.id} />
        <input type="hidden" name="intent" value="run" />
        <Button type="submit" size="sm" variant="primary" disabled={busy} data-testid="cron-run">
          手動実行
        </Button>
      </fetcher.Form>
    </div>
  );
}

export function CronJobCard({ job, fetcher }: { job: CronJobView; fetcher: FetcherWithComponents<CronActionResult> }) {
  return (
    <Card data-testid="cron-job" className="min-w-0">
      <CardHeader
        title={
          <Link to={`/cron/${encodeURIComponent(job.id)}`} className="break-all text-primary hover:underline">
            {job.name}
          </Link>
        }
        actions={<Badge tone={job.enabled ? "success" : "neutral"}>{job.enabled ? "有効" : "停止中"}</Badge>}
      />
      <CardBody className="space-y-4">
        <dl className="grid gap-3 text-sm sm:grid-cols-2">
          <DataItem label="schedule">
            <Mono>{job.schedule}</Mono>
          </DataItem>
          <DataItem label="タイムゾーン">{job.timezone}</DataItem>
          <DataItem label="次回">
            {job.next_fire_at ? <LocalTime iso={job.next_fire_at} mode="datetime" /> : "—"}
          </DataItem>
          <DataItem label="前回の結果">
            {job.last_run ? <CronRunSummary run={job.last_run} /> : "実行履歴なし"}
          </DataItem>
        </dl>
        <CronActions job={job} fetcher={fetcher} />
      </CardBody>
    </Card>
  );
}

export function CronRunSummary({ run }: { run: CronJobRun }) {
  return (
    <span className="flex flex-wrap items-center gap-2">
      <Badge tone={run.outcome === "created" ? "success" : run.outcome === "error" ? "danger" : "warning"}>
        {run.outcome}
      </Badge>
      <LocalTime iso={run.recorded_at} mode="datetime" />
      {run.task_id && (
        <Link to={`/tasks/${encodeURIComponent(run.task_id)}`} className="text-primary hover:underline">
          task {run.task_id}
        </Link>
      )}
    </span>
  );
}

export function CronErrorBoundary({ error }: { error: unknown }) {
  if (isRouteErrorResponse(error) && error.data && typeof error.data === "object" && "kind" in error.data) {
    const detail = error.data as CelerisRouteErrorData;
    if (detail.kind === "unavailable") {
      return (
        <main className="p-4">
          <CelerisBanner celerisApiUrl={detail.baseUrl ?? ""} problem={null} />
          <RouteRecovery />
        </main>
      );
    }
    return (
      <main className="p-4">
        <h1 className="text-xl font-semibold">エラー {detail.status}</h1>
        <p>{detail.detail}</p>
        {detail.status && isTransientStatus(detail.status) && <RouteRecovery />}
      </main>
    );
  }
  return (
    <main className="p-4">
      <h1 className="text-xl font-semibold">エラー</h1>
      <RouteRecovery />
    </main>
  );
}
