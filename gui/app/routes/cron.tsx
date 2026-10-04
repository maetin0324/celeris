import { useFetcher } from "react-router";
import { getCelerisClient } from "~/celeris/client.server";
import { type CronActionResult, handleCronForm, loadCronJobs } from "~/celeris/cron.server";
import { celerisErrorResponse } from "~/celeris/errors";
import { CronActionFeedback, CronErrorBoundary, CronJobCard } from "~/components/CronJobs";
import { EmptyState, PageHeader } from "~/components/ui/misc";
import { revalidateAfterActionErrors } from "~/lib/revalidate";
import type { Route } from "./+types/cron";

export const shouldRevalidate = revalidateAfterActionErrors;

export async function loader({ request }: Route.LoaderArgs) {
  try {
    return await loadCronJobs(getCelerisClient(), request.signal);
  } catch (error) {
    throw celerisErrorResponse(error);
  }
}

export async function action({ request }: Route.ActionArgs) {
  return handleCronForm(getCelerisClient(), request);
}

export function meta(_: Route.MetaArgs) {
  return [{ title: "定期実行 - Celeris" }];
}

export default function CronPage({ loaderData }: Route.ComponentProps) {
  const fetcher = useFetcher<CronActionResult>();
  return (
    <div className="space-y-6" data-testid="cron-page">
      <PageHeader icon="activity" title="定期実行" description="登録された job の予定と実行結果を確認できます。" />
      <CronActionFeedback result={fetcher.data} />
      {loaderData.items.length === 0 ? (
        <EmptyState icon="activity" title="定期実行はありません" />
      ) : (
        <div className="grid gap-4 xl:grid-cols-2">
          {loaderData.items.map((job) => (
            <CronJobCard key={job.id} job={job} fetcher={fetcher} />
          ))}
        </div>
      )}
    </div>
  );
}

export function ErrorBoundary({ error }: Route.ErrorBoundaryProps) {
  return <CronErrorBoundary error={error} />;
}
