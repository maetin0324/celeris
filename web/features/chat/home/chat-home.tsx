import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { inboxItemsQuery } from "../../../api/queries/inbox-notifications";
import { ScreenFrame } from "../../../components/shell/screen-frame";
import { ChatCardItem } from "../cards/chat-card";
import { ChatComposer, INBOX_COMPOSER_PLACEHOLDER } from "../composer/chat-composer";
import { useChatSession } from "../data/use-chat-session";
import { attachmentIdsOf, useAttachmentMap, useOlderMessages } from "../messages/history";
import { MessageList } from "../messages/message-list";
import { ThreadsModel } from "../threads/model";
import { ChatThreads } from "../threads/threads";

/** URL は選択した会話の正本。未指定のときだけ直近の人の会話を選ぶ。 */
export function ChatHome() {
  const { thread } = useSearch({ from: "/" });
  const navigate = useNavigate();
  const inbox = useQuery(inboxItemsQuery());
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
    const human = threads.items.find((item) => item.kind === "human");
    if (human) {
      void navigate({ to: "/", search: { thread: human.id }, replace: true });
      return;
    }
    void model
      .create((id) => void navigate({ to: "/", search: { thread: id }, replace: true }))
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

  const select = (id: string) => void navigate({ to: "/", search: { thread: id } });
  return (
    <ScreenFrame title="ホーム" route="/">
      <div
        ref={frame}
        data-chat-home
        className="flex min-w-0 flex-col overflow-hidden rounded-lg border border-border bg-background md:flex-row"
        style={{ height }}
      >
        <div className="shrink-0 border-b border-border p-2 md:flex md:h-full md:min-h-0 md:flex-col md:border-r md:border-b-0 md:p-0">
          <ChatThreads
            model={model}
            selectedThreadId={thread}
            inboxWaitingCount={inbox.data?.counts.total ?? 0}
            onSelectThread={select}
          />
        </div>
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          {thread ? (
            <ChatConversation key={thread} threadId={thread} />
          ) : (
            <p role="status" className="flex-1 p-4 text-muted-foreground">
              会話を準備しています
            </p>
          )}
        </div>
      </div>
    </ScreenFrame>
  );
}

function ChatConversation({ threadId }: { threadId: string }) {
  const { session, snapshot } = useChatSession(threadId);
  const chat = snapshot.chat;
  const history = useOlderMessages(chat, session.dispatch);
  const attachments = useAttachmentMap(attachmentIdsOf(chat));
  return (
    <>
      {snapshot.loadError ? (
        <p role="alert" className="p-3 text-destructive">
          会話を読み込めませんでした。
        </p>
      ) : null}
      <MessageList
        state={chat}
        attachments={attachments}
        renderCards={(cards) => cards.map((card) => <ChatCardItem key={`${card.kind}:${card.id}`} card={card} />)}
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
      <ChatComposer
        threadId={threadId}
        session={session}
        snapshot={snapshot}
        placeholder={chat.thread?.kind === "inbox" ? INBOX_COMPOSER_PLACEHOLDER : undefined}
      />
    </>
  );
}
