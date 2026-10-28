import type { ChatThread } from "../../../api/generated/types";
import { listThreads, openThread, readThread } from "../data/client";

// ADR 2026-10-28-single-human-window-general-cos D5: the person talks to the general CoS. The home
// opens it; a project's CoS thread is opened for reading from a card (札) or the project page.
// Nobody creates threads: `open` returns the fixed CoS thread of a scope.

export type ThreadApi = {
  list: typeof listThreads;
  open: typeof openThread;
  read: typeof readThread;
};

export const threadApi: ThreadApi = { list: listThreads, open: openThread, read: readThread };

export type ThreadState = {
  items: ChatThread[];
  loading: boolean;
  error: string | null;
};

export const OPEN_ERROR = "CoS の会話を開けませんでした。接続を確認して再試行してください。";

export class ThreadsModel {
  private api: ThreadApi;
  private listeners = new Set<() => void>();
  state: ThreadState = { items: [], loading: false, error: null };

  constructor(api: ThreadApi = threadApi) {
    this.api = api;
  }
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
  snapshot = () => this.state;
  private set(patch: Partial<ThreadState>) {
    this.state = { ...this.state, ...patch };
    for (const listener of this.listeners) listener();
  }
  private replace(thread: ChatThread) {
    this.set({ items: [...this.state.items.filter((item) => item.id !== thread.id), thread] });
  }

  /** The general CoS thread once loaded or opened. */
  general(): ChatThread | null {
    return this.state.items.find((item) => item.kind === "general") ?? null;
  }

  /** The CoS threads the home knows (for the read watermark and the general thread). */
  async load(): Promise<void> {
    this.set({ loading: true, error: null });
    try {
      const response = await this.api.list({ status: "open", limit: 100 });
      this.set({ items: response.items, loading: false });
    } catch {
      this.set({ loading: false, error: OPEN_ERROR });
    }
  }

  /** Open the general CoS (`projectId` null) or a project's CoS; idempotent on the server. */
  async open(projectId: string | null, onSelect: (id: string) => void): Promise<void> {
    this.set({ error: null });
    try {
      const response = await this.api.open({ project_id: projectId });
      this.replace(response.thread);
      onSelect(response.thread.id);
    } catch {
      this.set({ error: OPEN_ERROR });
    }
  }

  /** Mark the thread read through `seq` when it is ahead of the stored watermark. */
  async markRead(threadId: string, seq: number): Promise<void> {
    const current = this.state.items.find((item) => item.id === threadId);
    if (current && current.read_through_seq >= seq) return;
    try {
      const response = await this.api.read(threadId, { through_seq: seq });
      this.replace(response.thread);
    } catch {
      // 既読は次に開いたときに上げ直す。読めなかったことは会話の操作を妨げない。
    }
  }
}
