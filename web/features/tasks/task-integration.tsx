import { useState } from "react";
import { apiMutate } from "../../api/client";
import type { IntegrationView, PromotionView } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";

const STATE_LABELS: Record<string, string> = {
  integrated: "取り込み済み",
  merged: "取り込み済み",
  unintegrated: "未取り込み",
  pending: "未取り込み",
  discarded: "捨てた",
  held: "保留",
};

export function integrationLabel(state: string): string {
  return STATE_LABELS[state] ?? state;
}

export function integrationActionPath(
  taskId: string,
  repoId: string,
  method: "merge" | "discard" | "hold" | "reopen",
): string {
  return `/api/v1/tasks/${encodeURIComponent(taskId)}/integrations/${encodeURIComponent(repoId)}/${method}`;
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
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const pending = integration.state === "unintegrated";
  const act = async (method: "merge" | "discard" | "hold" | "reopen") => {
    if ((method === "discard" || method === "hold") && !note.trim()) {
      setMessage("理由を入力してください");
      return;
    }
    setBusy(true);
    setMessage("");
    try {
      await apiMutate("POST", integrationActionPath(taskId, integration.repo_id, method), {
        ...(method === "discard" || method === "hold" ? { note: note.trim() } : {}),
        ...(method === "discard" ? { confirm: true } : {}),
      });
      setMessage("操作を受け付けました");
    } catch {
      setMessage("操作に失敗しました");
    } finally {
      setBusy(false);
    }
  };
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
      {pending ? (
        <>
          <label className="grid gap-1 text-label">
            discard / hold の理由
            <input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              className="min-h-11 rounded-md border border-input bg-surface px-3"
            />
          </label>
          <div className="flex flex-wrap gap-2">
            {(["merge", "discard", "hold"] as const).map((method) => (
              <Button
                key={method}
                type="button"
                className="min-h-11 min-w-11"
                disabled={busy}
                onClick={() => void act(method)}
              >
                {method}
              </Button>
            ))}
          </div>
        </>
      ) : integration.state === "held" || integration.state === "discarded" ? (
        <Button
          type="button"
          className="min-h-11 min-w-11 self-start"
          disabled={busy}
          onClick={() => void act("reopen")}
        >
          reopen
        </Button>
      ) : null}
      {message ? (
        <p role="status" className="text-label">
          {message}
        </p>
      ) : null}
      {inbox ? <span className="sr-only">受信箱からの操作</span> : null}
    </div>
  );
}

export function InboxIntegrationActions({ item }: { item: import("../../api/generated/types").InboxItem }) {
  const [note, setNote] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const mergePath = item.answer.native?.path;
  const run = async (method: "merge" | "discard" | "hold" | "reopen") => {
    if (!mergePath || !item.task) return;
    if ((method === "discard" || method === "hold") && !note.trim()) {
      setMessage("理由を入力してください");
      return;
    }
    setBusy(true);
    try {
      const base = mergePath.replace(/merge$/, "");
      await apiMutate("POST", method === "merge" ? mergePath : `${base}${method}`, {
        ...(method === "discard" || method === "hold" ? { note: note.trim() } : {}),
        ...(method === "discard" ? { confirm: true } : {}),
      });
      setMessage("操作を受け付けました");
    } catch {
      setMessage("操作に失敗しました");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="grid gap-2" data-testid="inbox-integration-actions">
      <label className="grid gap-1 text-label">
        discard / hold の理由
        <input
          value={note}
          onChange={(event) => setNote(event.target.value)}
          className="min-h-11 rounded-md border border-input bg-surface px-3"
        />
      </label>
      <div className="flex flex-wrap gap-2">
        {(["merge", "discard", "hold", "reopen"] as const).map((method) => (
          <Button
            key={method}
            type="button"
            className="min-h-11 min-w-11"
            disabled={busy || !mergePath}
            onClick={() => void run(method)}
          >
            {method}
          </Button>
        ))}
      </div>
      {message ? <p role="status">{message}</p> : null}
    </div>
  );
}
