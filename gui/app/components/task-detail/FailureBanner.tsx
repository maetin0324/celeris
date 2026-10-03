import type { useFetcher } from "react-router";
import type { RetryOutcome, TaskCommentOutcome, TaskRereviewOutcome } from "~/celeris/action-types";
import type { Action, TaskDetail } from "~/celeris/types";
import { ErrorFlash, RetryFlash, TaskRereviewFlash } from "~/components/Flash";
import { Button } from "~/components/ui/button";
import { Icon } from "~/components/ui/Icon";
import { Alert } from "~/components/ui/misc";

const FAILURE_CLASS_LABEL: Record<"infra" | "work", string> = {
  infra: "インフラ",
  work: "作業内容",
};

/**
 * ADR-0070 D1/D2（Phase 116）: failed のタスク詳細に出す赤いバナー。「失敗: <分類> — <理由>」に、
 * 成果が main に取り込み済みなら一言添え、「やり直す」（常に。`retry`）「再レビュー」（`actions` に `rereview` が
 * あるときだけ）「取り下げ」（状態は変えず、対応不要と記録するコメントを残すだけ。ADR-0070 D2）を出す。
 * ヘルプ画面（`gui/app/routes/help.tsx`、`/help`）の「失敗したタスクの直し方」と同じ言葉づかい。
 */
export function FailureBanner({
  taskId,
  failure,
  actions,
  retryFetcher,
  retrying,
  rereviewFetcher,
  rereviewing,
  dismissFetcher,
  dismissing,
}: {
  taskId: string;
  failure: NonNullable<TaskDetail["failure"]>;
  actions: Action[];
  retryFetcher: ReturnType<typeof useFetcher<RetryOutcome>>;
  retrying: boolean;
  rereviewFetcher: ReturnType<typeof useFetcher<TaskRereviewOutcome>>;
  rereviewing: boolean;
  dismissFetcher: ReturnType<typeof useFetcher<TaskCommentOutcome>>;
  dismissing: boolean;
}) {
  return (
    <section aria-labelledby="failure-banner-heading" data-testid="failure-banner">
      <Alert tone="danger" icon="alert" className="rounded-2xl border-2 p-5 shadow-sm">
        <p id="failure-banner-heading" className="text-base font-semibold" data-testid="failure-banner-summary">
          失敗: {FAILURE_CLASS_LABEL[failure.class]} — {failure.reason}
        </p>
        {failure.delivered_release && (
          <p data-testid="failure-banner-delivered">
            成果は main に取り込み済み（release {failure.delivered_release}）だがレビューで不合格。
          </p>
        )}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          {actions.includes("retry") && (
            <retryFetcher.Form method="post" action={`/tasks/${taskId}`}>
              <input type="hidden" name="intent" value="retry" />
              <Button type="submit" variant="primary" size="sm" disabled={retrying} data-testid="failure-banner-retry">
                <Icon name="rotate" />
                やり直す
              </Button>
            </retryFetcher.Form>
          )}
          {actions.includes("rereview") && (
            <rereviewFetcher.Form method="post" action={`/tasks/${taskId}`}>
              <input type="hidden" name="intent" value="rereview" />
              <Button
                type="submit"
                variant="secondary"
                size="sm"
                disabled={rereviewing}
                data-testid="failure-banner-rereview"
              >
                <Icon name="check" />
                再レビュー
              </Button>
            </rereviewFetcher.Form>
          )}
          <dismissFetcher.Form method="post" action={`/tasks/${taskId}`}>
            <input type="hidden" name="intent" value="comment" />
            <input type="hidden" name="body" value="取り下げ: 対応不要と判断しました（状態は failed のまま）。" />
            <Button type="submit" variant="ghost" size="sm" disabled={dismissing} data-testid="failure-banner-dismiss">
              <Icon name="x" />
              取り下げ
            </Button>
          </dismissFetcher.Form>
        </div>
        <RetryFlash outcome={retryFetcher.data} />
        <TaskRereviewFlash outcome={rereviewFetcher.data} />
        {dismissFetcher.data && !dismissFetcher.data.ok && <ErrorFlash error={dismissFetcher.data.error} />}
        {dismissFetcher.data?.ok && (
          <p className="mt-1 text-sm text-fg-muted" data-testid="failure-banner-dismissed">
            取り下げのコメントを記録しました。
          </p>
        )}
      </Alert>
    </section>
  );
}
