import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { Action, Inbox, TaskRef } from "../../api/generated/types";
import { inboxKeys } from "../../api/queries/keys";
import { inboxQuery } from "../../api/queries/server-state";
import { ActionResultView, type ActionTarget, useActionResult } from "../../components/actions/use-action-result";
import { ArtifactPreview } from "../../components/content/artifact-preview";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { attentionIntegrationRepair } from "../tasks/integration-repair";
import { IntegrationRepairPanel } from "../tasks/integration-repair-panel";

const labels: Record<string, string> = { approve: "承認", reject: "却下", answer: "回答", cancel: "中止" };
const actions = ["approve", "reject", "answer", "cancel"] as const;
type Transition = (typeof actions)[number];

type Sender = ReturnType<typeof useActionResult>;
function TaskActions({
  task,
  sender,
  allowed = actions,
}: {
  task: TaskRef;
  sender: Sender;
  allowed?: readonly Transition[];
}) {
  const [note, setNote] = useState("");
  const [answer, setAnswer] = useState("");
  const available = actions.filter((action) => allowed.includes(action) && task.actions.includes(action as Action));
  if (available.length === 0) return null;
  const result = sender.results[task.id];
  const fieldId = `action-result-${task.id}`;
  function submit(intent: Transition) {
    const ids = [task];
    const targets: ActionTarget[] = ids.map((target) => ({
      id: target.id,
      path: `/api/tasks/${encodeURIComponent(target.id)}/${intent}`,
      body: {
        expected_status: target.status,
        ...(intent === "answer" ? { answer } : intent === "approve" || intent === "reject" ? { note } : {}),
      },
    }));
    void sender.run(targets);
  }
  return (
    <div className="space-y-2 min-w-0">
      {(available.includes("approve") || available.includes("reject")) && (
        <label className="block">
          理由・note（任意）
          <textarea
            className="block w-full min-h-11 rounded border p-2"
            value={note}
            onChange={(event) => setNote(event.target.value)}
            aria-describedby={result?.status === 422 ? fieldId : undefined}
          />
        </label>
      )}
      {available.includes("answer") && (
        <label className="block">
          回答
          <textarea
            className="block w-full min-h-11 rounded border p-2"
            value={answer}
            onChange={(event) => setAnswer(event.target.value)}
            aria-describedby={result?.status === 422 ? fieldId : undefined}
          />
        </label>
      )}
      <div className="flex flex-wrap gap-2">
        {available.map((intent) => (
          <Button key={intent} disabled={sender.pending} onClick={() => submit(intent)}>
            {labels[intent]}
          </Button>
        ))}
      </div>
      <ActionResultView result={result} fieldId={fieldId} />
    </div>
  );
}

function Section({ title, count, children }: { title: string; count: number; children: React.ReactNode }) {
  return (
    <section className="min-w-0 rounded-lg border border-neutral-300 p-3 space-y-3">
      <h2 className="text-lg font-semibold">
        {title}（{count}）
      </h2>
      {count ? children : <p>ありません。</p>}
    </section>
  );
}
function TaskLink({ task }: { task: TaskRef }) {
  return (
    <Link className="font-medium underline break-words" to="/tasks/$id" params={{ id: task.id }}>
      {task.title}
    </Link>
  );
}
function InboxContent({ inbox, sender }: { inbox: Inbox; sender: Sender }) {
  return (
    <div className="space-y-4 min-w-0">
      {Object.values(sender.results).length > 0 && (
        <section aria-label="操作の結果" className="rounded border p-3 space-y-1">
          {Object.values(sender.results).map((result) => (
            <div key={result.id}>
              <strong>{result.id}: </strong>
              <ActionResultView result={result} />
            </div>
          ))}
        </section>
      )}
      <p className="text-sm">
        承認待ち {inbox.counts.approvals} / 質問 {inbox.counts.questions} / draft {inbox.counts.drafts} / 注意{" "}
        {inbox.counts.attention}
      </p>
      <Section title="承認待ち" count={inbox.approvals.length}>
        <ul className="space-y-3">
          {inbox.approvals.map((item) => (
            <li key={item.approval.id} className="min-w-0 rounded border p-3 space-y-2">
              <TaskLink task={item.approval} />
              <Markdown source={item.criterion_text} />
              {item.artifacts.map((artifact) => (
                <ArtifactPreview
                  key={artifact.idx}
                  taskId={item.parent?.id ?? item.approval.id}
                  idx={artifact.idx}
                  name={artifact.name}
                />
              ))}
              <TaskActions task={item.approval} sender={sender} allowed={["approve", "reject"]} />
            </li>
          ))}
        </ul>
      </Section>
      <Section title="質問" count={inbox.questions.length}>
        <ul className="space-y-3">
          {inbox.questions.map((item) => (
            <li key={item.task.id} className="rounded border p-3 space-y-2">
              <TaskLink task={item.task} />
              <Markdown source={item.question} />
              <TaskActions task={item.task} sender={sender} allowed={["answer"]} />
            </li>
          ))}
        </ul>
      </Section>
      <Section title="受け入れ待ちの draft" count={inbox.drafts.length}>
        <ul className="space-y-3">
          {inbox.drafts.map((group, index) => (
            <li key={group.parent?.id ?? index} className="rounded border p-3 space-y-2">
              {group.parent && <TaskLink task={group.parent} />}
              {group.plan_summary && <Markdown source={group.plan_summary} />}
              {group.drafts.length > 1 && (
                <Button
                  disabled={sender.pending}
                  onClick={() =>
                    void sender.run(
                      group.drafts.map((draft) => ({
                        id: draft.id,
                        path: `/api/tasks/${encodeURIComponent(draft.id)}/approve`,
                        body: { expected_status: draft.status, note: null },
                      })),
                    )
                  }
                >
                  この Plan の子を全部受け入れ
                </Button>
              )}
              {group.drafts.map((draft) => (
                <div key={draft.id} className="border-t pt-2">
                  <TaskLink task={draft} />
                  <TaskActions task={draft} sender={sender} allowed={["approve", "cancel"]} />
                </div>
              ))}
            </li>
          ))}
        </ul>
      </Section>
      <Section title="注意" count={inbox.attention.length}>
        <ul className="space-y-3">
          {inbox.attention.map((item, index) => (
            <li key={`${item.type}-${"task" in item ? item.task.id : index}`} className="rounded border p-3 space-y-2">
              <p className="font-medium">{item.type}</p>
              {"task" in item && (
                <>
                  <TaskLink task={item.task} />
                  <TaskActions task={item.task} sender={sender} allowed={["cancel"]} />
                </>
              )}
              {"reason" in item && <Markdown source={item.reason} />}
              <IntegrationRepairPanel view={attentionIntegrationRepair(item)} />
              {"cluster" in item && (
                <Link className="underline" to="/clusters">
                  クラスタ {item.cluster} を確認
                </Link>
              )}
              {"phase_title" in item && <p>{item.phase_title}</p>}
            </li>
          ))}
        </ul>
      </Section>
      {inbox.decisions.length > 0 && (
        <Section title="決定" count={inbox.decisions.length}>
          <ul>
            {inbox.decisions.map((item) => (
              <li key={item.id} className="rounded border p-3">
                <Link className="underline" to="/tasks/$id" params={{ id: item.task_id }}>
                  {item.question}
                </Link>
              </li>
            ))}
          </ul>
        </Section>
      )}
    </div>
  );
}
export function InboxScreen() {
  const query = useQuery(inboxQuery);
  const sender = useActionResult(inboxKeys.list());
  return (
    <ScreenFrame title="受信箱" route="/inbox">
      <FetchFrame query={query}>{query.data && <InboxContent inbox={query.data} sender={sender} />}</FetchFrame>
    </ScreenFrame>
  );
}
