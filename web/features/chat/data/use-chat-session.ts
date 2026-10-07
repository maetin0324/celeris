// 1 thread のチャット状態を購読する hook。thread を変えると controller を作り直す。
// 画面が前面に戻ったら stream を張り直す（cursor から継ぐ。切断は run の停止扱いにしない）。

import { useEffect, useState, useSyncExternalStore } from "react";
import { type ChatSession, type ChatSessionOptions, type ChatSessionSnapshot, createChatSession } from "./session";

export function useChatSession(
  threadId: string,
  options: Omit<ChatSessionOptions, "threadId"> = {},
): { session: ChatSession; snapshot: ChatSessionSnapshot } {
  // options は初回だけ読む（試験で api を差し替える用途）。
  const [initialOptions] = useState(options);
  const [session, setSession] = useState(() => createChatSession({ ...initialOptions, threadId }));
  const current = session.getSnapshot().chat.threadId === threadId ? session : undefined;

  useEffect(() => {
    if (current) return;
    setSession(createChatSession({ ...initialOptions, threadId }));
  }, [current, initialOptions, threadId]);

  useEffect(() => {
    session.start();
    const onVisible = () => {
      if (document.visibilityState === "visible") session.reconnect();
    };
    const onOnline = () => session.reconnect();
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener("online", onOnline);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener("online", onOnline);
      session.stop();
    };
  }, [session]);

  const snapshot = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  return { session, snapshot };
}
