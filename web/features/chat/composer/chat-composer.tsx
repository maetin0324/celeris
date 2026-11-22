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
import type { ChatDiscussionMode, ChatThread } from "../../../api/generated/types";
import { Button } from "../../../components/ui/button";
import { Select } from "../../../components/ui/select";
import { cancelMessage, resumeQueue, stopRun } from "../data/client";
import { selectActiveRun } from "../data/reducer";
import { DISCUSSION_MODE_LABEL, discussionModeField, newClientId } from "../data/send";
import type { ChatSession, ChatSessionSnapshot, SendInput } from "../data/session";
import { createUploadQueue, pastedFiles, shouldSendOnEnter, type UploadApi } from "./upload-queue";

export type ChatComposerProps = {
  threadId: string;
  session: ChatSession;
  snapshot: ChatSessionSnapshot;
  uploadApi?: UploadApi;
  /** 入力欄の placeholder（CoS の会話では受信箱の件もここで聞く・指示する場所であることを示す）。 */
  placeholder?: string;
};

/** ADR 2026-10-22-cos-fixed-threads D3: 統括・案件の CoS の composer の placeholder。 */
export const COS_COMPOSER_PLACEHOLDER = "この CoS に聞く・指示する（受信箱の件もここで）";

/**
 * ADR 2026-11-21-chat-discussion-mode D9: 発言の扱いを選べるのは案件の CoS thread だけ（統括・review の composer は今のまま）。
 * 選択肢は議論・案・実行の順で、既定は「実行」（欄を送らない。mode を選ぶ前と同じ送信）。
 */
export const DISCUSSION_MODE_CHOICES: readonly ChatDiscussionMode[] = ["discuss", "propose", "execute"];
export const DEFAULT_DISCUSSION_MODE: ChatDiscussionMode = "execute";
export const DISCUSSION_MODE_NOTE: Partial<Record<ChatDiscussionMode, string>> = {
  discuss: "議論: CoS は task の起票・案件と roadmap の変更・公開をしません（API が拒否します）",
  propose: "案: CoS は案を返事の添付で返すだけで、起票・変更はしません（API が拒否します）",
};

export function offersDiscussionMode(thread: Pick<ChatThread, "kind"> | null | undefined): boolean {
  return thread?.kind === "project";
}

/** composer の入力から送信の要求を組む（client_message_id は呼び出し側が再試行の鍵で決める）。 */
export function composerSendInput(draft: {
  text: string;
  attachmentIds: string[];
  mode: "queue" | "interrupt";
  paused: boolean;
  discussionMode?: ChatDiscussionMode;
}): Omit<SendInput, "client_message_id"> {
  return {
    text: draft.text,
    attachment_ids: draft.attachmentIds,
    mode: draft.mode,
    resume_queue: draft.paused,
    ...discussionModeField(draft.discussionMode),
  };
}

function bytes(size: number) {
  return size < 1024 ? `${size} B` : `${(size / 1024).toFixed(1)} KiB`;
}

export function ChatComposer({ threadId, session, snapshot, uploadApi, placeholder }: ChatComposerProps) {
  const queue = useMemo(() => createUploadQueue(threadId, uploadApi), [threadId, uploadApi]);
  const uploads = useSyncExternalStore(queue.subscribe, queue.getSnapshot, queue.getSnapshot);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState("");
  const [discussionMode, setDiscussionMode] = useState<ChatDiscussionMode>(DEFAULT_DISCUSSION_MODE);
  const input = useRef<HTMLInputElement>(null);
  const composing = useRef(false);
  const sending = useRef(false);
  const sentFileIds = useRef<Set<string>>(new Set());
  const retryKey = useRef<{ fingerprint: string; id: string } | null>(null);

  useEffect(() => () => queue.dispose(), [queue]);

  const chat = snapshot.chat;
  const choosesMode = offersDiscussionMode(chat.thread);
  const activeRun = selectActiveRun(chat);
  // キュー（停止含む）は chat.queue が正本（snapshot でも queue event でも導かれる）。
  const paused = chat.queue.paused;
  const waiting = chat.queue.message_ids
    .map((id) => chat.messages[id])
    .filter((message) => message?.state === "queued");
  // ADR 2026-10-28-cos-inbox-chat-summary D4: daemon の依頼（role=system）は人の送信待ちと分けて出す。
  const queued = waiting.filter((message) => message.role !== "system");
  const requests = waiting.filter((message) => message.role === "system");
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
    const input = composerSendInput({
      text: sentText,
      attachmentIds,
      mode,
      paused,
      discussionMode: choosesMode ? discussionMode : undefined,
    });
    // 扱いを変えた送り直しは別の発言（同じ client_message_id で本文の違う要求を送らない）。
    const fingerprint = JSON.stringify([threadId, input]);
    const clientId = retryKey.current?.fingerprint === fingerprint ? retryKey.current.id : newClientId();
    retryKey.current = { fingerprint, id: clientId };
    // 送信中の印は id を作り終えてから立てる（id の生成が例外になっても『送信中』のまま固まらない）。
    sending.current = true;
    setBusy(true);
    sentFileIds.current = new Set(ids);
    try {
      await session.send({ client_message_id: clientId, ...input });
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
      {requests.length > 0 && (
        <ol aria-label="CoS への自動の依頼" className="max-h-24 overflow-y-auto">
          {requests.map((message, index) => (
            <li key={message.id} className="flex min-w-0 items-center gap-2 border-b border-border py-1">
              <span className="min-w-0 flex-1 truncate text-sm text-muted-foreground">
                自動の依頼: {message.text.split("\n")[0]}
              </span>
              <Button
                size="sm"
                variant="ghost"
                aria-label={`自動の依頼 ${index + 1} を取り消す`}
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
          placeholder={placeholder}
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
        {choosesMode && (
          <Select
            aria-label="発言の扱い"
            value={discussionMode}
            onChange={(event) => setDiscussionMode(event.currentTarget.value as ChatDiscussionMode)}
            className="w-auto shrink-0"
          >
            {DISCUSSION_MODE_CHOICES.map((choice) => (
              <option key={choice} value={choice}>
                {DISCUSSION_MODE_LABEL[choice]}
              </option>
            ))}
          </Select>
        )}
        <Button variant="primary" disabled={!canSend} onClick={() => void send("queue")}>
          送信
        </Button>
      </div>
      {choosesMode && DISCUSSION_MODE_NOTE[discussionMode] && (
        <p data-discussion-mode-note={discussionMode} className="pt-1 text-label text-muted-foreground">
          {DISCUSSION_MODE_NOTE[discussionMode]}
        </p>
      )}
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
