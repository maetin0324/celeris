import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ChatAttachment } from "../../../api/generated/types";
import { message, run, thread } from "../data/fixtures.test-support";
import { initialChatState } from "../data/reducer";
import type { ChatSession, ChatSessionSnapshot } from "../data/session";
import { ChatComposer } from "./chat-composer";
import { createUploadQueue, pastedFiles, shouldSendOnEnter } from "./upload-queue";

const flush = async () => {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
};
const file = () => new File(["abc"], "original.png", { type: "image/png" });

describe("chat composer", () => {
  it("chat_composer_enter_shift_enter_and_ime", () => {
    const event = { key: "Enter", shiftKey: false, isComposing: false, keyCode: 13 };
    expect(shouldSendOnEnter(event)).toBe(true);
    expect(shouldSendOnEnter({ ...event, shiftKey: true })).toBe(false);
    expect(shouldSendOnEnter({ ...event, isComposing: true })).toBe(false);
    expect(shouldSendOnEnter({ ...event, keyCode: 229 })).toBe(false);
  });

  it("chat_composer_drop_and_paste_share_file_extraction", () => {
    const image = file();
    expect(
      pastedFiles({
        items: [{ kind: "file", getAsFile: () => image }] as unknown as DataTransferItemList,
        files: [] as unknown as FileList,
      }),
    ).toEqual([image]);
    expect(
      pastedFiles({ items: [] as unknown as DataTransferItemList, files: [image] as unknown as FileList }),
    ).toEqual([image]);
  });

  it("chat_composer_upload_progress_failure_retry_cancel", async () => {
    const attempts: Array<{
      signal: AbortSignal;
      progress: (p: { loaded: number; total: number }) => void;
      resolve: (value: ChatAttachment) => void;
      reject: (error: Error) => void;
    }> = [];
    const remove = vi.fn(async () => undefined);
    const queue = createUploadQueue("t1", {
      remove,
      upload: (_thread, _file, options) =>
        new Promise<ChatAttachment>((resolve, reject) => {
          attempts.push({ signal: options.signal, progress: options.onProgress, resolve, reject });
        }),
    });
    queue.add([file()]);
    expect(queue.getSnapshot()[0]?.file.name).toBe("original.png");
    attempts[0]?.progress({ loaded: 1, total: 2 });
    expect(queue.getSnapshot()[0]?.progress).toBe(50);
    attempts[0]?.reject(new Error("offline"));
    await flush();
    expect(queue.getSnapshot()[0]?.state).toBe("failed");
    queue.retry(queue.getSnapshot()[0]?.id ?? "");
    expect(attempts).toHaveLength(2);
    attempts[1]?.resolve({
      id: "a1",
      name: "original.png",
      size_bytes: 3,
      media_type: "image/png",
      state: "ready",
      thread_id: "t1",
      sha256: "x",
      download_url: "/a",
    });
    await flush();
    expect(queue.getSnapshot()[0]?.state).toBe("ready");
    queue.remove(queue.getSnapshot()[0]?.id ?? "");
    expect(remove).toHaveBeenCalledWith("a1");
    queue.add([file()]);
    queue.remove(queue.getSnapshot()[0]?.id ?? "");
    expect(attempts[2]?.signal.aborted).toBe(true);
    queue.dispose();
  });

  it("chat_composer_late_upload_after_cancel_is_deleted_and_new_draft_survives_send", async () => {
    const complete: Array<(attachment: ChatAttachment) => void> = [];
    const remove = vi.fn(async () => undefined);
    const queue = createUploadQueue("t1", {
      remove,
      upload: () => new Promise<ChatAttachment>((resolve) => complete.push(resolve)),
    });
    const first = file();
    queue.add([first]);
    const firstId = queue.getSnapshot()[0]?.id ?? "";
    queue.remove(firstId);
    complete[0]?.({ id: "late", thread_id: "t1" } as ChatAttachment);
    await flush();
    expect(queue.getSnapshot()).toHaveLength(0);
    expect(remove).toHaveBeenCalledWith("late");

    queue.add([first]);
    complete[1]?.({ id: "sent", thread_id: "t1" } as ChatAttachment);
    await flush();
    const sentId = queue.getSnapshot()[0]?.id ?? "";
    queue.add([new File(["b"], "next.txt")]);
    queue.clearSent([sentId]);
    expect(queue.getSnapshot().map((item) => item.file.name)).toEqual(["next.txt"]);
    queue.dispose();
  });

  it("chat_composer_queue_stop_interrupt_resume_and_status_are_separate_controls", () => {
    const chat = initialChatState("t1");
    chat.thread = thread({ queue_paused: true, active_run_id: "r1" });
    chat.runs.r1 = run("r1", "stopping");
    chat.messages.m1 = message("m1", 1, { text: "待機中", state: "queued" });
    chat.queue = { message_ids: ["m1"], paused: true };
    const snapshot: ChatSessionSnapshot = { chat, stream: "reconnecting", loading: false, loadError: undefined };
    const html = renderToStaticMarkup(<ChatComposer threadId="t1" snapshot={snapshot} session={{} as ChatSession} />);
    for (const label of ["送信待ち", "1. 待機中", "停止", "割り込んで送信", "キューを再開", "停止を要求中"])
      expect(html).toContain(label);
    expect(html).toContain("--shell-bottom-inset");
    expect(html).toContain("safe-area-inset-bottom");
    expect(html).toContain('aria-label="CoS へのメッセージ"');
    expect(html).toContain('aria-label="ファイルを添付"');
    expect(html).toContain('aria-label="1番目を取り消す"');
  });

  it("chat_composer_quota_disconnected_and_failed_states", () => {
    const chat = initialChatState("t1");
    chat.thread = thread({ active_run_id: "r1" });
    chat.runs.r1 = { ...run("r1", "running"), reason: "quota_exhausted" };
    const snapshot: ChatSessionSnapshot = { chat, stream: "open", loading: false, loadError: undefined };
    const render = () =>
      renderToStaticMarkup(<ChatComposer threadId="t1" snapshot={snapshot} session={{} as ChatSession} />);
    expect(render()).toContain("利用枠の回復を待っています");
    chat.runs.r1 = run("r1", "stopping");
    expect(render()).toContain("停止を要求中です");
    chat.runs.r1 = run("r1", "failed");
    chat.thread = thread({ active_run_id: null });
    expect(render()).toContain("実行に失敗しました");
    snapshot.stream = "reconnecting";
    expect(render()).toContain("接続が切れています");
  });
});
