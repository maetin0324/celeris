import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { Approval, ApprovalList, Decision, StandingRuleList } from "../../api/generated/types";
import { approvalKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

const pendingKey = approvalKeys.list({ pending: true });
const decidedKey = approvalKeys.list({ pending: false });
const rulesKey = ["approvals", "rules"] as const;

function ApprovalRow({ approval }: { approval: Approval }) {
  const [answer, setAnswer] = useState("");
  const [scope, setScope] = useState("");
  const sender = useActionResult(approvalKeys.all);
  const result = sender.results[approval.id];
  const fieldId = `approval-result-${approval.id}`;
  const decide = (decision: Decision) => {
    void sender.run([
      {
        id: approval.id,
        path: `/api/approvals/${encodeURIComponent(approval.id)}/decide`,
        body: { answer, decision, ...(scope ? { scope } : {}) },
      },
    ]);
  };
  return (
    <li id={`approval-${approval.id}`} data-approval-id={approval.id} className="min-w-0 rounded border p-3 space-y-2">
      <div className="flex flex-wrap gap-2 text-sm text-neutral-600">
        <span>{approval.node_id}</span>
        {approval.task_id && (
          <Link
            to="/tasks/$id"
            params={{ id: approval.task_id }}
            className="underline min-h-11 inline-flex items-center"
          >
            タスク {approval.task_id}
          </Link>
        )}
      </div>
      <Markdown source={approval.question} />
      <label className="block">
        回答
        <textarea
          className="block w-full min-h-11 rounded border p-2"
          value={answer}
          onChange={(event) => setAnswer(event.target.value)}
          aria-describedby={result?.status === 422 ? fieldId : undefined}
        />
      </label>
      <label className="block">
        適用範囲（任意）
        <input
          className="block w-full min-h-11 rounded border p-2"
          value={scope}
          onChange={(event) => setScope(event.target.value)}
        />
      </label>
      <div className="flex flex-wrap gap-2">
        <Button disabled={sender.pending} onClick={() => decide("once")}>
          今回だけ認める
        </Button>
        <Button disabled={sender.pending} onClick={() => decide("standing")}>
          今後も認める
        </Button>
        <Button disabled={sender.pending} onClick={() => decide("denied")}>
          認めない
        </Button>
      </div>
      <ActionResultView result={result} fieldId={fieldId} />
    </li>
  );
}

function Rules() {
  const query = useQuery({
    queryKey: rulesKey,
    queryFn: ({ signal }) => apiGet<StandingRuleList>("/api/standing-rules", signal),
  });
  const sender = useActionResult(rulesKey);
  const [rule, setRule] = useState("");
  const [node, setNode] = useState("");
  return (
    <section className="space-y-3">
      <h2 className="text-lg font-semibold">常設ルール</h2>
      <FetchFrame query={query}>
        {query.data?.items.length ? (
          <ul className="space-y-2">
            {query.data.items.map((item) => (
              <li key={item.id} className="min-w-0 rounded border p-3">
                <p className="break-words">{item.rule}</p>
                <p className="text-sm">対象: {item.node_id || "全員"}</p>
                <Button
                  disabled={sender.pending}
                  onClick={() =>
                    void sender.run([
                      { id: item.id, path: `/api/standing-rules/${encodeURIComponent(item.id)}`, method: "DELETE" },
                    ])
                  }
                >
                  削除
                </Button>
                <ActionResultView result={sender.results[item.id]} />
              </li>
            ))}
          </ul>
        ) : (
          <p>常設ルールはありません。</p>
        )}
      </FetchFrame>
      <div className="rounded border p-3 space-y-2">
        <h3 className="font-medium">常設ルールを追加</h3>
        <label className="block">
          対象の node ID（空欄は全員）
          <input
            className="block w-full min-h-11 rounded border p-2"
            value={node}
            onChange={(event) => setNode(event.target.value)}
          />
        </label>
        <label className="block">
          規則文
          <textarea
            className="block w-full min-h-11 rounded border p-2"
            value={rule}
            onChange={(event) => setRule(event.target.value)}
            aria-describedby={sender.results.create?.status === 422 ? "rule-result" : undefined}
          />
        </label>
        <Button
          disabled={sender.pending || !rule.trim()}
          onClick={() =>
            void sender.run([
              { id: "create", path: "/api/standing-rules", body: { rule, ...(node ? { node_id: node } : {}) } },
            ])
          }
        >
          追加
        </Button>
        <ActionResultView result={sender.results.create} fieldId="rule-result" />
      </div>
    </section>
  );
}

export function ApprovalsScreen() {
  const pending = useQuery({
    queryKey: pendingKey,
    queryFn: ({ signal }) => apiGet<ApprovalList>("/api/approvals?pending=true", signal),
  });
  const decided = useQuery({
    queryKey: decidedKey,
    queryFn: ({ signal }) => apiGet<ApprovalList>("/api/approvals?pending=false", signal),
  });
  return (
    <ScreenFrame title="承認" route="/approvals">
      <section className="space-y-3">
        <h2 className="text-lg font-semibold">認可待ち</h2>
        <FetchFrame query={pending}>
          {pending.data?.items.length ? (
            <ul className="space-y-3">
              {pending.data.items.map((item) => (
                <ApprovalRow key={item.id} approval={item} />
              ))}
            </ul>
          ) : (
            <p>未決の要求はありません。</p>
          )}
        </FetchFrame>
      </section>
      <section className="space-y-3">
        <h2 className="text-lg font-semibold">決めたもの</h2>
        <FetchFrame query={decided}>
          {decided.data?.items.length ? (
            <ul className="space-y-2">
              {decided.data.items.map((item) => (
                <li key={item.id} className="rounded border p-3">
                  <span>{item.decision}</span>
                  <Markdown source={item.question} />
                </li>
              ))}
            </ul>
          ) : (
            <p>まだ決めたものはありません。</p>
          )}
        </FetchFrame>
      </section>
      <Rules />
    </ScreenFrame>
  );
}
