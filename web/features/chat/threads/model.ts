import { isApiError } from "../../../api/client";
import type { ChatThread } from "../../../api/generated/types";
import { listThreads, openThread, patchThread, readThread } from "../data/client";

// ADR 2026-10-22-cos-fixed-threads: the home lists the general CoS, one CoS per project and
// read-only legacy threads. Nobody creates threads: `open` returns the fixed CoS thread of a scope.

export type ThreadApi = {
  list: typeof listThreads;
  open: typeof openThread;
  read: typeof readThread;
  patch: typeof patchThread;
};

export const threadApi: ThreadApi = { list: listThreads, open: openThread, read: readThread, patch: patchThread };

export type ThreadState = {
  items: ChatThread[];
  query: string;
  loading: boolean;
  busy: boolean;
  error: string | null;
  nextCursor: string | null;
};

const KIND_ORDER: Record<ChatThread["kind"], number> = { general: 0, project: 1, legacy: 2 };

/** 統括 CoS を先頭に、案件 CoS、旧会話の順。同じ種類の中は API と同じ更新日時・id の降順。 */
export function orderThreads(items: ChatThread[]): ChatThread[] {
  const unique = new Map<string, ChatThread>();
  for (const item of items) unique.set(item.id, item);
  const ordered = [...unique.values()].filter((item) => item.status === "open");
  ordered.sort(
    (a, b) =>
      KIND_ORDER[a.kind] - KIND_ORDER[b.kind] || b.updated_at.localeCompare(a.updated_at) || b.id.localeCompare(a.id),
  );
  return ordered;
}

/** 一覧の見出しに出す数: 未読と要対応（受信箱の未解決項目）の合計。 */
export function threadTotals(items: ChatThread[]): { unread: number; attention: number } {
  return items.reduce(
    (acc, item) => ({ unread: acc.unread + item.unread_count, attention: acc.attention + item.attention_count }),
    { unread: 0, attention: 0 },
  );
}

export function threadError(error: unknown, operation: "archive" | "open" | "list"): string {
  if (isApiError(error) && error.status === 409 && operation === "archive") {
    return "実行中または待機中の発言があるため、会話をアーカイブできません。";
  }
  if (operation === "open") return "CoS の会話を開けませんでした。接続を確認して再試行してください。";
  return "会話を操作できませんでした。接続を確認して再試行してください。";
}

export class ThreadsModel {
  private api: ThreadApi;
  private listeners = new Set<() => void>();
  private request = 0;
  state: ThreadState = {
    items: [],
    query: "",
    loading: false,
    busy: false,
    error: null,
    nextCursor: null,
  };

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
    this.set({ items: orderThreads([...this.state.items.filter((item) => item.id !== thread.id), thread]) });
  }

  /** The general CoS thread when it is in the list. */
  general(): ChatThread | null {
    return this.state.items.find((item) => item.kind === "general") ?? null;
  }

  async load(query = this.state.query): Promise<void> {
    const request = ++this.request;
    this.set({ query, loading: true, error: null, nextCursor: null });
    try {
      const response = await this.api.list({ q: query || undefined, status: "open", limit: 100 });
      if (request !== this.request) return;
      this.set({ items: orderThreads(response.items), loading: false, nextCursor: response.next_cursor ?? null });
    } catch (error) {
      if (request === this.request) this.set({ loading: false, error: threadError(error, "list") });
    }
  }

  async more(): Promise<void> {
    const cursor = this.state.nextCursor;
    if (!cursor || this.state.loading) return;
    const request = ++this.request;
    this.set({ loading: true, error: null });
    try {
      const response = await this.api.list({
        q: this.state.query || undefined,
        status: "open",
        before: cursor,
        limit: 100,
      });
      if (request !== this.request) return;
      this.set({
        items: orderThreads([...this.state.items, ...response.items]),
        loading: false,
        nextCursor: response.next_cursor ?? null,
      });
    } catch (error) {
      if (request === this.request) this.set({ loading: false, error: threadError(error, "list") });
    }
  }

  /** Open the general CoS (`projectId` null) or a project's CoS; idempotent on the server. */
  async open(projectId: string | null, onSelect: (id: string) => void): Promise<void> {
    if (this.state.busy) return;
    this.set({ busy: true, error: null });
    try {
      const response = await this.api.open({ project_id: projectId });
      this.set({ busy: false });
      this.replace(response.thread);
      onSelect(response.thread.id);
    } catch (error) {
      this.set({ busy: false, error: threadError(error, "open") });
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

  /** Only legacy threads can be archived (CoS threads stay). */
  async archive(item: ChatThread): Promise<boolean> {
    if (this.state.busy || item.kind !== "legacy") return false;
    this.set({ busy: true, error: null });
    try {
      await this.api.patch(item.id, { status: "archived", expected_revision: item.revision });
      this.set({ busy: false, items: this.state.items.filter((current) => current.id !== item.id) });
      return true;
    } catch (error) {
      this.set({ busy: false, error: threadError(error, "archive") });
      return false;
    }
  }
}
