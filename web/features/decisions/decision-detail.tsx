import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useLocation } from "@tanstack/react-router";
import { useEffect, useId, useState } from "react";
import { isApiError } from "../../api/client";
import type { DecisionList, DecisionOutcome, DecisionView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { formatAbsolute } from "../../lib/time";
import {
  allDecisionsKey,
  answerEventsQuery,
  answerHistory,
  byLabel,
  decisionAnchor,
  EFFECT_LABELS,
  FREE_TEXT_OPTION,
  type HistoryEntry,
  NOTE_MAX,
  optionLabel,
  type ReviseInput,
  revisability,
  reviseDecision,
  reviseFailure,
  reviseReady,
  reviseSummary,
  STATUS_LABELS,
  taskDecisionsQuery,
} from "./decision-model";

// task 詳細の「決定」区画（2026-10-09 人の指摘: 回答済みの決定を画面から直せない）。
// 一覧は `GET /tasks/{id}/decisions`（その task の subtree）。行を開くと詳細（今の答え・答えた人と日時・選択肢・
// 回答の履歴）と、回答済みの choice の決定だけ「答えを変える」（確認の段を挟んで `POST /decisions/{id}/revise`）。
// 受信箱・チャットのカードからは `/tasks/<出した task>#decision-<id>` で来て、その行が開いた状態になる。

const STATUS_TONES: Record<DecisionView["decision"]["status"], BadgeTone> = {
  open: "warning",
  answered: "success",
  withdrawn: "neutral",
};

export function TaskDecisionsPanel({ taskId }: { taskId: string }) {
  const list = useQuery(taskDecisionsQuery(taskId));
  const hash = useLocation({ select: (location) => location.hash.replace(/^#/, "") });
  if (list.isPending)
    return (
      <p role="status" aria-busy="true">
        決定を読み込み中…
      </p>
    );
  const items = list.data?.items ?? [];
  // 決定 API を持たない daemon も明示する。
  const unavailable = isApiError(list.error) && list.error.status === 404;
  return (
    <section
      id="task-decisions"
      aria-labelledby="task-decisions-title"
      data-testid="task-decisions"
      className="min-w-0 scroll-mt-4 space-y-3 rounded-lg border border-border bg-surface p-4"
    >
      <h2 id="task-decisions-title" className="text-section font-semibold text-foreground">
        決定
      </h2>
      {list.isError ? (
        <div role="alert" data-testid="task-decisions-error" className="text-label text-muted-foreground">
          <p>
            {unavailable
              ? "決定の一覧は利用できません。"
              : "決定の一覧を更新できませんでした。表示中の内容は古い可能性があります。"}
          </p>
          <Button variant="secondary" onClick={() => void list.refetch()}>
            再取得
          </Button>
        </div>
      ) : items.length === 0 ? (
        <p className="text-label text-muted-foreground">決定はありません。</p>
      ) : null}
      <ul className="flex min-w-0 flex-col gap-2">
        {items.map((view) => (
          <DecisionItem key={view.decision.id} view={view} focused={hash === decisionAnchor(view.decision.id)} />
        ))}
      </ul>
    </section>
  );
}

function DecisionItem({ view, focused }: { view: DecisionView; focused: boolean }) {
  const [open, setOpen] = useState(focused);
  useEffect(() => {
    if (!focused) return;
    setOpen(true);
    // 一覧は task 詳細より後に届くので、hash の移動先へはここで scroll する。
    const frame = requestAnimationFrame(() =>
      document.getElementById(decisionAnchor(view.decision.id))?.scrollIntoView({ block: "start" }),
    );
    return () => cancelAnimationFrame(frame);
  }, [focused, view.decision.id]);
  const events = useQuery({ ...answerEventsQuery(view.task_id), enabled: open });
  const history = answerHistory(view, events.data);
  const d = view.decision;
  return (
    <li id={decisionAnchor(d.id)} data-decision-id={d.id} className="min-w-0 scroll-mt-4">
      <details
        open={open}
        onToggle={(event) => setOpen(event.currentTarget.open)}
        className="min-w-0 rounded-md border border-border p-2"
      >
        <summary className="flex min-h-11 min-w-0 cursor-pointer flex-wrap items-center gap-2">
          <Badge tone={STATUS_TONES[d.status]}>{STATUS_LABELS[d.status]}</Badge>
          <span className="min-w-0 break-words font-medium">{d.question}</span>
          {d.answer ? (
            <span className="min-w-0 break-words text-label text-muted-foreground">
              → {optionLabel(d, d.answer.option)}
            </span>
          ) : null}
        </summary>
        {open ? (
          <>
            {events.isError ? (
              <div role="alert" className="text-label text-muted-foreground">
                <p>回答の履歴を更新できませんでした。履歴の一部が表示されていない可能性があります。</p>
                <Button variant="secondary" onClick={() => void events.refetch()}>
                  履歴を再取得
                </Button>
              </div>
            ) : null}
            <DecisionDetail view={view} history={history} historyLoading={events.isPending} />
          </>
        ) : null}
      </details>
    </li>
  );
}

export type DecisionDetailProps = {
  view: DecisionView;
  history: HistoryEntry[];
  historyLoading?: boolean;
  /** 試験で差し替える。既定は API に送る。 */
  revise?: (view: DecisionView, input: ReviseInput) => Promise<DecisionOutcome>;
};

/** 決定 1 件の詳細と「答えを変える」。 */
export function DecisionDetail({
  view,
  history,
  historyLoading = false,
  revise = reviseDecision,
}: DecisionDetailProps) {
  const queryClient = useQueryClient();
  const d = view.decision;
  const allowed = revisability(view);
  const [outcome, setOutcome] = useState<DecisionOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  async function refresh() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: allDecisionsKey }),
      queryClient.invalidateQueries({ queryKey: taskKeys.detail(view.task_id) }),
      queryClient.invalidateQueries({ queryKey: taskKeys.timelines(view.task_id) }),
    ]);
  }

  async function submit(input: ReviseInput) {
    setFailure(null);
    setOutcome(null);
    try {
      const result = await revise(view, input);
      queryClient.setQueriesData<DecisionList>({ queryKey: [...allDecisionsKey, "task"] }, (list) =>
        list
          ? {
              ...list,
              items: list.items.map((item) =>
                item.decision.id === result.decision.decision.id ? result.decision : item,
              ),
            }
          : list,
      );
      setOutcome(result);
      void refresh();
    } catch (error) {
      const failed = reviseFailure(error);
      setFailure(failed.message);
      if (failed.stale) void refresh();
      throw new Error(failed.message);
    }
  }

  return (
    <div className="mt-2 flex min-w-0 flex-col gap-3 text-label" data-testid="decision-detail">
      <dl className="grid min-w-0 gap-x-4 gap-y-1 sm:grid-cols-2">
        <dt className="font-medium text-muted-foreground">今の答え</dt>
        <dd className="min-w-0 break-words" data-testid="decision-current-answer">
          {d.answer ? (
            <>
              <strong>{optionLabel(d, d.answer.option)}</strong>
              {d.answer.note ? <span className="block whitespace-pre-wrap">理由: {d.answer.note}</span> : null}
            </>
          ) : (
            <span className="text-muted-foreground">{d.status === "withdrawn" ? "取り下げ済み" : "未回答"}</span>
          )}
        </dd>
        {d.answer ? (
          <>
            <dt className="font-medium text-muted-foreground">答えた人</dt>
            <dd>{byLabel(d.answer.by)}</dd>
            <dt className="font-medium text-muted-foreground">日時</dt>
            <dd>
              {view.answered_at ? <time dateTime={view.answered_at}>{formatAbsolute(view.answered_at)}</time> : "—"}
            </dd>
          </>
        ) : null}
        {view.effect ? (
          <>
            <dt className="font-medium text-muted-foreground">効き目</dt>
            <dd>{EFFECT_LABELS[view.effect]}</dd>
          </>
        ) : null}
        {d.withdrawn_reason ? (
          <>
            <dt className="font-medium text-muted-foreground">取り下げの理由</dt>
            <dd className="break-words">{d.withdrawn_reason}</dd>
          </>
        ) : null}
      </dl>
      <div className="min-w-0">
        <p className="font-medium text-muted-foreground">選択肢</p>
        <ul className="list-disc pl-5" data-testid="decision-options">
          {d.options.map((option) => (
            <li key={option.key} className="break-words">
              {option.label}
              {option.key === d.recommended ? "（推奨）" : ""}
              {option.key === d.answer?.option ? "（今の答え）" : ""}
              {option.consequence ? <span className="block text-muted-foreground">{option.consequence}</span> : null}
            </li>
          ))}
        </ul>
      </div>
      <HistoryList history={history} loading={historyLoading} withdrawn={d.status === "withdrawn"} />
      {failure ? (
        <p role="alert" data-testid="decision-revise-error" className="rounded-md bg-danger p-3 text-danger-foreground">
          {failure}
        </p>
      ) : null}
      {outcome ? (
        <div
          role="status"
          data-testid="decision-revise-result"
          className="rounded-md bg-success p-3 text-success-foreground"
        >
          {reviseSummary(outcome).map((line) => (
            <p key={line} className="break-words">
              {line}
            </p>
          ))}
          {(outcome.notified_children ?? []).length > 0 ? (
            <ul className="mt-1 flex flex-wrap gap-2">
              {(outcome.notified_children ?? []).map((child) => (
                <li key={child}>
                  <Link className="inline-flex min-h-11 items-center underline" to="/tasks/$id" params={{ id: child }}>
                    子タスク {child}
                  </Link>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
      {allowed.ok ? (
        <ReviseForm key={`${d.answer?.option ?? ""}:${view.answered_at ?? ""}`} view={view} onSubmit={submit} />
      ) : (
        <p data-testid="decision-revise-unavailable" className="break-words text-muted-foreground">
          答えを変えられません: {allowed.reason}
        </p>
      )}
    </div>
  );
}

function HistoryList({
  history,
  loading,
  withdrawn,
}: {
  history: HistoryEntry[];
  loading: boolean;
  withdrawn: boolean;
}) {
  return (
    <div className="min-w-0">
      <p className="font-medium text-muted-foreground">
        回答の履歴{withdrawn ? "（取り下げ済み）" : "（最後の回答が有効）"}
      </p>
      {loading ? (
        <p role="status" aria-busy="true">
          履歴を読み込み中…
        </p>
      ) : null}
      {history.length === 0 ? (
        <p className="text-muted-foreground">{loading ? "読み込み中…" : "回答はまだありません。"}</p>
      ) : (
        <ol className="flex min-w-0 flex-col gap-1" data-testid="decision-history">
          {history.map((row, index) => {
            const latest = index === history.length - 1;
            return (
              <li
                // biome-ignore lint/suspicious/noArrayIndexKey: 履歴は追記だけで、同じ時刻の行も順で区別する
                key={`${row.at ?? ""}:${index}`}
                data-latest={latest ? "true" : undefined}
                className="min-w-0 break-words"
              >
                {row.at ? <time dateTime={row.at}>{formatAbsolute(row.at)}</time> : "日時不明"} {byLabel(row.by)}: 「
                {row.label}」{row.note ? ` — ${row.note}` : ""}
                {latest ? (
                  <strong>{withdrawn ? "（取り下げ前の最終回答）" : "（有効）"}</strong>
                ) : (
                  <span className="text-muted-foreground">（置き換え済み）</span>
                )}
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}

function ReviseForm({ view, onSubmit }: { view: DecisionView; onSubmit: (input: ReviseInput) => Promise<void> }) {
  const id = useId();
  const d = view.decision;
  const current = d.answer?.option ?? "";
  const [option, setOption] = useState("");
  const [note, setNote] = useState("");
  const input = { option, note };
  const ready = reviseReady(view, input);
  const choices = d.options.some((o) => o.key === FREE_TEXT_OPTION)
    ? d.options
    : [...d.options, { key: FREE_TEXT_OPTION, label: optionLabel(d, FREE_TEXT_OPTION), consequence: null }];
  const from = current ? optionLabel(d, current) : "未回答";
  const to = option ? optionLabel(d, option) : "";
  return (
    <fieldset className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0" data-testid="decision-revise-form">
      <legend className="font-semibold text-foreground">答えを変える</legend>
      <div role="radiogroup" aria-label="新しい答え" className="flex min-w-0 flex-col">
        {choices.map((choice) => (
          <label key={choice.key} className="flex min-h-11 min-w-0 items-center gap-2 break-words">
            <input
              type="radio"
              name={`${id}-option`}
              value={choice.key}
              checked={option === choice.key}
              onChange={() => setOption(choice.key)}
              className="size-5 shrink-0"
            />
            <span className="min-w-0">
              {choice.label}
              {choice.key === current ? "（今の答え）" : ""}
            </span>
          </label>
        ))}
      </div>
      <label className="block">
        理由（note）
        {option === FREE_TEXT_OPTION && !d.options.some((o) => o.key === FREE_TEXT_OPTION) ? "（必須）" : "（任意）"}
        <textarea
          aria-label="答えを変える理由"
          maxLength={NOTE_MAX}
          className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </label>
      {option !== "" && option === current && note.trim() === "" ? (
        <p className="text-muted-foreground">今と同じ答えです。別の選択肢を選ぶか、理由を書いてください。</p>
      ) : null}
      <div>
        <ConfirmDialog
          trigger={
            <Button variant="primary" disabled={!ready}>
              答えを変える…
            </Button>
          }
          title="決定の答えを変える"
          target={d.question}
          consequence={`答えを「${from}」から「${to}」に変えます。理由: ${note.trim() || "記入なし"}。これから作られる子・これから走る仕事は新しい答えを読みます。既に作られた子タスクは作り直さず、コメントで新しい答えを届けます。`}
          reversibility="もう一度「答えを変える」で別の選択肢に戻せます。前の答えは回答の履歴に残ります。"
          followUp="この決定の回答の履歴と、通知した子タスクの一覧"
          confirmLabel={`答えを「${to}」に変える`}
          cancelLabel="戻る（変えない）"
          onConfirm={() => onSubmit(input)}
        />
      </div>
    </fieldset>
  );
}
