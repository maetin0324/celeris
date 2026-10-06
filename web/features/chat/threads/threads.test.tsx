import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../../../api/client";
import { thread } from "../data/fixtures.test-support";
import { orderThreads, type ThreadApi, ThreadsModel, threadError } from "./model";
import { ChatThreads } from "./threads";

function mockApi() {
  return {
    list: vi.fn<ThreadApi["list"]>(),
    create: vi.fn<ThreadApi["create"]>(),
    patch: vi.fn<ThreadApi["patch"]>(),
  };
}

const conflict = new ApiError("conflict", { method: "PATCH", path: "/api/v1/chat/threads/t1", status: 409 });

describe("chat threads", () => {
  it("chat_threads_inbox_is_pinned_and_legacy_is_marked", async () => {
    const inbox = thread({ id: "inbox", kind: "inbox", title: "受信箱", updated_at: "2026-01-01" });
    const recent = thread({ id: "recent", updated_at: "2026-10-05" });
    const legacy = thread({ id: "legacy", kind: "legacy", title: "以前の会話", updated_at: "2026-09-01" });
    expect(orderThreads([legacy, recent, inbox], inbox).map((item) => item.id)).toEqual(["inbox", "recent", "legacy"]);
    const api = mockApi();
    api.list.mockResolvedValue({ items: [legacy, recent, inbox], next_cursor: null });
    const model = new ThreadsModel(api);
    await model.load();
    const html = renderToStaticMarkup(<ChatThreads model={model} inboxWaitingCount={3} onSelectThread={() => {}} />);
    expect(html).toContain("旧会話");
    expect(html).toContain("3 件待ち");
    expect(html.indexOf("受信箱")).toBeLessThan(html.indexOf("以前の会話"));
    expect(html).toContain('aria-label="会話一覧を開く"');
  });

  it("chat_threads_create_reuses_client_thread_id_after_unknown_failure_and_selects_existing_response", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    const selected = vi.fn();
    api.list.mockResolvedValue({ items: [thread({ id: "existing", title: "残す会話" })], next_cursor: null });
    await model.load();
    api.create
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce({ thread: thread({ id: "created", updated_at: "2026-10-06T00:00:00Z" }) });
    await model.create(selected);
    expect(model.state.error).toBeTruthy();
    await model.create(selected);
    expect(api.create).toHaveBeenCalledTimes(2);
    expect(api.create.mock.calls[0][0].client_thread_id).toBe(api.create.mock.calls[1][0].client_thread_id);
    expect(selected).toHaveBeenCalledWith("created");
    expect(model.state.items.map((item) => item.id)).toEqual(["created", "existing"]);
  });

  it("chat_threads_search_passes_q_and_uses_fts_result_in_list", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    api.list
      .mockResolvedValueOnce({ items: [thread({ id: "inbox", kind: "inbox" })], next_cursor: null })
      .mockResolvedValueOnce({ items: [thread({ id: "hit", title: "会話本文の一致" })], next_cursor: null });
    await model.load();
    await model.load("本文");
    expect(api.list).toHaveBeenLastCalledWith({ q: "本文", status: "open", limit: 100 });
    expect(model.state.items.map((item) => item.id)).toEqual(["inbox", "hit"]);
  });

  it("chat_threads_rename_sends_expected_revision_and_reports_409", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    api.list.mockResolvedValue({ items: [thread({ id: "t", revision: 7, title: "前" })], next_cursor: null });
    await model.load();
    api.patch.mockRejectedValue(conflict);
    expect(await model.rename(model.state.items[0], "後")).toBe(false);
    expect(api.patch).toHaveBeenCalledWith("t", { title: "後", expected_revision: 7 });
    expect(model.state.error).toMatch(/別の場所で更新/);
    expect(model.state.items[0].title).toBe("前");
  });

  it("chat_threads_archive_sends_revision_and_shows_queue_run_409", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    api.list.mockResolvedValue({ items: [thread({ id: "t", revision: 4, queued_count: 1 })], next_cursor: null });
    await model.load();
    api.patch
      .mockRejectedValueOnce(conflict)
      .mockResolvedValueOnce({ thread: thread({ id: "t", status: "archived" }) });
    expect(await model.archive(model.state.items[0])).toBe(false);
    expect(model.state.error).toMatch(/実行中または待機中/);
    expect(api.patch).toHaveBeenCalledWith("t", { status: "archived", expected_revision: 4 });
    expect(await model.archive(model.state.items[0])).toBe(true);
    expect(model.state.items).toEqual([]);
    expect(threadError(conflict, "archive")).toMatch(/アーカイブできません/);
  });

  it("chat_threads_drawer_has_focus_return_trigger_and_keyboard_controls", () => {
    const html = renderToStaticMarkup(<ChatThreads model={new ThreadsModel(mockApi())} onSelectThread={() => {}} />);
    expect(html).toContain('aria-label="会話一覧を開く"');
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain("新しい会話");
    // Drawer の trigger は Radix Dialog.Trigger で、Escape 時の focus 復元は primitive の browser 試験で確認する。
  });
});
