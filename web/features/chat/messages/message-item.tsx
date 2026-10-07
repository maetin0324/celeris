// 吹き出し 1 件: 人の発言・CoS の返事（Markdown・tool・添付・カード）・システム・送信中の発言。
import type { ReactNode } from "react";
import type { ChatAttachment, ChatCard, ChatMessage } from "../../../api/generated/types";
import { Markdown } from "../../../components/content/markdown";
import { Button } from "../../../components/ui/button";
import { cn } from "../../../lib/utils";
import { pendingHumanCount } from "../cards/card-model";
import type { DraftMessage, PendingMessage, ToolEntry } from "../data/reducer";
import { AttachmentList, ToolCallList } from "./parts";

/** カードを差し込む slot。cards 葉の部品を home が渡す。 */
export type RenderCards = (cards: ChatCard[], message: ChatMessage) => ReactNode;

const stateNote: Partial<Record<ChatMessage["state"], string>> = {
  queued: "順番待ち",
  cancelled: "取り消し済み",
  interrupted: "中断",
  failed: "失敗",
};

function Bubble({
  speaker,
  busy,
  children,
  label,
}: {
  speaker: ChatMessage["role"];
  busy?: boolean;
  children: ReactNode;
  label: string;
}) {
  return (
    <article
      aria-label={label}
      aria-busy={busy ? true : undefined}
      data-role={speaker}
      className={cn(
        "flex min-w-0 flex-col gap-2",
        speaker === "user" && "items-end",
        speaker === "system" && "items-center",
      )}
    >
      {children}
    </article>
  );
}

const bodyClass: Record<ChatMessage["role"], string> = {
  user: "max-w-[85%] rounded-lg bg-secondary px-3 py-2 text-body text-secondary-foreground",
  assistant: "w-full max-w-full text-body text-foreground",
  system: "max-w-full rounded-md bg-muted px-3 py-1 text-label text-muted-foreground",
};

function Body({ speaker, text, streaming }: { speaker: ChatMessage["role"]; text: string; streaming?: boolean }) {
  if (text === "" && !streaming) return null;
  return (
    <div data-slot="chat-body" className={cn("min-w-0 break-words", bodyClass[speaker])}>
      {speaker === "assistant" ? (
        <Markdown source={text} links="target" />
      ) : (
        <p className="whitespace-pre-wrap">{text}</p>
      )}
      {streaming ? (
        <span
          data-slot="chat-caret"
          aria-hidden="true"
          className="ml-0.5 inline-block h-4 w-1 animate-pulse bg-running align-middle"
        />
      ) : null}
    </div>
  );
}

const roleName: Record<ChatMessage["role"], string> = { user: "あなた", assistant: "CoS", system: "システム" };

/** 折りたたんだ system 行の 1 行目（本文の先頭行。無ければカードの題名の数）。 */
export function foldedSummary(message: Pick<ChatMessage, "text" | "cards">): string {
  const first = message.text.split("\n").find((line) => line.trim() !== "") ?? "";
  if (first !== "") return first;
  return message.cards.length > 0 ? `${message.cards.length} 件の知らせ` : "システムの記録";
}

export function MessageItem({
  message,
  streaming,
  tools = [],
  attachments = {},
  renderCards,
}: {
  message: ChatMessage;
  streaming: boolean;
  tools?: ToolEntry[];
  attachments?: Record<string, ChatAttachment | undefined>;
  renderCards?: RenderCards;
}) {
  const note = streaming ? undefined : stateNote[message.state];
  // ADR 2026-10-07-cos-inbox-thread-conversation D5: a system row with no card waiting on the
  // person (triage input lines, notices, closed hand-offs) folds into one line so the rows that
  // need an answer stand out. Rows with a human wait, and every human/CoS message, stay open.
  if (message.role === "system" && !streaming && pendingHumanCount(message.cards) === 0) {
    return (
      <Bubble speaker="system" label={roleName.system}>
        <details data-slot="chat-system-folded" className="w-full min-w-0 max-w-full">
          <summary className="cursor-pointer truncate rounded-md bg-muted px-3 py-1 text-label text-muted-foreground">
            {foldedSummary(message)}
          </summary>
          <div className="mt-2 flex min-w-0 flex-col items-center gap-2">
            <Body speaker="system" text={message.text} />
            {note ? <p className="text-label text-muted-foreground">{note}</p> : null}
            {message.cards.length > 0 && renderCards ? (
              <div data-slot="chat-cards" className="w-full min-w-0">
                {renderCards(message.cards, message)}
              </div>
            ) : null}
          </div>
        </details>
      </Bubble>
    );
  }
  return (
    <Bubble speaker={message.role} busy={streaming} label={roleName[message.role]}>
      <ToolCallList tools={tools} />
      <Body speaker={message.role} text={message.text} streaming={streaming} />
      <AttachmentList ids={message.attachment_ids} attachments={attachments} />
      {note ? (
        <p className={cn("text-label", message.state === "failed" ? "text-danger" : "text-muted-foreground")}>{note}</p>
      ) : null}
      {message.cards.length > 0 && renderCards ? (
        <div data-slot="chat-cards" className="w-full min-w-0">
          {renderCards(message.cards, message)}
        </div>
      ) : null}
    </Bubble>
  );
}

/** message event が来る前の streaming 本文。message が来たら reducer が消すので二重にならない。 */
export function DraftItem({ draft, tools = [] }: { draft: DraftMessage; tools?: ToolEntry[] }) {
  return (
    <Bubble speaker="assistant" busy label={roleName.assistant}>
      <ToolCallList tools={tools} />
      <Body speaker="assistant" text={draft.text} streaming />
    </Bubble>
  );
}

/** 送信中・送信失敗の人の発言。確定した message が来たら reducer が消す。 */
export function PendingItem({
  pending,
  onRetry,
  onDiscard,
}: {
  pending: PendingMessage;
  onRetry?: (pending: PendingMessage) => void;
  onDiscard?: (pending: PendingMessage) => void;
}) {
  const failed = pending.state === "failed";
  return (
    <Bubble speaker="user" busy={!failed} label={roleName.user}>
      <Body speaker="user" text={pending.text} />
      {pending.attachment_ids.length > 0 ? (
        <p className="text-label text-muted-foreground">{`添付 ${pending.attachment_ids.length} 件`}</p>
      ) : null}
      {failed ? (
        <div className="flex flex-wrap items-center justify-end gap-2">
          <p className="text-label text-danger">{`送信できませんでした${pending.error ? `: ${pending.error}` : ""}`}</p>
          {onRetry ? (
            <Button size="sm" onClick={() => onRetry(pending)}>
              再送
            </Button>
          ) : null}
          {onDiscard ? (
            <Button size="sm" variant="ghost" onClick={() => onDiscard(pending)}>
              破棄
            </Button>
          ) : null}
        </div>
      ) : (
        <p className="text-label text-muted-foreground">送信中</p>
      )}
    </Bubble>
  );
}
