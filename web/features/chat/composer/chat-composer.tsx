import {
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Button } from "../../../components/ui/button";
import { cancelMessage, resumeQueue, stopRun } from "../data/client";
import { selectActiveRun } from "../data/reducer";
import { newClientId } from "../data/send";
import type { ChatSession, ChatSessionSnapshot } from "../data/session";
import { createUploadQueue, pastedFiles, shouldSendOnEnter, type UploadApi } from "./upload-queue";

export type ChatComposerProps = {
  threadId: string;
  session: ChatSession;
  snapshot: ChatSessionSnapshot;
  uploadApi?: UploadApi;
};

function bytes(size: number) {
  return size < 1024 ? `${size} B` : `${(size / 1024).toFixed(1)} KiB`;
}

export function ChatComposer({ threadId, session, snapshot, uploadApi }: ChatComposerProps) {
  const queue = useMemo(() => createUploadQueue(threadId, uploadApi), [threadId, uploadApi]);
  const uploads = useSyncExternalStore(queue.subscribe, queue.getSnapshot, queue.getSnapshot);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const composing = useRef(false);
  const sending = useRef(false);
  const sentFileIds = useRef<Set<string>>(new Set());
  const retryKey = useRef<{ fingerprint: string; id: string } | null>(null);

  useEffect(() => () => queue.dispose(), [queue]);

  const chat = snapshot.chat;
  const activeRun = selectActiveRun(chat);
  // キュー（停止含む）は chat.queue が正本（snapshot でも queue event でも導かれる）。
  const paused = chat.queue.paused;
  const queued = chat.queue.message_ids.map((id) => chat.messages[id]).filter((message) => message?.state === "queued");
  const uploading = uploads.some((item) => item.state !== "ready");
  const canSend = !busy && !uploading && (text.trim().length > 0 || uploads.length > 0);
  const disconnected =
    snapshot.stream === "reconnecting" ||
    snapshot.stream === "closed" ||
    snapshot.stream === "unauthorized" ||
    snapshot.loadError != null;
  const quotaWaiting = activeRun?.reason?.includes("quota") ?? false;
  const latestRun = Object.values(chat.runs)
    .sort((a, b) => (a.started_at ?? "").localeCompare(b.started_at ?? ""))
    .at(-1);
  const failed = !activeRun && latestRun?.state === "failed";

  const send = async (mode: "queue" | "interrupt") => {
    if (!canSend || sending.current) return;
    setActionError("");
    const sentText = text;
    const sentUploads = uploads;
    const ids = sentUploads.map((item) => item.id);
    const attachmentIds = sentUploads.map((item) => item.attachment?.id).filter((id): id is string => !!id);
    const fingerprint = JSON.stringify([threadId, sentText, attachmentIds, mode, paused]);
    const clientId = retryKey.current?.fingerprint === fingerprint ? retryKey.current.id : newClientId();
    retryKey.current = { fingerprint, id: clientId };
    // 送信中の印は id を作り終えてから立てる（id の生成が例外になっても『送信中』のまま固まらない）。
    sending.current = true;
    setBusy(true);
    sentFileIds.current = new Set(ids);
    try {
      await session.send({
        client_message_id: clientId,
        text: sentText,
        attachment_ids: attachmentIds,
        mode,
        resume_queue: paused,
      });
      retryKey.current = null;
      setText((current) => (current === sentText ? "" : current));
      queue.clearSent(ids);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      sentFileIds.current = new Set();
      sending.current = false;
      setBusy(false);
    }
  };
  const action = async (perform: () => Promise<unknown>) => {
    setActionError("");
    try {
      await perform();
      await session.refresh();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    }
  };
  const drop = (event: DragEvent) => {
    event.preventDefault();
    queue.add(pastedFiles(event.dataTransfer));
  };
  const paste = (event: ClipboardEvent) => {
    const files = pastedFiles(event.clipboardData);
    if (files.length) {
      event.preventDefault();
      queue.add(files);
    }
  };
  const keyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      shouldSendOnEnter({
        key: event.key,
        shiftKey: event.shiftKey,
        isComposing: event.nativeEvent.isComposing || composing.current,
        keyCode: event.keyCode,
      })
    ) {
      event.preventDefault();
      void send("queue");
    }
  };

  // ChatHome が visualViewport と shell の余白から枠を測る。入力欄はその末尾に置き、本文を覆わない。
  return (
    <section
      aria-label="メッセージ入力"
      onDragOver={(event) => event.preventDefault()}
      onDrop={drop}
      className="z-20 min-w-0 shrink-0 border-t border-border bg-background p-2 pb-[max(0.5rem,env(safe-area-inset-bottom))] md:p-3"
    >
      <div aria-live="polite" className="text-sm text-muted-foreground">
        {activeRun?.state === "stopping"
          ? "停止を要求中です"
          : quotaWaiting
            ? "利用枠の回復を待っています"
            : disconnected
              ? "接続が切れています。再接続を試みています"
              : failed
                ? "実行に失敗しました"
                : paused
                  ? "キューを停止中です"
                  : activeRun
                    ? "実行中。送信するとキューに追加します"
                    : ""}
      </div>
      {queued.length > 0 && (
        <ol aria-label={paused ? "停止中の送信待ち" : "送信待ち"} className="max-h-32 overflow-y-auto">
          {queued.map((message, index) => (
            <li key={message.id} className="flex min-w-0 items-center gap-2 border-b border-border py-1">
              <span className="min-w-0 flex-1 truncate text-sm">
                {index + 1}. {message.text || "添付ファイル"}
              </span>
              <Button
                size="sm"
                variant="ghost"
                aria-label={`${index + 1}番目を取り消す`}
                onClick={() => void action(() => cancelMessage(threadId, message.id))}
              >
                取消
              </Button>
            </li>
          ))}
        </ol>
      )}
      {uploads.length > 0 && (
        <ul aria-label="添付ファイル" className="max-h-40 overflow-y-auto">
          {uploads.map((item) => (
            <li key={item.id} className="flex min-w-0 items-center gap-2 py-1">
              {item.preview && <img src={item.preview} alt="" className="h-11 w-11 shrink-0 rounded object-cover" />}
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm">
                  {item.file.name} · {bytes(item.file.size)}
                </div>
                {item.state === "uploading" && (
                  <progress
                    aria-label={`${item.file.name} のアップロード`}
                    value={item.progress ?? undefined}
                    max={100}
                    className="w-full"
                  />
                )}
                {item.state === "failed" && (
                  <span role="alert" className="text-sm text-destructive">
                    失敗: {item.error}
                  </span>
                )}
              </div>
              {item.state === "failed" && (
                <Button
                  size="sm"
                  variant="ghost"
                  aria-label={`${item.file.name} を再試行`}
                  onClick={() => queue.retry(item.id)}
                >
                  再試行
                </Button>
              )}
              <Button
                size="sm"
                variant="ghost"
                aria-label={`${item.file.name} を取り消す`}
                disabled={busy && sentFileIds.current.has(item.id)}
                onClick={() => queue.remove(item.id)}
              >
                取消
              </Button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex min-w-0 items-end gap-2">
        <input
          ref={input}
          type="file"
          multiple
          className="hidden"
          aria-label="添付ファイルを選択"
          onChange={(event) => {
            queue.add(Array.from(event.currentTarget.files ?? []));
            event.currentTarget.value = "";
          }}
        />
        <Button size="icon" aria-label="ファイルを添付" onClick={() => input.current?.click()}>
          ＋
        </Button>
        <textarea
          aria-label="CoS へのメッセージ"
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={keyDown}
          onCompositionStart={() => {
            composing.current = true;
          }}
          onCompositionEnd={() => {
            composing.current = false;
          }}
          onPaste={paste}
          rows={2}
          className="min-h-11 min-w-0 flex-1 resize-y rounded-md border border-input bg-background px-3 py-2 text-foreground"
        />
        <Button variant="primary" disabled={!canSend} onClick={() => void send("queue")}>
          送信
        </Button>
      </div>
      {(activeRun || paused) && (
        <div className="flex flex-wrap gap-2 pt-2">
          {activeRun && (
            <Button
              variant="secondary"
              disabled={activeRun.state === "stopping"}
              onClick={() => void action(() => stopRun(threadId, activeRun.id))}
            >
              停止
            </Button>
          )}
          {activeRun && (
            <Button variant="secondary" disabled={!canSend} onClick={() => void send("interrupt")}>
              割り込んで送信
            </Button>
          )}
          {paused && chat.thread && (
            <Button
              variant="secondary"
              onClick={() => void action(() => resumeQueue(threadId, chat.thread?.revision ?? 0))}
            >
              キューを再開
            </Button>
          )}
        </div>
      )}
      {actionError && (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      )}
    </section>
  );
}
