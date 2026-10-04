import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import { apiGet } from "../../api/client";
import type { ApprovalList, StandingRuleList } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { approvalKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button, buttonVariants } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Section } from "../../components/ui/panel";

const decidedKey = approvalKeys.list({ pending: false });
const rulesKey = ["approvals", "rules"] as const;

const fieldClass =
  "block min-h-11 w-full rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

const decisionLabels: Record<string, string> = {
  once: "今回だけ認めた",
  standing: "今後も認めた",
  denied: "認めなかった",
  withdrawn: "取り下げ",
};

function Rules() {
  const query = useQuery({
    queryKey: rulesKey,
    queryFn: ({ signal }) => apiGet<StandingRuleList>("/api/standing-rules", signal),
  });
  const sender = useActionResult(rulesKey);
  const [rule, setRule] = useState("");
  const [node, setNode] = useState("");
  const id = useId();
  return (
    <Section title="常設ルール" description="一致する認可の依頼を、受信箱に出さずに自動で認めます。">
      <div className="flex flex-col gap-4">
        <FetchFrame query={query}>
          {query.data?.items.length ? (
            <ul aria-label="常設ルールの一覧" className="flex flex-col border-t border-border">
              {query.data.items.map((item) => (
                <li
                  key={item.id}
                  className="flex min-w-0 flex-col gap-2 border-b border-border py-3 sm:flex-row sm:items-center sm:gap-4"
                >
                  <div className="min-w-0 flex-1">
                    <p className="break-words">{item.rule}</p>
                    <p className="text-label text-muted-foreground">対象: {item.node_id || "全員"}</p>
                    <ActionResultView result={sender.results[item.id]} />
                  </div>
                  <ConfirmDialog
                    trigger={
                      <Button variant="destructive" size="sm" disabled={sender.pending}>
                        削除
                      </Button>
                    }
                    title="常設ルールを削除しますか"
                    target={item.rule}
                    consequence="以後、一致する認可の依頼は受信箱に出て、人の判断を待ちます。"
                    reversibility="同じ規則文で追加し直せます。"
                    followUp="この一覧から消えたことで確かめられます。"
                    confirmLabel="常設ルールを削除"
                    onConfirm={async () => {
                      const [outcome] = await sender.run([
                        { id: item.id, path: `/api/standing-rules/${encodeURIComponent(item.id)}`, method: "DELETE" },
                      ]);
                      if (outcome && !outcome.ok) throw new Error(outcome.message);
                    }}
                  />
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-muted-foreground">常設ルールはありません。</p>
          )}
        </FetchFrame>
        <form
          aria-label="常設ルールを追加"
          className="flex max-w-form flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (!rule.trim()) return;
            void sender.run([
              { id: "create", path: "/api/standing-rules", body: { rule, ...(node ? { node_id: node } : {}) } },
            ]);
          }}
        >
          <h3 className="text-body font-semibold">常設ルールを追加</h3>
          <div className="flex flex-col gap-1">
            <label htmlFor={`${id}-node`} className="text-label font-medium">
              対象の node ID（空欄は全員）
            </label>
            <input
              id={`${id}-node`}
              className={fieldClass}
              value={node}
              onChange={(event) => setNode(event.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={`${id}-rule`} className="text-label font-medium">
              規則文
            </label>
            <textarea
              id={`${id}-rule`}
              className={fieldClass}
              value={rule}
              onChange={(event) => setRule(event.target.value)}
              aria-describedby={sender.results.create?.status === 422 ? "rule-result" : undefined}
            />
          </div>
          <div>
            <Button type="submit" disabled={sender.pending || !rule.trim()}>
              追加
            </Button>
          </div>
          <ActionResultView result={sender.results.create} fieldId="rule-result" />
        </form>
      </div>
    </Section>
  );
}

export function ApprovalsScreen() {
  // /approvals は常設ルールと履歴だけの最小限。未決の認可は受信箱の `authorization` 項目として答える（web ADR 2026-10-04 D4）。ここは件数と誘導だけ。
  const pending = useQuery(inboxItemsQuery({ kind: "authorization" }));
  const decided = useQuery({
    queryKey: decidedKey,
    queryFn: ({ signal }) => apiGet<ApprovalList>("/api/approvals?pending=false", signal),
  });
  const count = pending.data?.counts.total;
  return (
    <ScreenFrame
      title="承認"
      route="/approvals"
      description="認可の判断は受信箱で返します。この画面は常設ルールと、これまでに決めた認可の記録です。"
    >
      <Section title="認可待ち">
        <FetchFrame query={pending}>
          <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:gap-4">
            <p>{count ? `受信箱に未決の認可が ${count} 件あります。` : "未決の認可はありません。"}</p>
            <Link
              className={buttonVariants({ variant: count ? "primary" : "secondary" })}
              to="/inbox"
              search={{ kind: "authorization" }}
            >
              受信箱で認可を判断する
            </Link>
          </div>
        </FetchFrame>
      </Section>
      <Section title="決めたもの">
        <FetchFrame query={decided}>
          {decided.data?.items.length ? (
            <ul aria-label="決めた認可" className="flex flex-col border-t border-border">
              {decided.data.items.map((item) => (
                <li
                  key={item.id}
                  className="flex min-w-0 flex-col gap-1 border-b border-border py-3 sm:flex-row sm:gap-4"
                >
                  <span className="shrink-0 sm:w-24">
                    <Badge tone={item.decision === "denied" ? "danger" : "success"}>
                      {decisionLabels[item.decision ?? ""] ?? item.decision}
                    </Badge>
                  </span>
                  <div className="min-w-0 flex-1 text-label">
                    <Markdown source={item.question} />
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-muted-foreground">まだ決めたものはありません。</p>
          )}
        </FetchFrame>
      </Section>
      <Rules />
    </ScreenFrame>
  );
}
