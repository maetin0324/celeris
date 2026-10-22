import { useState } from "react";
import { apiMutate, apiPathFromDaemon } from "../../api/client";
import type {
  InboxItem,
  IntegrationActionResult,
  IntegrationView,
  PromotionView,
  ReasonBody,
} from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";

const STATE_LABELS: Record<string, string> = {
  integrated: "取り込み済み",
  merged: "取り込み済み",
  unintegrated: "未取り込み",
  pending: "未取り込み",
  discarded: "捨てた",
  held: "保留",
  not_applicable: "対象外（遠隔 repo）",
};

export function integrationLabel(state: string): string {
  return STATE_LABELS[state] ?? state;
}

export function integrationActionPath(
  taskId: string,
  repoName: string,
  method: "merge" | "discard" | "hold" | "reopen",
): string {
  return `/api/tasks/${encodeURIComponent(taskId)}/integrations/${encodeURIComponent(repoName)}/${method}`;
}

export function TaskIntegrationActions({
  taskId,
  integration,
  promotion,
  inbox = false,
}: {
  taskId: string;
  integration: IntegrationView;
  promotion?: PromotionView | null;
  inbox?: boolean;
}) {
  const pending = integration.state === "unintegrated";
  const status = integrationLabel(integration.state);
  return (
    <div className="flex min-w-0 flex-col gap-2" data-testid="task-integration" data-state={integration.state}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="break-words font-medium">{integration.repo}</span>
        <Badge tone={pending ? "warning" : integration.state === "discarded" ? "danger" : "neutral"}>{status}</Badge>
        {promotion ? (
          <Badge tone="warning">
            昇格:{" "}
            {promotion.status === "promotion_pending"
              ? "昇格待ち"
              : promotion.status === "promoted"
                ? "昇格済み"
                : "対象外"}
          </Badge>
        ) : null}
      </div>
      <dl className="grid min-w-0 gap-1 break-words text-label">
        <div>
          <dt className="inline text-muted-foreground">branch: </dt>
          <dd className="inline">{integration.branch}</dd>
        </div>
        <div>
          <dt className="inline text-muted-foreground">head: </dt>
          <dd className="inline break-all">{integration.head}</dd>
        </div>
        <div>
          <dt className="inline text-muted-foreground">最終確認: </dt>
          <dd className="inline">{integration.checked_at || "未確認"}</dd>
        </div>
        {integration.reason ? (
          <div>
            <dt className="inline text-muted-foreground">理由: </dt>
            <dd className="inline">{integration.reason}</dd>
          </div>
        ) : null}
      </dl>
      <IntegrationControls
        path={(method) => integrationActionPath(taskId, integration.repo, method)}
        target={integration.repo}
        // not_applicable は遷移できない（API は 409）ので操作を出さない。
        methods={
          pending
            ? ["merge", "discard", "hold"]
            : integration.state === "held" || integration.state === "discarded"
              ? ["reopen"]
              : []
        }
      />
      {inbox ? <span className="sr-only">受信箱からの操作</span> : null}
    </div>
  );
}

export function InboxIntegrationActions({ item }: { item: InboxItem }) {
  const native = item.answer.native?.path;
  const mergePath = native === undefined ? undefined : apiPathFromDaemon(native);
  return (
    <div data-testid="inbox-integration-actions">
      <IntegrationControls
        path={(method) => mergePath?.replace(/merge$/, method)}
        target={item.detail ?? item.title}
        methods={["merge", "discard", "hold", "reopen"]}
        disabled={!mergePath || !item.task}
      />
    </div>
  );
}

type IntegrationMethod = "merge" | "discard" | "hold" | "reopen";

// Both entry points use the task-api ReasonBody contract, not InboxAnswerBody.note.
function IntegrationControls({
  path,
  target,
  methods,
  disabled = false,
}: {
  path: (method: IntegrationMethod) => string | undefined;
  target: string;
  methods: IntegrationMethod[];
  disabled?: boolean;
}) {
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [failed, setFailed] = useState(false);
  const run = async (method: IntegrationMethod) => {
    const url = path(method);
    if (!url || busy || disabled) return;
    if ((method === "discard" || method === "hold") && !reason.trim()) {
      setFailed(true);
      setMessage("理由を入力してください");
      return;
    }
    setBusy(true);
    setFailed(false);
    setMessage("");
    try {
      const body: ReasonBody | undefined =
        method === "discard" || method === "hold" ? { reason: reason.trim() } : undefined;
      const result = await apiMutate<IntegrationActionResult>("POST", url, body);
      setMessage(
        method === "merge" && !result.integrated
          ? "取り込みを依頼しました。取り込み完了は状態の更新で確認してください。"
          : "操作を受け付けました",
      );
    } catch {
      setFailed(true);
      setMessage("操作の完了を確認できません。ページを再読み込みして状態を確認してください。");
    } finally {
      setBusy(false);
    }
  };
  if (!methods.length) return null;
  return (
    <div className="grid min-w-0 gap-2" aria-busy={busy}>
      {methods.includes("hold") || methods.includes("discard") ? (
        <label className="grid min-w-0 gap-1 text-label">
          discard / hold の理由（必須）
          <input
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            disabled={busy || disabled}
            className="min-h-11 min-w-0 rounded-md border border-input bg-surface px-3 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          />
        </label>
      ) : null}
      <div className="flex flex-wrap gap-2">
        {methods.map((method) =>
          method === "discard" ? (
            <ConfirmDialog
              key={method}
              trigger={
                <Button type="button" disabled={busy || disabled || !reason.trim()}>
                  discard
                </Button>
              }
              title="変更を破棄"
              target={target}
              consequence="この repository の取り込みを見送り、理由を記録します。"
              reversibility="reopen で未取り込みに戻せます。"
              followUp="task 詳細の取り込み状態で確認できます。"
              confirmLabel="対象の変更を破棄"
              onConfirm={() => run("discard")}
            />
          ) : (
            <Button key={method} type="button" disabled={busy || disabled} onClick={() => void run(method)}>
              {method}
            </Button>
          ),
        )}
      </div>
      {busy ? <p role="status">送信中です</p> : null}
      {message ? <p role={failed ? "alert" : "status"}>{message}</p> : null}
    </div>
  );
}
