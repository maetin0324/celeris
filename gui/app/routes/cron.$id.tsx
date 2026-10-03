import { Link, useFetcher } from "react-router";
import { getCelerisClient } from "~/celeris/client.server";
import { type CronActionResult, handleCronForm, loadCronJobDetail } from "~/celeris/cron.server";
import { celerisErrorResponse } from "~/celeris/errors";
import { CronActionFeedback, CronActions, CronErrorBoundary } from "~/components/CronJobs";
import { LocalTime } from "~/components/LocalTime";
import { Badge } from "~/components/ui/badge";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { EmptyState, Mono, PageHeader } from "~/components/ui/misc";
import { revalidateAfterActionErrors } from "~/lib/revalidate";
import type { Route } from "./+types/cron.$id";

export const shouldRevalidate = revalidateAfterActionErrors;

export async function loader({ request, params }: Route.LoaderArgs) {
  try {
    return await loadCronJobDetail(getCelerisClient(), params.id, request.signal);
  } catch (error) {
    throw celerisErrorResponse(error);
  }
}

export async function action({ request, params }: Route.ActionArgs) {
  return handleCronForm(getCelerisClient(), request, params.id);
}

export function meta(_: Route.MetaArgs) {
  return [{ title: "定期実行の履歴 - Celeris" }];
}

export default function CronDetailPage({ loaderData }: Route.ComponentProps) {
  const { job, history } = loaderData;
  const fetcher = useFetcher<CronActionResult>();
  return (
    <div className="space-y-6" data-testid="cron-detail">
      <Link to="/cron" className="inline-flex min-h-11 items-center text-primary hover:underline">
        ← 定期実行一覧
      </Link>
      <PageHeader icon="activity" title={job.name} description={`${job.schedule} · ${job.timezone}`} />
      <CronActionFeedback result={fetcher.data} />
      <CronActions job={job} fetcher={fetcher} />
      <Card>
        <CardHeader title="job の雛形" />
        <CardBody>
          <pre className="overflow-x-auto whitespace-pre-wrap break-words text-sm">
            {JSON.stringify(job.template, null, 2)}
          </pre>
        </CardBody>
      </Card>
      <Card>
        <CardHeader title="実行履歴" description="新しい順に最大 50 件" />
        <CardBody>
          {history.items.length === 0 ? (
            <EmptyState icon="activity" title="実行履歴はありません" />
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-left text-sm" data-testid="cron-runs">
                <thead>
                  <tr className="border-b border-border">
                    <th className="p-2">予定時刻</th>
                    <th className="p-2">起動</th>
                    <th className="p-2">結果</th>
                    <th className="p-2">task</th>
                  </tr>
                </thead>
                <tbody>
                  {history.items.map((run) => (
                    <tr key={run.id} className="border-b border-border">
                      <td className="p-2">
                        <LocalTime iso={run.scheduled_for} mode="datetime" />
                      </td>
                      <td className="p-2">{run.trigger}</td>
                      <td className="p-2">
                        <Badge
                          tone={run.outcome === "created" ? "success" : run.outcome === "error" ? "danger" : "warning"}
                        >
                          {run.outcome}
                        </Badge>
                        {run.detail && <p className="mt-1 text-fg-muted">{run.detail}</p>}
                      </td>
                      <td className="p-2">
                        {run.task_id ? (
                          <Link
                            className="text-primary hover:underline"
                            to={`/tasks/${encodeURIComponent(run.task_id)}`}
                          >
                            <Mono>{run.task_id}</Mono>
                          </Link>
                        ) : (
                          "—"
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </CardBody>
      </Card>
    </div>
  );
}

export function ErrorBoundary({ error }: Route.ErrorBoundaryProps) {
  return <CronErrorBoundary error={error} />;
}
