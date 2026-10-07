import { isApiError } from "../../../api/client";
import type { ChatThread } from "../../../api/generated/types";
import { createThread, listThreads, patchThread } from "../data/client";

export type ThreadApi = {
  list: typeof listThreads;
  create: typeof createThread;
  patch: typeof patchThread;
};

export const threadApi: ThreadApi = { list: listThreads, create: createThread, patch: patchThread };

export type ThreadState = {
  items: ChatThread[];
  inbox: ChatThread | null;
  query: string;
  loading: boolean;
  busy: boolean;
  error: string | null;
  nextCursor: string | null;
};

/** inbox を常に先頭に保ち、残りは API と同じ更新日時・id の降順。 */
export function orderThreads(items: ChatThread[], inbox: ChatThread | null): ChatThread[] {
  const unique = new Map<string, ChatThread>();
  for (const item of items) unique.set(item.id, item);
  if (inbox) unique.set(inbox.id, inbox);
  const ordered = [...unique.values()].filter((item) => item.status === "open");
  ordered.sort((a, b) => {
    if (a.kind === "inbox" && b.kind !== "inbox") return -1;
    if (b.kind === "inbox" && a.kind !== "inbox") return 1;
    return b.updated_at.localeCompare(a.updated_at) || b.id.localeCompare(a.id);
  });
  return ordered;
}

export function threadError(error: unknown, operation: "rename" | "archive" | "create" | "list"): string {
  if (isApiError(error) && error.status === 409) {
    if (operation === "archive") return "実行中または待機中の発言があるため、会話をアーカイブできません。";
    if (operation === "rename") return "会話が別の場所で更新されました。一覧を読み直してから再試行してください。";
    return "会話が競合しました。一覧を読み直してから再試行してください。";
  }
  return "会話を操作できませんでした。接続を確認して再試行してください。";
}

export class ThreadsModel {
  private api: ThreadApi;
  private listeners = new Set<() => void>();
  private request = 0;
  private createId: string | null = null;
  state: ThreadState = {
    items: [],
    inbox: null,
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

  async load(query = this.state.query): Promise<void> {
    const request = ++this.request;
    this.set({ query, loading: true, error: null, nextCursor: null });
    try {
      const response = await this.api.list({ q: query || undefined, status: "open", limit: 100 });
      if (request !== this.request) return;
      const inbox = !query
        ? (response.items.find((item) => item.kind === "inbox") ?? this.state.inbox)
        : this.state.inbox;
      this.set({
        items: orderThreads(response.items, inbox),
        inbox,
        loading: false,
        nextCursor: response.next_cursor ?? null,
      });
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
        items: orderThreads([...this.state.items, ...response.items], this.state.inbox),
        loading: false,
        nextCursor: response.next_cursor ?? null,
      });
    } catch (error) {
      if (request === this.request) this.set({ loading: false, error: threadError(error, "list") });
    }
  }

  async create(onSelect: (id: string) => void): Promise<void> {
    if (this.state.busy) return;
    this.createId ??= crypto.randomUUID();
    this.set({ busy: true, error: null });
    try {
      const response = await this.api.create({ client_thread_id: this.createId, title: "新しい会話" });
      this.createId = null;
      this.set({
        busy: false,
        query: "",
        items: orderThreads([response.thread, ...this.state.items], this.state.inbox),
      });
      onSelect(response.thread.id);
    } catch (error) {
      this.set({ busy: false, error: threadError(error, "create") });
    }
  }

  async rename(item: ChatThread, title: string): Promise<boolean> {
    const next = title.trim();
    if (!next || next === item.title || this.state.busy) return false;
    this.set({ busy: true, error: null });
    try {
      const response = await this.api.patch(item.id, { title: next, expected_revision: item.revision });
      this.set({
        busy: false,
        items: orderThreads(
          this.state.items.map((current) => (current.id === item.id ? response.thread : current)),
          this.state.inbox,
        ),
      });
      return true;
    } catch (error) {
      this.set({ busy: false, error: threadError(error, "rename") });
      return false;
    }
  }

  async archive(item: ChatThread): Promise<boolean> {
    if (this.state.busy) return false;
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
