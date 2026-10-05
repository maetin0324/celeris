import { useState } from "react";
import type { TaskDetail } from "../../api/generated/types";
import { ActionResultView, type ActionTarget, useActionResult } from "../../components/actions/use-action-result";
import { Button } from "../../components/ui/button";
import { statusView } from "../../components/ui/status-badge";
import { taskDetailQueryKey } from "./task-detail-query";

// /tasks/:id の判断パネル（P3-09、R23）。approve / reject / answer / cancel / retry / edit / comment / reopen。
// 送信は P3-03 の useActionResult。状態を変える操作は expected_status を送り、409 は detail の key を取り直す。
// パネルは本文の流れに置き（fixed にしない）、スマホでも本文を隠さない。

export type DecisionIntent = "approve" | "reject" | "answer" | "cancel" | "retry" | "edit" | "comment" | "reopen";

const labels: Record<DecisionIntent, string> = {
  approve: "承認",
  reject: "却下",
  answer: "回答",
  cancel: "中止",
  retry: "やり直す",
  edit: "編集を保存",
  comment: "コメント",
  reopen: "再開",
};

/** 送信先と本文。詳細の status を expected_status にする（comment と retry は状態の比較をしない）。 */
export function decisionTarget(
  detail: TaskDetail,
  intent: DecisionIntent,
  input: { note: string; answer: string; comment: string; title: string; objective: string },
): ActionTarget {
  const id = detail.task.id;
  const base = `/api/tasks/${encodeURIComponent(id)}`;
  const expected_status = detail.task.status;
  const key = `${id}:${intent}`;
  switch (intent) {
    case "approve":
    case "reject":
      return { id: key, path: `${base}/${intent}`, body: { expected_status, note: input.note } };
    case "answer":
      return { id: key, path: `${base}/answer`, body: { expected_status, answer: input.answer } };
    case "cancel":
      return { id: key, path: `${base}/cancel`, body: { expected_status } };
    case "retry":
      return { id: key, path: `${base}/retry`, body: {} };
    case "reopen":
      return { id: key, path: `${base}/reopen`, body: { expected_status } };
    case "comment":
      return { id: key, path: `${base}/comments`, body: { body: input.comment } };
    case "edit":
      return {
        id: key,
        path: base,
        method: "PATCH",
        body: { expected_status, title: input.title, objective: input.objective },
      };
  }
}

export function DecisionPanel({ detail }: { detail: TaskDetail }) {
  const sender = useActionResult(taskDetailQueryKey(detail.task.id));
  const [note, setNote] = useState("");
  const [answer, setAnswer] = useState("");
  const [comment, setComment] = useState("");
  const [title, setTitle] = useState(detail.task.title);
  const [objective, setObjective] = useState(detail.task.objective);
  const actions = detail.actions;
  const has = (intent: DecisionIntent) => (actions as string[]).includes(intent);
  const [last, setLast] = useState<DecisionIntent | null>(null);
  const input = { note, answer, comment, title, objective };
  function submit(intent: DecisionIntent) {
    setLast(intent);
    void sender.run([decisionTarget(detail, intent, input)]);
  }
  const result = last ? sender.results[`${detail.task.id}:${last}`] : undefined;
  const fieldId = `decision-result-${detail.task.id}`;
  const described = result && !result.ok ? fieldId : undefined;
  const transitions = (["approve", "reject", "cancel", "retry", "reopen"] as const).filter(has);
  return (
    <section
      id="decision-panel"
      aria-labelledby="decision-panel-title"
      data-testid="decision-panel"
      className="min-w-0 scroll-mt-4 space-y-3 rounded-lg border border-border bg-surface p-4"
    >
      <h2 id="decision-panel-title" className="text-section font-semibold text-foreground">
        判断
      </h2>
      <p className="text-label">
        状態{" "}
        <span data-testid="decision-status" data-status={detail.task.status}>
          {statusView(detail.task.status).label}
        </span>
      </p>
      {detail.latest_question && has("answer") ? (
        <p className="break-words text-label">質問: {detail.latest_question}</p>
      ) : null}
      <ActionResultView result={result} fieldId={fieldId} />
      {(has("approve") || has("reject")) && (
        <label className="block text-label">
          理由・note（任意）
          <textarea
            aria-label="理由・note（任意）"
            className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
            value={note}
            onChange={(event) => setNote(event.target.value)}
            aria-describedby={described}
          />
        </label>
      )}
      {has("answer") && (
        <div className="space-y-2">
          <label className="block text-label">
            回答
            <textarea
              aria-label="回答"
              className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
              value={answer}
              onChange={(event) => setAnswer(event.target.value)}
              aria-describedby={described}
            />
          </label>
          <Button disabled={sender.pending || answer.trim() === ""} onClick={() => submit("answer")}>
            {labels.answer}
          </Button>
        </div>
      )}
      {transitions.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {transitions.map((intent) => (
            <Button key={intent} disabled={sender.pending} onClick={() => submit(intent)}>
              {labels[intent]}
            </Button>
          ))}
        </div>
      )}
      {has("edit") && (
        <details className="min-w-0">
          <summary className="inline-flex min-h-11 cursor-pointer items-center">編集</summary>
          <div className="space-y-2">
            <label className="block text-label">
              題
              <input
                className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
                value={title}
                onChange={(event) => setTitle(event.target.value)}
              />
            </label>
            <label className="block text-label">
              目的
              <textarea
                aria-label="目的"
                className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
                value={objective}
                onChange={(event) => setObjective(event.target.value)}
              />
            </label>
            <Button disabled={sender.pending} onClick={() => submit("edit")}>
              {labels.edit}
            </Button>
          </div>
        </details>
      )}
      <div className="space-y-2">
        <label className="block text-label">
          コメント
          <textarea
            aria-label="コメント"
            className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body"
            value={comment}
            onChange={(event) => setComment(event.target.value)}
          />
        </label>
        <Button disabled={sender.pending || comment.trim() === ""} onClick={() => submit("comment")}>
          {labels.comment}
        </Button>
      </div>
    </section>
  );
}
