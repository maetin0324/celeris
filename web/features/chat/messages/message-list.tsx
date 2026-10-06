// メッセージ一覧（ADR 2026-10-05-cos-chat-home D5 の 2・3 項目目）。
// - 本文は reducer の selectTimeline を描く。streaming 中の本文と確定 message は同じ key の 1 項目（二重にしない）。
// - 下端を見ているときだけ追従し、上へ scroll 中は位置を保って「最新へ」と未読件数を出す（logic.ts followReducer）。
// - 上端に近づくか「以前のメッセージ」を押すと before_seq で古い頁を足し、足した高さだけずらして位置を保つ。
// - 一覧そのものは live region にしない。確定した返事だけを別の live region でまとめて読む。

import { type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ChatAttachment } from "../../../api/generated/types";
import { Button } from "../../../components/ui/button";
import { cn } from "../../../lib/utils";
import {
  type ChatState,
  type PendingMessage,
  selectActiveRun,
  selectTimeline,
  type TimelineItem,
} from "../data/reducer";
import {
  type AnnounceState,
  type FollowAction,
  type FollowState,
  followReducer,
  groupTools,
  initialFollowState,
  isAtBottom,
  nextAnnouncement,
  type ScrollMetrics,
  scrollTopAfterPrepend,
} from "./logic";
import { DraftItem, MessageItem, PendingItem, type RenderCards } from "./message-item";
import { JumpToLatest, StatusLine, ToolCallList } from "./parts";

/** 上端からこの px 以内に来たら古い頁を読む。 */
const TOP_LOAD_PX = 64;

export type MessageListProps = {
  state: ChatState;
  attachments?: Record<string, ChatAttachment | undefined>;
  renderCards?: RenderCards;
  /** 古い頁があるか（useOlderMessages）。 */
  hasOlder?: boolean;
  loadingOlder?: boolean;
  onLoadOlder?: () => Promise<unknown> | undefined;
  onRetryPending?: (pending: PendingMessage) => void;
  onDiscardPending?: (pending: PendingMessage) => void;
  /** 空の会話に出す内容。 */
  empty?: ReactNode;
  className?: string;
};

function metricsOf(element: HTMLElement): ScrollMetrics {
  return { scrollTop: element.scrollTop, scrollHeight: element.scrollHeight, clientHeight: element.clientHeight };
}

export function MessageList({
  state,
  attachments = {},
  renderCards,
  hasOlder = false,
  loadingOlder = false,
  onLoadOlder,
  onRetryPending,
  onDiscardPending,
  empty,
  className,
}: MessageListProps) {
  const items = useMemo(() => selectTimeline(state), [state]);
  const tools = useMemo(() => groupTools(state), [state]);
  const active = selectActiveRun(state);
  const status = state.status && (!state.status.run_id || active?.id === state.status.run_id) ? state.status : null;

  const scroller = useRef<HTMLElement>(null);
  const follow = useRef<FollowState>(initialFollowState());
  const [view, setView] = useState({ atBottom: true, unread: 0 });
  const prependAnchor = useRef<ScrollMetrics | null>(null);
  const announce = useRef<AnnounceState | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const apply = useCallback((action: FollowAction) => {
    const decision = followReducer(follow.current, action);
    follow.current = decision.state;
    setView((prev) =>
      prev.atBottom === decision.state.atBottom && prev.unread === decision.state.unread
        ? prev
        : { atBottom: decision.state.atBottom, unread: decision.state.unread },
    );
    const element = scroller.current;
    if (decision.scrollToBottom && element) element.scrollTop = element.scrollHeight;
  }, []);

  // 描画の直後（paint 前）に位置を決める: 古い頁の足し込み → 追従の順。
  useLayoutEffect(() => {
    const element = scroller.current;
    const anchor = prependAnchor.current;
    if (element && anchor && element.scrollHeight !== anchor.scrollHeight) {
      element.scrollTop = scrollTopAfterPrepend(anchor, element.scrollHeight);
      prependAnchor.current = null;
    }
    apply({ type: "timeline", items });
  }, [items, apply]);

  useEffect(() => {
    const next = nextAnnouncement(announce.current, items);
    if (next !== announce.current && next.text !== "") setAnnouncement(next.text);
    announce.current = next;
  }, [items]);

  const loadOlder = useCallback(() => {
    const element = scroller.current;
    if (!onLoadOlder || loadingOlder || !hasOlder) return;
    prependAnchor.current = element ? metricsOf(element) : null;
    const result = onLoadOlder();
    // 失敗・空振り（高さが変わらない）なら anchor を捨てる。
    void Promise.resolve(result).finally(() => {
      const current = scroller.current;
      if (current && prependAnchor.current && current.scrollHeight === prependAnchor.current.scrollHeight)
        prependAnchor.current = null;
    });
  }, [hasOlder, loadingOlder, onLoadOlder]);

  const onScroll = useCallback(() => {
    const element = scroller.current;
    if (!element) return;
    const atBottom = isAtBottom(metricsOf(element));
    if (atBottom !== follow.current.atBottom) apply({ type: "scrolled", atBottom });
    if (element.scrollTop <= TOP_LOAD_PX) loadOlder();
  }, [apply, loadOlder]);

  return (
    <div data-slot="chat-messages" className={cn("relative flex min-h-0 min-w-0 flex-1 flex-col", className)}>
      <section
        ref={scroller}
        onScroll={onScroll}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: 会話の scroll 領域を keyboard で読めるようにする（WCAG 2.1.1）。
        tabIndex={0}
        aria-label="会話"
        className="min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-contain px-3 py-4 focus-visible:outline-2 focus-visible:outline-ring"
      >
        {hasOlder ? (
          <div className="mb-4 flex justify-center">
            <Button size="sm" variant="ghost" onClick={loadOlder} disabled={loadingOlder} aria-busy={loadingOlder}>
              {loadingOlder ? "以前のメッセージを読み込み中" : "以前のメッセージ"}
            </Button>
          </div>
        ) : null}
        {items.length === 0 && !status ? empty : null}
        <ol className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-4">
          {items.map((item) => (
            <li key={item.key} data-key={item.key} className="min-w-0">
              {renderItem(item, { attachments, renderCards, tools: tools.byMessage, onRetryPending, onDiscardPending })}
            </li>
          ))}
          {tools.loose.length > 0 || status ? (
            <li key="activity" className="flex min-w-0 flex-col gap-2">
              <ToolCallList tools={tools.loose} />
              {status ? <StatusLine status={status} /> : null}
            </li>
          ) : null}
        </ol>
      </section>
      {!view.atBottom ? (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
          <div className="pointer-events-auto">
            <JumpToLatest unread={view.unread} onClick={() => apply({ type: "jumped" })} />
          </div>
        </div>
      ) : null}
      <div aria-live="polite" aria-atomic="true" className="sr-only">
        {announcement}
      </div>
    </div>
  );
}

function renderItem(
  item: TimelineItem,
  context: {
    attachments: Record<string, ChatAttachment | undefined>;
    renderCards?: RenderCards;
    tools: ReturnType<typeof groupTools>["byMessage"];
    onRetryPending?: (pending: PendingMessage) => void;
    onDiscardPending?: (pending: PendingMessage) => void;
  },
): ReactNode {
  switch (item.kind) {
    case "message":
      return (
        <MessageItem
          message={item.message}
          streaming={item.streaming}
          tools={context.tools[item.message.id]}
          attachments={context.attachments}
          renderCards={context.renderCards}
        />
      );
    case "draft":
      return <DraftItem draft={item.draft} tools={context.tools[item.draft.id]} />;
    case "pending":
      return (
        <PendingItem pending={item.pending} onRetry={context.onRetryPending} onDiscard={context.onDiscardPending} />
      );
    default:
      return null;
  }
}
