// 発言の編集・撤回の操作と、「編集済み（rev n）」「撤回済み」・前の版の表示（ADR 2026-11-21-chat-discussion-mode D5・D9）。
// 操作を出すかは data/revision.ts の revisableBy が決める（CoS・取り次ぎ・他人の発言には出さない）。
// 409 は他の変更との衝突なので、再読込を促す（自動では送り直さない）。
import { useEffect, useId, useState } from "react";
import type { ChatEvent, ChatMessage, ChatMessageRevisedData } from "../../../api/generated/types";
import { Button } from "../../../components/ui/button";
import { fieldClassName, Input } from "../../../components/ui/input";
import { Notice } from "../../../components/ui/notice";
import { cn } from "../../../lib/utils";
import { listMessageRevisions } from "../data/client";
import {
  messageRevision,
  REVISION_CONFLICT_TEXT,
  REVISION_FORBIDDEN_TEXT,
  type RevisionChange,
  type RevisionOutcome,
  revisableBy,
} from "../data/revision";

/** 一覧から吹き出しへ渡す編集・撤回の口。無ければ操作を出さない。 */
export type MessageRevisionActions = {
  viewer: string | null;
  revise: (message: ChatMessage, change: RevisionChange) => Promise<RevisionOutcome>;
  /** 409 の後の再読込（thread と messages の取り直し）。 */
  reload: () => void;
  /** 前の版の読み出し（既定は GET …/revisions）。 */
  loadRevisions?: (message: ChatMessage) => Promise<ChatEvent[]>;
};

/** 「編集済み（rev n）」「撤回済み」の 1 行。変えていない発言には出さない。 */
export function revisionNoteText(message: Pick<ChatMessage, "revision" | "edited" | "retracted">): string | undefined {
  if (message.retracted) {
    const reason = message.retracted.reason?.trim();
    return reason ? `撤回済み（理由: ${reason}）` : "撤回済み";
  }
  if (message.edited) return `編集済み（rev ${messageRevision(message)}）`;
  return undefined;
}

export function RevisionNote({ message }: { message: Pick<ChatMessage, "revision" | "edited" | "retracted"> }) {
  const text = revisionNoteText(message);
  if (!text) return null;
  return (
    <p
      data-slot="chat-revision-note"
      data-revision-state={message.retracted ? "retracted" : "edited"}
      className="text-label text-muted-foreground"
    >
      {text}
    </p>
  );
}

/** 結果の知らせ。conflict は再読込の button を添える。 */
export function RevisionFeedback({ outcome, onReload }: { outcome: RevisionOutcome; onReload: () => void }) {
  if (outcome.kind === "ok") return null;
  if (outcome.kind === "conflict")
    return (
      <Notice
        data-slot="chat-revision-conflict"
        tone="warning"
        action={
          <Button size="sm" onClick={onReload}>
            再読込
          </Button>
        }
      >
        {REVISION_CONFLICT_TEXT}
      </Notice>
    );
  return (
    <Notice data-slot="chat-revision-error" tone="danger">
      {outcome.kind === "forbidden" ? REVISION_FORBIDDEN_TEXT : `操作できませんでした: ${outcome.detail}`}
    </Notice>
  );
}

type Phase = { kind: "idle" } | { kind: "editing"; text: string } | { kind: "retracting"; reason: string };

/** 自分の発言の「編集」「撤回」と、その入力欄。 */
export function RevisionControls({ message, actions }: { message: ChatMessage; actions: MessageRevisionActions }) {
  const allowed = revisableBy(message, actions.viewer);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<RevisionOutcome | null>(null);
  const fieldId = useId();
  if (!allowed.edit && !allowed.retract) return null;
  const submit = async (change: RevisionChange) => {
    setBusy(true);
    const result = await actions.revise(message, change);
    setBusy(false);
    setOutcome(result);
    if (result.kind === "ok") setPhase({ kind: "idle" });
  };
  const reload = () => {
    setOutcome(null);
    setPhase({ kind: "idle" });
    actions.reload();
  };
  return (
    <div data-slot="chat-revision-controls" className="flex w-full max-w-[85%] flex-col items-end gap-2">
      {phase.kind === "idle" ? (
        <div className="flex flex-wrap justify-end gap-2">
          {allowed.edit ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setOutcome(null);
                setPhase({ kind: "editing", text: message.text });
              }}
            >
              編集
            </Button>
          ) : null}
          {allowed.retract ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setOutcome(null);
                setPhase({ kind: "retracting", reason: "" });
              }}
            >
              撤回
            </Button>
          ) : null}
        </div>
      ) : null}
      {phase.kind === "editing" ? (
        <form
          className="flex w-full flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void submit({ kind: "edit", text: phase.text });
          }}
        >
          <label htmlFor={fieldId} className="text-label text-muted-foreground">
            発言を編集（前の本文は前の版に残ります）
          </label>
          <textarea
            id={fieldId}
            value={phase.text}
            onChange={(event) => setPhase({ kind: "editing", text: event.target.value })}
            rows={3}
            className={cn(fieldClassName, "min-h-20 w-full")}
          />
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setPhase({ kind: "idle" })}>
              やめる
            </Button>
            <Button size="sm" variant="primary" type="submit" disabled={busy || phase.text === message.text}>
              保存
            </Button>
          </div>
        </form>
      ) : null}
      {phase.kind === "retracting" ? (
        <form
          className="flex w-full flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void submit({ kind: "retract", reason: phase.reason });
          }}
        >
          <label htmlFor={fieldId} className="text-label text-muted-foreground">
            撤回の理由（任意。この発言を引いた要約・案は失効と表示されます）
          </label>
          <Input
            id={fieldId}
            value={phase.reason}
            maxLength={500}
            onChange={(event) => setPhase({ kind: "retracting", reason: event.target.value })}
          />
          <div className="flex justify-end gap-2">
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => setPhase({ kind: "idle" })}>
              やめる
            </Button>
            <Button size="sm" variant="destructive" type="submit" disabled={busy}>
              撤回する
            </Button>
          </div>
        </form>
      ) : null}
      {outcome ? <RevisionFeedback outcome={outcome} onReload={reload} /> : null}
    </div>
  );
}

const revisionKindLabel: Record<ChatMessageRevisedData["kind"], string> = { edit: "編集", retract: "撤回" };

/** 前の版 1 件（`message_revised` の data）。 */
export function RevisionEntry({ data, at }: { data: ChatMessageRevisedData; at: string }) {
  return (
    <li data-slot="chat-revision" data-revision={data.previous_revision} className="flex flex-col gap-1">
      <p className="text-label text-muted-foreground">
        {`rev ${data.previous_revision}（${at} に ${data.by} が${revisionKindLabel[data.kind]}）`}
      </p>
      <p className="whitespace-pre-wrap rounded-md border border-border px-3 py-2 text-body">
        {data.previous_text === "" ? "（本文なし）" : data.previous_text}
      </p>
    </li>
  );
}

function defaultLoadRevisions(message: ChatMessage): Promise<ChatEvent[]> {
  return listMessageRevisions(message.thread_id, message.id).then((page) => page.items);
}

/** 「前の版を見る」。開いたときと revision が変わったときに GET …/revisions を読む。 */
export function RevisionHistory({
  message,
  load = defaultLoadRevisions,
}: {
  message: ChatMessage;
  load?: (message: ChatMessage) => Promise<ChatEvent[]>;
}) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<ChatEvent[] | null>(null);
  const [failed, setFailed] = useState(false);
  const revision = messageRevision(message);
  const listId = useId();
  // biome-ignore lint/correctness/useExhaustiveDependencies: 開いたときと revision が変わったときだけ読み直す（load・message の identity では読まない）。
  useEffect(() => {
    if (!open || revision <= 1) return;
    let live = true;
    setFailed(false);
    load(message).then(
      (events) => live && setItems(events),
      () => live && setFailed(true),
    );
    return () => {
      live = false;
    };
  }, [open, revision, message.id]);
  if (revision <= 1) return null;
  return (
    <div data-slot="chat-revision-history" className="flex w-full max-w-[85%] flex-col items-end gap-2">
      <Button
        size="sm"
        variant="ghost"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        {open ? "前の版を閉じる" : "前の版を見る"}
      </Button>
      {open ? (
        <div id={listId} className="w-full">
          {failed ? (
            <p className="text-label text-danger">前の版を読めませんでした。</p>
          ) : items === null ? (
            <p className="text-label text-muted-foreground">前の版を読み込み中</p>
          ) : (
            <ol aria-label="前の版" className="flex flex-col gap-2">
              {items.map((event) => (
                <RevisionEntry key={event.id} data={event.data as ChatMessageRevisedData} at={event.at} />
              ))}
            </ol>
          )}
        </div>
      ) : null}
    </div>
  );
}
