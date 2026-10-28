import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import type { ChatMessage, ChatThread } from "../../../api/generated/types";
import { ScreenFrame } from "../../../components/shell/screen-frame";
import { Button } from "../../../components/ui/button";
import { CardLink, ChatCardItem } from "../cards/chat-card";
import { useChatSession } from "../data/use-chat-session";
import { attachmentIdsOf, useAttachmentMap, useOlderMessages } from "../messages/history";
import { MessageList } from "../messages/message-list";
import { ThreadsModel } from "../threads/model";
import { ConversationInput } from "./conversation-input";

/**
 * ホームは統括 CoS を開く（ADR 2026-10-28-single-human-window-general-cos D5）。人は統括で話し、案件 CoS の件は
 * 統括の札（escalation）で届く。案件スレッドは札の「元のスレッドを開く」と案件の画面から `?thread=` で読むだけで、
 * スレッドを選ぶ一覧は置かない。
 */
export function ChatHome() {
  const { thread } = useSearch({ from: "/" });
  const navigate = useNavigate();
  const [model] = useState(() => new ThreadsModel());
  const [loaded, setLoaded] = useState(false);
  const threads = useSyncExternalStore(model.subscribe, model.snapshot, model.snapshot);
  const choosing = useRef(false);
  const frame = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(560);

  useEffect(() => {
    void model.load().then(() => setLoaded(true));
  }, [model]);

  useEffect(() => {
    if (!loaded || thread || threads.loading || threads.error || choosing.current) return;
    choosing.current = true;
    const general = model.general();
    if (general) {
      choosing.current = false;
      void navigate({ to: "/", search: { thread: general.id }, replace: true });
      return;
    }
    void model
      .open(null, (id) => void navigate({ to: "/", search: { thread: id }, replace: true }))
      .finally(() => {
        choosing.current = false;
      });
  }, [loaded, thread, threads, model, navigate]);

  // shell のヘッダ・下部タブバー・safe area を避け、本文だけをスクロールさせる。
  useLayoutEffect(() => {
    const element = frame.current;
    if (!element) return;
    const resize = () => {
      const main = element.closest("main");
      const column = main?.parentElement;
      const mainBottom = main ? Number.parseFloat(getComputedStyle(main).paddingBottom) || 0 : 0;
      const shellBottom = column ? Number.parseFloat(getComputedStyle(column).paddingBottom) || 0 : 0;
      const viewport = window.visualViewport;
      const visibleBottom = viewport ? viewport.offsetTop + viewport.height : window.innerHeight;
      // keyboard が layout viewport の下部タブを覆う間は、その退避余白を重ねて引かない。
      const keyboardInset = Math.max(0, window.innerHeight - visibleBottom);
      const tabInset = Math.max(0, shellBottom - keyboardInset);
      setHeight(Math.max(160, Math.floor(visibleBottom - element.getBoundingClientRect().top - mainBottom - tabInset)));
    };
    resize();
    window.addEventListener("resize", resize);
    window.visualViewport?.addEventListener("resize", resize);
    window.visualViewport?.addEventListener("scroll", resize);
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", resize);
      window.visualViewport?.removeEventListener("resize", resize);
      window.visualViewport?.removeEventListener("scroll", resize);
    };
  }, []);

  const backToGeneral = () => {
    const general = model.general();
    void navigate({ to: "/", search: general ? { thread: general.id } : {} });
  };
  return (
    <ScreenFrame title="ホーム" route="/">
      <div
        ref={frame}
        data-chat-home
        className="flex min-w-0 flex-col overflow-hidden rounded-lg border border-border bg-background"
        style={{ height }}
      >
        {thread ? (
          <ChatConversation key={thread} threadId={thread} model={model} onBackToGeneral={backToGeneral} />
        ) : threads.error ? (
          <p role="alert" className="flex-1 p-4 text-destructive">
            {threads.error}
          </p>
        ) : (
          <p role="status" className="flex-1 p-4 text-muted-foreground">
            統括 CoS を開いています
          </p>
        )}
      </div>
    </ScreenFrame>
  );
}

/** 会話の上の帯。統括なら題名だけ、案件スレッドなら読む用であることと統括へ戻る導線を出す。 */
export function ThreadBar({ thread, onBackToGeneral }: { thread: ChatThread | null; onBackToGeneral: () => void }) {
  if (!thread) return null;
  if (thread.kind === "general")
    return (
      <div data-thread-bar="general" className="shrink-0 border-b border-border px-3 py-2 font-medium">
        {thread.title}
      </div>
    );
  return (
    <div
      data-thread-bar={thread.kind}
      className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-2"
    >
      <span className="min-w-0 truncate font-medium">{thread.title}</span>
      <span className="text-label text-muted-foreground">読む用です。話すときは統括 CoS で。</span>
      <Button size="sm" className="min-h-11" onClick={onBackToGeneral}>
        統括 CoS に戻る
      </Button>
    </div>
  );
}

/** 札（案件 CoS が人へ回した件）から、その件の案件スレッドへ読みに行くリンク。 */
export function EscalationSourceLink({ message }: { message: Pick<ChatMessage, "escalation"> }) {
  const escalation = message.escalation;
  if (!escalation) return null;
  return (
    <p data-escalation-state={escalation.open ? "open" : "resolved"} className="text-label text-muted-foreground">
      {escalation.open ? "人の判断を待っています。" : "解消しました。"}
      <CardLink href={`/?thread=${encodeURIComponent(escalation.source_thread_id)}`}>元のスレッドを開く</CardLink>
    </p>
  );
}

function ChatConversation({
  threadId,
  model,
  onBackToGeneral,
}: {
  threadId: string;
  model: ThreadsModel;
  onBackToGeneral: () => void;
}) {
  const { session, snapshot } = useChatSession(threadId);
  const chat = snapshot.chat;
  const lastSeq = latestSeq(chat.messages);
  // D5: what is on screen is read; the list's unread count follows.
  useEffect(() => {
    if (lastSeq > 0) void model.markRead(threadId, lastSeq);
  }, [model, threadId, lastSeq]);
  const history = useOlderMessages(chat, session.dispatch);
  const attachments = useAttachmentMap(attachmentIdsOf(chat));
  return (
    <>
      <ThreadBar thread={chat.thread ?? null} onBackToGeneral={onBackToGeneral} />
      {snapshot.loadError ? (
        <p role="alert" className="p-3 text-destructive">
          会話を読み込めませんでした。
        </p>
      ) : null}
      <MessageList
        state={chat}
        attachments={attachments}
        renderCards={(cards, message) => (
          <>
            {cards.map((card) => (
              <ChatCardItem key={`${card.kind}:${card.id}`} card={card} />
            ))}
            <EscalationSourceLink message={message} />
          </>
        )}
        hasOlder={history.hasOlder}
        loadingOlder={history.loading}
        onLoadOlder={history.loadOlder}
        onRetryPending={
          (pending) =>
            void session
              .send({
                client_message_id: pending.client_message_id,
                text: pending.text,
                attachment_ids: pending.attachment_ids,
                mode: pending.mode,
              })
              .catch(() => {}) // session が同じ pending に失敗を記録する。
        }
        onDiscardPending={(pending) =>
          session.dispatch({ type: "pending_discard", clientMessageId: pending.client_message_id })
        }
        empty={<p className="mx-auto max-w-3xl text-muted-foreground">CoS にメッセージを送って会話を始めましょう。</p>}
      />
      <ConversationInput thread={chat.thread ?? undefined} threadId={threadId} session={session} snapshot={snapshot} />
    </>
  );
}

/** The highest seq among the loaded messages (0 when empty). */
export function latestSeq(messages: Record<string, ChatMessage>): number {
  let max = 0;
  for (const message of Object.values(messages)) if (message.seq > max) max = message.seq;
  return max;
}
