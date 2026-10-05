import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import { apiGet } from "../../api/client";
import type { Approval, ApprovalList, OrgList, StandingRule, StandingRuleList } from "../../api/generated/types";
import { inboxItemsQuery } from "../../api/queries/inbox-notifications";
import { approvalKeys, orgKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame, fetchView } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button, buttonVariants } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { fieldClassName, Input } from "../../components/ui/input";
import { Notice } from "../../components/ui/notice";
import { Section } from "../../components/ui/panel";
import { Select } from "../../components/ui/select";
import { ShortId } from "../../components/ui/short-id";
import { formatAbsolute } from "../../lib/time";

const decidedKey = approvalKeys.list({ pending: false });
const rulesKey = ["approvals", "rules"] as const;
const rulesAnchor = "standing-rules";

const linkClass =
  "inline-flex min-h-11 items-center underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

// 決めた結果は見た目でも区別する。今後も認めた（常設）は以後の依頼に効くので warning、取り下げは結果なしの neutral。
export const decisionView: Record<string, { label: string; tone: BadgeTone }> = {
  once: { label: "今回だけ認めた", tone: "success" },
  standing: { label: "今後も認めた", tone: "warning" },
  denied: { label: "認めなかった", tone: "danger" },
  withdrawn: { label: "取り下げ", tone: "neutral" },
};

/** 新しく決めたものを上に。決めた日時が無い記録は依頼の日時で並べる。 */
export function sortDecided(items: readonly Approval[]): Approval[] {
  const at = (item: Approval) => item.decided_at ?? item.created_at;
  return [...items].sort((a, b) => at(b).localeCompare(at(a)));
}

function useOrgNames() {
  const org = useQuery({ queryKey: orgKeys.list(), queryFn: ({ signal }) => apiGet<OrgList>("/api/org", signal) });
  const names = new Map((org.data?.items ?? []).map((node) => [node.id, node.name]));
  return { org, names };
}

function NodeName({ id, names }: { id: string; names: Map<string, string> }) {
  const name = names.get(id);
  return name ? <span>{name}</span> : <ShortId value={id} label="課の ID" length={16} copyable={false} />;
}

function RuleRow({
  item,
  names,
  sender,
}: {
  item: StandingRule;
  names: Map<string, string>;
  sender: ReturnType<typeof useActionResult>;
}) {
  const target = item.node_id ? (names.get(item.node_id) ?? item.node_id) : "全員";
  return (
    <li className="flex min-w-0 flex-col gap-2 border-b border-border py-3 sm:flex-row sm:items-center sm:gap-4">
      <div className="min-w-0 flex-1">
        <p className="break-words">{item.rule}</p>
        <p className="text-label text-muted-foreground">
          対象: {item.node_id ? <NodeName id={item.node_id} names={names} /> : "全員"}・追加{" "}
          {formatAbsolute(item.created_at)}
        </p>
        <ActionResultView result={sender.results[item.id]} />
      </div>
      <ConfirmDialog
        trigger={
          <Button variant="destructive" size="sm" disabled={sender.pending}>
            削除
          </Button>
        }
        title="常設ルールを削除しますか"
        target={`${item.rule}（対象: ${target}）`}
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
  );
}

function Rules() {
  const query = useQuery({
    queryKey: rulesKey,
    queryFn: ({ signal }) => apiGet<StandingRuleList>("/api/standing-rules", signal),
  });
  const { org, names } = useOrgNames();
  const sender = useActionResult(rulesKey);
  const [rule, setRule] = useState("");
  const [node, setNode] = useState("");
  const id = useId();
  const view = fetchView(query);
  // 既存のルールを確かめられない間は足さない（重複・広すぎる規則を防ぐ）。
  const blocked = view === "error" || view === "disconnected" || view === "permission-denied" || view === "loading";
  const created = sender.results.create;
  const createdId =
    created?.ok && created.response && typeof created.response === "object"
      ? (created.response as Partial<StandingRule>).id
      : undefined;
  const createdStillListed = createdId !== undefined && query.data?.items.some((item) => item.id === createdId);
  const targetName = node ? (names.get(node) ?? node) : "全員";
  return (
    <Section
      id={rulesAnchor}
      title="常設ルール"
      description="ここにある規則は対象の課の仕事に常に添えられ、一致する認可の依頼は受信箱に出ずに自動で認められます。"
    >
      <div className="flex flex-col gap-4">
        <FetchFrame query={query} subject="常設ルール">
          {query.data?.items.length ? (
            <ul aria-label="常設ルールの一覧" className="flex flex-col border-t border-border">
              {query.data.items.map((item) => (
                <RuleRow key={item.id} item={item} names={names} sender={sender} />
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
            if (!rule.trim() || blocked) return;
            void sender.run([
              { id: "create", path: "/api/standing-rules", body: { rule, ...(node ? { node_id: node } : {}) } },
            ]);
          }}
        >
          <h3 className="text-body font-semibold">常設ルールを追加</h3>
          {blocked && view !== "loading" ? (
            <Notice title="既存のルールを確認できないため、追加を止めています。">
              {view === "permission-denied"
                ? "常設ルールを扱う権限がありません。"
                : "上の一覧を取り直してから追加してください。"}
            </Notice>
          ) : null}
          <fieldset disabled={blocked} className="flex min-w-0 flex-col gap-3">
            <div className="flex flex-col gap-1">
              <label htmlFor={`${id}-node`} className="text-label font-medium">
                対象の課（空は全員）
              </label>
              {org.data ? (
                <Select id={`${id}-node`} value={node} onChange={(event) => setNode(event.target.value)}>
                  <option value="">全員</option>
                  {org.data.items.map((item) => (
                    <option key={item.id} value={item.id}>
                      {item.name}
                    </option>
                  ))}
                </Select>
              ) : (
                <Input
                  id={`${id}-node`}
                  value={node}
                  placeholder="組織を読めないときは課の ID を入力"
                  onChange={(event) => setNode(event.target.value)}
                />
              )}
            </div>
            <div className="flex flex-col gap-1">
              <label id={`${id}-rule-label`} htmlFor={`${id}-rule`} className="text-label font-medium">
                規則文
              </label>
              <p id={`${id}-rule-hint`} className="text-label text-muted-foreground">
                例: 「{"cross-department: software-engineering -> cluster-hpc"}
                」。書いた範囲に一致する依頼が以後すべて自動で通るので、広すぎる書き方を避けます。
              </p>
              <textarea
                id={`${id}-rule`}
                aria-labelledby={`${id}-rule-label`}
                className={fieldClassName}
                value={rule}
                onChange={(event) => setRule(event.target.value)}
                aria-describedby={
                  sender.results.create?.status === 422 ? `${id}-rule-hint rule-result` : `${id}-rule-hint`
                }
              />
            </div>
            <p className="text-label text-muted-foreground">
              追加すると、{targetName}
              の認可の依頼のうちこの規則に一致するものは、確認なしで自動で認められます。取り消すには一覧の「削除」を使います。
            </p>
            <div>
              <Button type="submit" disabled={sender.pending || !rule.trim()}>
                追加
              </Button>
            </div>
          </fieldset>
          <ActionResultView result={created} fieldId="rule-result" />
          {createdId && createdStillListed ? (
            <div>
              <Button
                variant="secondary"
                size="sm"
                disabled={sender.pending}
                onClick={() =>
                  void sender.run([
                    { id: createdId, path: `/api/standing-rules/${encodeURIComponent(createdId)}`, method: "DELETE" },
                  ])
                }
              >
                いま足した規則を取り消す
              </Button>
            </div>
          ) : null}
        </form>
      </div>
    </Section>
  );
}

function DecidedRow({ item, names }: { item: Approval; names: Map<string, string> }) {
  const view = item.decision ? decisionView[item.decision] : undefined;
  return (
    <li
      data-approval-id={item.id}
      className="flex min-w-0 flex-col gap-2 border-b border-border py-3 sm:flex-row sm:gap-4"
    >
      <span className="flex shrink-0 flex-col gap-1 sm:w-32">
        <Badge tone={view?.tone ?? "neutral"}>{view?.label ?? "記録なし"}</Badge>
        {item.decision === "standing" ? (
          <a href={`#${rulesAnchor}`} className={`${linkClass} text-label`}>
            常設ルールを見る
          </a>
        ) : null}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="max-w-prose-ja min-w-0 break-words">
          <Markdown source={item.question} />
        </div>
        {item.answer ? <p className="max-w-prose-ja break-words text-label">回答: {item.answer}</p> : null}
        <p className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-label text-muted-foreground">
          <span>
            依頼元: <NodeName id={item.node_id} names={names} />
          </span>
          {item.task_id ? (
            <Link to="/tasks/$id" params={{ id: item.task_id }} className={linkClass}>
              元のタスクを開く
            </Link>
          ) : null}
          {item.decided_at ? (
            <span>
              決めた日時: <time dateTime={item.decided_at}>{formatAbsolute(item.decided_at)}</time>
            </span>
          ) : null}
        </p>
      </div>
    </li>
  );
}

export function ApprovalsScreen() {
  // /approvals は常設ルールと履歴だけの最小限。未決の認可は受信箱の `authorization` 項目として答える（web ADR 2026-10-04 D4）。ここは件数と誘導だけ。
  const pending = useQuery(inboxItemsQuery({ kind: "authorization" }));
  const decided = useQuery({
    queryKey: decidedKey,
    queryFn: ({ signal }) => apiGet<ApprovalList>("/api/approvals?pending=false", signal),
  });
  const { names } = useOrgNames();
  const count = pending.data?.counts.total;
  const decidedItems = sortDecided(decided.data?.items ?? []);
  return (
    <ScreenFrame
      title="承認"
      route="/approvals"
      description="判断待ちの認可は受信箱で答えます。ここでは決めた認可を振り返り、自動で認める常設ルールを管理します。"
    >
      <Section title="認可待ち">
        <FetchFrame query={pending} subject="未決の認可の件数">
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
      <Section title="決めたもの" description="新しく決めた順です。">
        <FetchFrame query={decided} subject="決めた認可">
          {decidedItems.length ? (
            <ul aria-label="決めた認可" className="flex flex-col border-t border-border">
              {decidedItems.map((item) => (
                <DecidedRow key={item.id} item={item} names={names} />
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
