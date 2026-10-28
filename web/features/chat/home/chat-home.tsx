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
      // 枠は負の margin で main の余白に張り出す。下の余白から張り出した分を差し引く。
      const frameBottom = Number.parseFloat(getComputedStyle(element).marginBottom) || 0;
      const mainBottom = main
        ? Math.max(0, (Number.parseFloat(getComputedStyle(main).paddingBottom) || 0) + frameBottom)
        : 0;
      const shellBottom = column ? Number.parseFloat(getComputedStyle(column).paddingBottom) || 0 : 0;
      // main の後ろに続く列の中身（Console の置き場の枠線）の分も引き、ページを 1px も伸ばさない。
      const afterMain =
        main && column
          ? Math.max(0, column.getBoundingClientRect().bottom - shellBottom - main.getBoundingClientRect().bottom)
          : 0;
      const viewport = window.visualViewport;
      const visibleBottom = viewport ? viewport.offsetTop + viewport.height : window.innerHeight;
      // keyboard が layout viewport の下部タブを覆う間は、その退避余白を重ねて引かない。
      const keyboardInset = Math.max(0, window.innerHeight - visibleBottom);
      const tabInset = Math.max(0, shellBottom - keyboardInset);
      setHeight(
        Math.max(
          160,
          Math.floor(visibleBottom - element.getBoundingClientRect().top - mainBottom - afterMain - tabInset),
        ),
      );
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
    // 会話の縦の長さを優先する: 見出しは読み上げだけに残し、枠は main の余白まで広げる。
    <ScreenFrame title="ホーム" route="/" titleHidden>
      <div
        ref={frame}
        data-chat-home
        className="-m-4 flex min-w-0 flex-col overflow-hidden bg-background md:-m-6"
        style={{ height }}
      >
        {thread ? (
          <ChatConversation key={thread} threadId={thread} model={model} onBackToGeneral={backToGeneral} />
        ) : threads.error ? (
          <p role="alert" className="flex-1 p-4 text-destructive">
            {threads.error}
          </p>
        ) : (
          <p role="status" className="sr-only">
            統括 CoS を開いています
          </p>
        )}
      </div>
    </ScreenFrame>
  );
}

/**
 * 会話の上の帯。統括なら題名を読み上げだけに残して帯を出さない。案件スレッドなら読む用であることと統括へ戻る導線を
 * 1 行に詰めて出す（電話幅は「読む用」だけを見せ、全文は読み上げに残す）。
 */
export function ThreadBar({ thread, onBackToGeneral }: { thread: ChatThread | null; onBackToGeneral: () => void }) {
  if (!thread) return null;
  if (thread.kind === "general")
    return (
      <h2 data-thread-bar="general" className="sr-only">
        {thread.title}
      </h2>
    );
  return (
    <div
      data-thread-bar={thread.kind}
      className="flex min-w-0 shrink-0 items-center gap-2 border-b border-border bg-surface px-3 py-0.5"
    >
      <span className="min-w-0 flex-1 truncate font-medium" title={thread.title}>
        {thread.title}
      </span>
      <span className="shrink-0 whitespace-nowrap text-label text-muted-foreground">
        <span aria-hidden="true" className="md:hidden">
          読む用
        </span>
        <span className="max-md:sr-only">読む用です。話すときは統括 CoS で。</span>
      </span>
      <Button size="sm" className="min-h-11 shrink-0" onClick={onBackToGeneral}>
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
