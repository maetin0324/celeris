import { useState } from "react";
import { Form, Link, useFetcher, useSearchParams } from "react-router";
import type { TaskCommentOutcome, TaskReopenOutcome } from "~/celeris/action-types";
import type { CommentList, Event, EventsPage, TaskComment, TaskDetail, Timeline, TimelineItem } from "~/celeris/types";
/* ADR-0048 D2・フェーズ 74: worker_progress の折り畳みの中身は Console と同じ行を再利用する。 */
import { ReplyStepRow } from "~/components/ConsoleBlockItem";
import { TaskCommentFlash, TaskReopenFlash } from "~/components/Flash";
import { LocalTime } from "~/components/LocalTime";
import { Badge } from "~/components/ui/badge";
import { Button, buttonClass } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import {
  checkboxClass,
  chipLabelClass,
  hintClass,
  labelClass,
  textareaClass,
  touchLinkClass,
} from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { EmptyState, Mono } from "~/components/ui/misc";
import type { Tone } from "~/components/ui/tone";
import { docsHref } from "~/lib/docs";
import { splitOutcome } from "~/lib/format";
import { isKnowledgeFallback } from "~/lib/knowledge";
import { commentAuthorLabel, timelineKindLabel } from "~/lib/labels";
import {
  groupTimelineWorkerProgress,
  type TimelineWorkerProgressItem,
  timelineProgressGroupSummary,
  workerProgressStep,
} from "~/lib/task-timeline";
import { cn } from "~/lib/utils";
import { ACTION_LABELS } from "./labels";

/**
 * `docs/celeris-api-v1.md` §3.6 の `types` フィルタの選択肢。`Event` の `type` タグと同じ。
 */
const EVENT_TYPES: Event["type"][] = [
  "browser_updated",
  "created",
  "transitioned",
  "worker_started",
  "worker_progress",
  "artifact_produced",
  "worker_finished",
  "review_verdict",
  "approval_requested",
  "approval_decided",
  "answered",
  "provider_throttled",
  // ADR-0044 D1（Phase 53）: 人がタスクを編集したときの記録。
  "edited",
];

/** タイムラインの 1 件の色（ADR-0044 D5）。 */
const TIMELINE_TONE: Record<string, Tone> = {
  event: "neutral",
  comment: "info",
  approval: "warning",
  report: "teal",
  delegation: "primary",
  release: "success",
  integration: "neutral",
  doc: "teal",
  // ADR-0047 D4/D5（Phase 62）。
  knowledge: "primary",
};

/**
 * タイムラインタブ（ADR-0044 D5）。`GET /tasks/{id}/timeline` を**古い順に**並べ、一番下にコメント欄を置く
 * （D2: 人のコメントは担当をすぐ起こす）。終端のタスクで `actions` に `reopen` があれば「再開」も出す。
 * 生のイベント（`GET /tasks/{id}/events` の `types` 絞り込み）は従来どおり下に畳んで残す。
 */
export function TimelineTab({
  taskId,
  detail,
  timeline,
  comments,
  events,
  fetchedAt,
}: {
  taskId: string;
  detail: TaskDetail;
  timeline: Timeline;
  comments: CommentList;
  events: EventsPage;
  fetchedAt: string;
}) {
  const [searchParams] = useSearchParams();
  const selectedTypes = new Set(searchParams.getAll("types"));
  const commentFetcher = useFetcher<TaskCommentOutcome>({ key: `task-comment-${taskId}` });
  const commenting = commentFetcher.state !== "idle";
  const reopenFetcher = useFetcher<TaskReopenOutcome>({ key: `task-reopen-${taskId}` });
  const reopening = reopenFetcher.state !== "idle";

  return (
    <section aria-labelledby="timeline-heading" data-testid="timeline-section" className="space-y-4">
      <Card>
        <CardHeader
          icon="activity"
          tone="info"
          title={
            <h2 id="timeline-heading" className="text-[0.95rem] font-semibold text-fg">
              タイムライン
            </h2>
          }
          description="このタスクに起きたことを時刻の順に 1 本にまとめたものです（できごと・コメント・認可・報告・委譲・リリース）。"
        />
        <CardBody className="space-y-4">
          {timeline.items.length === 0 ? (
            <EmptyState icon="activity" title="まだ何も起きていません。" />
          ) : (
            <ol className="space-y-2" data-testid="timeline-list">
              {groupTimelineWorkerProgress(timeline.items).map((display) =>
                display.kind === "progress_group" ? (
                  <TimelineProgressGroupRow
                    key={`progress-${display.items[0].seq}`}
                    items={display.items}
                    fetchedAt={fetchedAt}
                  />
                ) : (
                  <TimelineRow
                    key={timelineItemKey(display.item)}
                    taskId={taskId}
                    item={display.item}
                    fetchedAt={fetchedAt}
                  />
                ),
              )}
            </ol>
          )}

          {/* ADR-0044 D2: コメント欄はタイムラインの下。人のコメントは担当をすぐ起こす。 */}
          <div className="rounded-xl border border-border bg-surface-2/40 p-3" data-testid="comment-box">
            <TaskCommentFlash outcome={commentFetcher.data} />
            <commentFetcher.Form method="post" className="space-y-2">
              <input type="hidden" name="intent" value="comment" />
              <label htmlFor="task-comment-body" className={labelClass}>
                コメント（{comments.items.length} 件）
              </label>
              <textarea
                id="task-comment-body"
                name="body"
                rows={3}
                placeholder="例: 先に関連研究を 3 本だけ読んでから進めてください。"
                data-testid="comment-input"
                className={cn(textareaClass, "w-full")}
              />
              <p className={hintClass}>
                作業中のタスクにコメントすると、走っている run を止めて待機中に戻します（続きは同じ worktree
                から始まります）。質問待ちのときは回答として渡されます。
              </p>
              <Button type="submit" variant="primary" size="sm" disabled={commenting} data-testid="comment-submit">
                <Icon name="send" />
                コメントする
              </Button>
            </commentFetcher.Form>
          </div>

          {/* ADR-0044 D2: 終端（done / failed）のタスクは同じ worktree のまま再開できる（cancelled は不可）。 */}
          {detail.actions.includes("reopen") && (
            <div className="rounded-xl border border-border p-3" data-testid="reopen-box">
              <TaskReopenFlash outcome={reopenFetcher.data} />
              <reopenFetcher.Form method="post" className="flex flex-wrap items-center gap-3">
                <input type="hidden" name="intent" value="reopen" />
                <input type="hidden" name="expected_status" value={detail.task.status} />
                <p className="text-sm text-fg-muted">
                  このタスクは終わっています。同じ作業ディレクトリのまま続きをやらせられます。
                </p>
                <Button type="submit" variant="primary" size="sm" disabled={reopening} data-testid="action-reopen">
                  <Icon name="rotate" />
                  {ACTION_LABELS.reopen}
                </Button>
              </reopenFetcher.Form>
            </div>
          )}
        </CardBody>
      </Card>

      {/* 生のイベント（従来の `GET /tasks/{id}/events` の絞り込み）。裏方の確認用に畳んで残す。 */}
      <Card>
        <CardBody>
          <details data-testid="raw-events">
            <summary className="cursor-pointer select-none text-sm font-medium text-fg">
              生のイベント（絞り込み）
            </summary>
            <div className="mt-3 space-y-4">
              <Form method="get" className="flex flex-wrap items-center gap-2" data-testid="timeline-filter-form">
                <input type="hidden" name="tab" value="timeline" />
                {EVENT_TYPES.map((type) => (
                  <label key={type} className={chipLabelClass}>
                    <input
                      type="checkbox"
                      name="types"
                      value={type}
                      defaultChecked={selectedTypes.has(type)}
                      className={checkboxClass}
                    />
                    {type}
                  </label>
                ))}
                <button type="submit" className={buttonClass({ variant: "secondary", size: "sm" })}>
                  絞り込み
                </button>
              </Form>
              {events.items.length === 0 ? (
                <EmptyState icon="activity" title="ありません。" />
              ) : (
                <ul className="space-y-1.5">
                  {events.items.map((row) => (
                    <li
                      key={row.id}
                      data-testid="event-item"
                      data-event-type={row.event.type}
                      className="rounded-lg border border-border px-3 py-2 text-sm"
                    >
                      {row.event.type === "worker_progress" ? (
                        <details>
                          <summary className="flex min-h-11 cursor-pointer flex-wrap items-center gap-2">
                            <Mono>#{row.seq}</Mono>
                            <span className="text-sm text-fg-subtle lg:text-xs">{row.ts}</span>
                            <Badge tone="neutral">{row.event.type}</Badge>
                          </summary>
                          <p className="mt-1.5 text-fg-muted">{row.event.msg}</p>
                        </details>
                      ) : (
                        <p className="flex flex-wrap items-center gap-2">
                          <Mono>#{row.seq}</Mono>
                          <span className="text-sm text-fg-subtle lg:text-xs">{row.ts}</span>
                          <Badge tone="neutral">{row.event.type}</Badge>
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
              {events.has_more && <p className="text-sm text-fg-subtle lg:text-xs">続きがあります（has_more）。</p>}
            </div>
          </details>
        </CardBody>
      </Card>
    </section>
  );
}

/**
 * タイムラインの 1 件の安定した key（`kind` ごとに celeris が持つ一意の値を使う。添字は使わない:
 * SSE の再検証で先頭に項目が増えると並びがずれるため）。
 */
function timelineItemKey(item: TimelineItem): string {
  switch (item.kind) {
    case "event":
      return `event-${item.seq}`;
    case "comment":
      return `comment-${item.comment.id}`;
    case "approval":
      return `approval-${item.approval.id}`;
    case "report":
      return `report-${item.report.id}`;
    case "delegation":
      return `delegation-${item.run_id}`;
    case "release":
      return `release-${item.sha12}`;
    case "doc":
      return `doc-${item.path}`;
    case "integration":
      return `integration-${item.at}-${item.action}`;
    case "knowledge":
      return `knowledge-${item.run_task_id}`;
  }
}

/** タイムラインの 1 件（ADR-0044 D5）。`kind` ごとに出し分ける（知らない `kind` は無視する）。 */
function TimelineRow({ taskId, item, fetchedAt }: { taskId: string; item: TimelineItem; fetchedAt: string }) {
  return (
    <li
      data-testid="timeline-item"
      data-timeline-kind={item.kind}
      className="rounded-lg border border-border px-3 py-2 text-sm"
    >
      <p className="flex flex-wrap items-center gap-2">
        {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。
            フェーズ 74（ADR-0055 D2 ラウンド 6）: 生の ISO のままだと 393px には長すぎるので、他画面
            （`/approvals` 等）と同じ `LocalTime`（`~/lib/reports.ts::relativeTimeLabel`）に揃え、
            絶対時刻は `title`/`dateTime` に残す。 */}
        <LocalTime iso={item.at} fetchedAtIso={fetchedAt} className="text-sm text-fg-subtle lg:text-xs" />
        <Badge tone={TIMELINE_TONE[item.kind] ?? "neutral"}>{timelineKindLabel(item.kind)}</Badge>
        {item.kind === "event" && <Mono>{item.event.type}</Mono>}
      </p>
      <TimelineBody taskId={taskId} item={item} />
    </li>
  );
}

/**
 * `worker_progress` イベントが連続した区間 1 つ（ADR-0048 D2、フェーズ 74）。既定は折り畳み
 * （`aria-expanded` を持つ `button`。44px のタップ領域）で、見出しは「件数 ・ 最後の kind ・ 最後の時刻」。
 * 開くと Console の `ReplyStepRow` と同じ行（`~/lib/task-timeline.ts::workerProgressStep` で
 * `ConsoleReplyStep` の形に写す）が並び、両画面の見た目が揃う。
 */
function TimelineProgressGroupRow({ items, fetchedAt }: { items: TimelineWorkerProgressItem[]; fetchedAt: string }) {
  const [open, setOpen] = useState(false);
  const last = items[items.length - 1];
  return (
    <li
      data-testid="timeline-progress-group"
      data-progress-count={items.length}
      className="rounded-lg border border-border px-3 py-2 text-sm"
    >
      {/* 通常の TimelineRow の見出し（時刻・kind バッジ・event type）とそろえる（ADR-0055 D1-3: 状態は
          1 語のバッジ + 色。ここでは `kind` = "event" のバッジ、`worker_progress` は他の event 行と
          同じ `Mono` 表示にする）。 */}
      <p className="flex flex-wrap items-center gap-2">
        <LocalTime iso={last.at} fetchedAtIso={fetchedAt} className="text-sm text-fg-subtle lg:text-xs" />
        <Badge tone={TIMELINE_TONE.event ?? "neutral"}>{timelineKindLabel("event")}</Badge>
        <Mono>worker_progress</Mono>
      </p>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        data-testid="timeline-progress-group-toggle"
        className="mt-1.5 flex min-h-11 w-full items-center gap-2 text-left"
      >
        <Icon name={open ? "chevronDown" : "chevronRight"} className="size-3.5 shrink-0 text-fg-subtle" />
        {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
        <span className="min-w-0 flex-1 text-sm text-fg-subtle lg:text-xs">{timelineProgressGroupSummary(items)}</span>
      </button>
      {open && (
        <div className="mt-2 space-y-1 border-t border-border pt-2" data-testid="timeline-progress-group-detail">
          {items.map((item) => (
            <ReplyStepRow key={item.seq} step={workerProgressStep(item.event)} />
          ))}
        </div>
      )}
    </li>
  );
}

function TimelineBody({ taskId, item }: { taskId: string; item: TimelineItem }) {
  switch (item.kind) {
    case "event":
      return <TimelineEventBody event={item.event} />;
    case "comment":
      return <CommentBody comment={item.comment} />;
    case "approval":
      return (
        <div className="mt-1.5 text-fg-muted">
          <p className="whitespace-pre-wrap">{item.approval.question}</p>
          {item.approval.decision && (
            <p data-testid="timeline-approval-decision">
              決定: {item.approval.decision}
              {item.approval.answer ? `（${item.approval.answer}）` : ""}
            </p>
          )}
        </div>
      );
    case "report":
      return (
        <p className="mt-1.5 text-fg-muted">
          <Link to="/reports" className={cn(touchLinkClass, "font-medium text-primary hover:underline")}>
            {item.report.headline}
          </Link>
          {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
          <span className="ml-2 text-sm text-fg-subtle lg:text-xs">{item.report.kind}</span>
        </p>
      );
    case "delegation":
      return (
        <div className="mt-1.5 text-fg-muted">
          {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
          <p className="text-sm text-fg-subtle lg:text-xs">
            run{" "}
            <Link
              to={`/tasks/${taskId}/runs/${item.run_id}`}
              className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
            >
              {item.run_id}
            </Link>
          </p>
          <ul className="mt-1 space-y-0.5">
            {item.tasks.map((child) => (
              <li key={child.id}>
                <Link
                  to={`/tasks/${child.id}`}
                  className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                >
                  {child.title}
                </Link>
                <span className="text-fg-subtle">（{child.status}）</span>
              </li>
            ))}
          </ul>
        </div>
      );
    case "release":
      return (
        <p className="mt-1.5 text-fg-muted" data-testid="timeline-release">
          このタスクの変更はリリース{" "}
          <Link to="/releases" className={cn(touchLinkClass, "font-mono font-medium text-primary hover:underline")}>
            {item.sha12}
          </Link>{" "}
          に入りました（コミット {item.commits.length} 件）。
        </p>
      );
    // ADR-0043 D5 / A2（取り込み）。`action` は merge / pr / discard、`detail` は 1 行の説明。
    case "integration":
      return (
        <p className="mt-1.5 text-fg-muted">
          {item.action}: {item.detail}
        </p>
      );
    // ADR-0044 D7（Phase 57 / G20）: 逆リンク。front matter の `tasks:` にこのタスクを持つページ。
    case "doc":
      return (
        <p className="mt-1.5 text-fg-muted" data-testid="timeline-doc">
          <Link
            to={docsHref(item.project_id, { path: item.path })}
            className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
          >
            {item.title}
          </Link>{" "}
          <Mono className="text-xs">{item.path}</Mono>
        </p>
      );
    // ADR-0047 D4/D5（Phase 62）: このタスクの終端から起きた知識整理 run。
    case "knowledge": {
      const total = (item.ingested ?? 0) + (item.inbox ?? 0) + (item.discarded ?? 0);
      return (
        <p className="mt-1.5 text-fg-muted" data-testid="timeline-knowledge">
          {item.state === "scheduled" ? (
            "知識整理 run を起こしました（まだ適用されていません）。"
          ) : item.state === "failed" ? (
            "知識整理 run が失敗しました（候補はありません）。"
          ) : (
            <>
              知識 {total} 件: 取り込み {item.ingested ?? 0} / 候補 {item.inbox ?? 0} / 破棄 {item.discarded ?? 0}
              {(item.inbox ?? 0) > 0 && (
                <>
                  {" "}
                  <Link
                    to="/knowledge/inbox"
                    className={cn(touchLinkClass, "font-medium text-primary hover:underline")}
                  >
                    知識の候補を見る
                  </Link>
                </>
              )}
            </>
          )}
          {/* ADR-0052 D2（Phase 64）: Qwen に届かず tier cheap の汎用ハーネスで抽出した run。 */}
          {isKnowledgeFallback(item.via) && (
            <span className="ml-1 text-fg-subtle" data-testid="timeline-knowledge-fallback">
              （cheap のハーネスで抽出）
            </span>
          )}
        </p>
      );
    }
    default:
      return null;
  }
}

/** イベント 1 件の中身。人が読む意味のあるものだけ文にする（それ以外は `type` のバッジだけ）。 */
function TimelineEventBody({ event }: { event: Event }) {
  switch (event.type) {
    case "transitioned":
      return (
        <p className="mt-1.5 text-fg-muted">
          {event.from} → {event.to}（{event.reason}）
        </p>
      );
    case "worker_started":
      return (
        <p className="mt-1.5 text-fg-muted">
          run {event.run_id} 開始（{event.adapter} / {event.model}）
        </p>
      );
    case "worker_finished": {
      // `done: <長い要約>` はステータス名だけ本文に出し、要約は折り畳みに分ける。
      const { status, text } = splitOutcome(event.outcome);
      return (
        <div className="mt-1.5 text-fg-muted">
          <p>
            run {event.run_id} 終了: {status}
          </p>
          {text && (
            <details data-testid="timeline-outcome-detail" className="mt-1 text-sm lg:text-xs">
              <summary className="cursor-pointer select-none">詳細</summary>
              <p className="mt-1 max-h-48 overflow-y-auto whitespace-pre-wrap break-words">{text}</p>
            </details>
          )}
        </div>
      );
    }
    case "worker_progress":
      return <p className="mt-1.5 text-fg-muted">{event.msg}</p>;
    case "artifact_produced":
      return <p className="mt-1.5 font-mono text-xs text-fg-muted">{event.artifact.name}</p>;
    case "review_verdict":
      return (
        <p className="mt-1.5 text-fg-muted">
          #{event.criterion_idx} {event.pass ? "pass" : "fail"} — {event.reason}
        </p>
      );
    case "answered":
      return <p className="mt-1.5 whitespace-pre-wrap text-fg-muted">{event.answer}</p>;
    // ADR-0044 D1: 人が編集したときの記録（何を変えたか）。
    case "edited":
      return (
        <p className="mt-1.5 text-fg-muted" data-testid="timeline-edited">
          {event.by} が変えた項目: {event.fields.join("・")}
        </p>
      );
    default:
      return null;
  }
}

/** コメント 1 件（ADR-0044 D2）。`dangerouslySetInnerHTML` は使わず、素のテキストとして出す。 */
function CommentBody({ comment }: { comment: TaskComment }) {
  return (
    <div className="mt-1.5" data-testid="comment-item" data-author-kind={comment.author_kind}>
      {/* ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。 */}
      <p className="text-sm font-medium text-fg-subtle lg:text-xs">
        {commentAuthorLabel(comment.author_kind)}
        {comment.author ? `（${comment.author}）` : ""}
      </p>
      <p className="mt-0.5 whitespace-pre-wrap text-fg">{comment.body}</p>
    </div>
  );
}
