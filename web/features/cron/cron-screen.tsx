import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ChatMessage, CronJobRun, CronJobView, CronRunOutcome } from "../../api/generated/types";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { CodeBlock } from "../../components/ui/code-block";
import { DataList } from "../../components/ui/data-list";
import { formatAbsolute } from "../../lib/time";
import { cronDetailQuery, cronKey, cronListQuery, cronPath, REVIEW_THREAD_PAGE, reviewThreadQuery } from "./queries";

const outcomes: Record<CronRunOutcome, string> = {
  created: "タスクを作成",
  queued: "順番待ち",
  skipped_overlap: "実行中のため見送り",
  skipped_missed: "過去の予定を見送り",
  error: "起動に失敗",
};
function Time({ value }: { value?: string | null }) {
  return value ? <time dateTime={value}>{formatAbsolute(value)}</time> : <>予定なし</>;
}
function RunSummary({ run }: { run: CronJobRun }) {
  return (
    <div className="min-w-0 space-y-1 break-words">
      <Badge tone={run.outcome === "error" ? "danger" : "neutral"}>{outcomes[run.outcome] ?? "未確認"}</Badge>
      <p>
        <Time value={run.recorded_at} />
      </p>
      {run.detail && <p>{run.detail}</p>}
      {run.task_id && (
        <Link
          to="/tasks/$id"
          params={{ id: run.task_id }}
          className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
        >
          作成したタスクを開く
        </Link>
      )}
    </div>
  );
}
function Actions({ job, disabled }: { job: CronJobView; disabled: boolean }) {
  const action = useActionResult(cronKey);
  const result = action.results[job.id];
  const denied = result?.status === 403;
  function run(intent: "pause" | "resume" | "run") {
    void action.run([{ id: job.id, path: `${cronPath(job.id)}/${intent}`, body: {} }]);
  }
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-2">
        <Button disabled={disabled || action.pending || denied} onClick={() => run(job.enabled ? "pause" : "resume")}>
          {job.enabled ? "一時停止" : "再開"}
        </Button>
        <Button disabled={disabled || action.pending || denied} onClick={() => run("run")}>
          手動実行
        </Button>
      </div>
      {action.pending && <p role="status">操作の結果を確認しています</p>}
      {denied && <p role="alert">この操作を行う権限がありません。</p>}
      <ActionResultView result={result} />
    </div>
  );
}
function Job({ job, disabled, showTitle = true }: { job: CronJobView; disabled: boolean; showTitle?: boolean }) {
  return (
    <section className="min-w-0 space-y-3 border-b border-border py-4" aria-label={job.name}>
      {showTitle && (
        <h2 className="text-section font-semibold">
          <Link
            to="/cron/$id"
            params={{ id: job.id }}
            className="inline-flex min-h-11 min-w-11 items-center break-all text-primary underline"
          >
            {job.name}
          </Link>
        </h2>
      )}
      <Badge tone={job.enabled ? "success" : "neutral"}>{job.enabled ? "有効" : "停止中"}</Badge>
      <DataList
        items={[
          { label: "実行予定", value: `${job.schedule} · ${job.timezone}` },
          { label: "次回", value: <Time value={job.next_fire_at} /> },
          { label: "前回の結果", value: job.last_run ? <RunSummary run={job.last_run} /> : "実行履歴なし" },
          ...(job.review_thread_id ? [{ label: "見直しの記録", value: <ReviewThreadLink jobId={job.id} /> }] : []),
        ]}
      />
      <Actions job={job} disabled={disabled} />
    </section>
  );
}
// ADR 2026-10-28-cos-review-back-thread D6: the cos_review job runs in the review thread (裏 thread), which
// the home never shows. The cron screen links to it and shows its messages read only: no composer, no actions.
export const REVIEW_THREAD_ANCHOR = "review-thread";
function ReviewThreadLink({ jobId }: { jobId: string }) {
  return (
    <Link
      to="/cron/$id"
      params={{ id: jobId }}
      hash={REVIEW_THREAD_ANCHOR}
      className="inline-flex min-h-11 min-w-11 items-center text-primary underline"
    >
      見直しの記録を読む（読み取り専用）
    </Link>
  );
}
const speakers: Record<ChatMessage["role"], string> = {
  user: "見直しの依頼",
  assistant: "CoS",
  system: "システム",
};
const messageStates: Partial<Record<ChatMessage["state"], string>> = {
  queued: "順番待ち",
  running: "実行中",
  cancelled: "取り消し済み",
  interrupted: "中断",
  failed: "失敗",
};
/** The review thread's messages, newest first. Read only: there is no form to post with. */
export function ReviewThreadMessages({ messages }: { messages: ChatMessage[] }) {
  if (messages.length === 0) return <p>見直しの記録はまだありません。</p>;
  return (
    <ol className="space-y-3">
      {messages.map((message) => (
        <li
          key={message.id}
          className="min-w-0 space-y-1 break-words border-b border-border pb-3"
          data-role={message.role}
        >
          <p className="text-label text-muted-foreground">
            {speakers[message.role]} · <Time value={message.created_at} />
            {messageStates[message.state] && <> · {messageStates[message.state]}</>}
          </p>
          <Markdown source={message.text} />
        </li>
      ))}
    </ol>
  );
}
function ReviewThreadHistory({ threadId }: { threadId: string }) {
  const query = useQuery(reviewThreadQuery(threadId));
  return (
    <section id={REVIEW_THREAD_ANCHOR} aria-label="見直しの記録" className="space-y-3">
      <h2 className="text-section font-semibold">見直しの記録（読み取り専用）</h2>
      <p className="text-label text-muted-foreground">
        統括 CoS が人の会話とは別の裏の会話で行った定期見直しの入力と返事です。新しい順に最大 {REVIEW_THREAD_PAGE}{" "}
        件。人に伝える事があった回だけ、CoS がホームの会話に報告します。ここからは書き込めません。
      </p>
      <FetchFrame query={query}>{query.data && <ReviewThreadMessages messages={query.data} />}</FetchFrame>
    </section>
  );
}
export function CronScreen() {
  const query = useQuery(cronListQuery);
  return (
    <ScreenFrame title="定期実行" route="/cron" description="登録された仕事の予定と実行結果を確認できます。">
      <FetchFrame query={query} empty={query.data?.items.length === 0} emptyMessage="定期実行はありません。">
        {query.data?.items.map((job) => (
          <Job key={job.id} job={job} disabled={query.isError} />
        ))}
      </FetchFrame>
    </ScreenFrame>
  );
}
export function CronDetailScreen({ id }: { id: string }) {
  const query = useQuery(cronDetailQuery(id));
  return (
    <ScreenFrame
      title={query.data?.job.name ?? "定期実行の詳細"}
      route="/cron/$id"
      breadcrumb={[{ label: "定期実行", link: { to: "/cron" } }, { label: "詳細" }]}
    >
      <FetchFrame query={query}>
        {query.data && (
          <div className="min-w-0 space-y-4">
            <Job job={query.data.job} disabled={query.isError} showTitle={false} />
            <details>
              <summary className="flex min-h-11 cursor-pointer items-center">仕事の雛形</summary>
              <CodeBlock label="仕事の雛形" wrap>
                {JSON.stringify(query.data.job.template, null, 2)}
              </CodeBlock>
            </details>
            {query.data.job.review_thread_id && <ReviewThreadHistory threadId={query.data.job.review_thread_id} />}
            <section aria-label="実行履歴" className="space-y-3">
              <h2 className="text-section font-semibold">実行履歴</h2>
              <p className="text-label text-muted-foreground">新しい順に最大 50 件</p>
              {query.data.history.items.length === 0 ? (
                <p>実行履歴はありません。</p>
              ) : (
                <ol className="space-y-3">
                  {query.data.history.items.map((run) => (
                    <li key={run.id} className="border-b border-border pb-3">
                      <p>
                        予定時刻: <Time value={run.scheduled_for} /> ·{" "}
                        {run.trigger === "manual"
                          ? "手動実行"
                          : run.trigger === "catch_up"
                            ? "過去の予定を実行"
                            : "定期実行"}
                      </p>
                      <RunSummary run={run} />
                    </li>
                  ))}
                </ol>
              )}
            </section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
