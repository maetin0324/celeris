import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../../../api/client";
import { thread } from "../data/fixtures.test-support";
import { orderThreads, type ThreadApi, ThreadsModel, threadError, threadTotals } from "./model";
import { ChatThreads } from "./threads";

function mockApi() {
  return {
    list: vi.fn<ThreadApi["list"]>(),
    open: vi.fn<ThreadApi["open"]>(),
    read: vi.fn<ThreadApi["read"]>(),
    patch: vi.fn<ThreadApi["patch"]>(),
  };
}

const conflict = new ApiError("conflict", { method: "PATCH", path: "/api/v1/chat/threads/t1", status: 409 });

// ADR 2026-10-22-cos-fixed-threads D5: the home lists the general CoS first, then each project's CoS.
const general = thread({ id: "general", title: "統括 CoS", updated_at: "2026-01-01", attention_count: 1 });
const projectA = thread({
  id: "pa",
  kind: "project",
  project_id: "A",
  title: "CoS: 案件A",
  updated_at: "2026-10-05",
  unread_count: 2,
  attention_count: 3,
});
const projectB = thread({ id: "pb", kind: "project", project_id: "B", title: "CoS: 案件B", updated_at: "2026-10-01" });
const legacy = thread({ id: "legacy", kind: "legacy", title: "以前の会話", updated_at: "2026-10-09" });

describe("chat threads", () => {
  it("chat_threads_general_first_then_projects_then_legacy", () => {
    expect(orderThreads([legacy, projectB, general, projectA]).map((item) => item.id)).toEqual([
      "general",
      "pa",
      "pb",
      "legacy",
    ]);
    expect(threadTotals([general, projectA, projectB])).toEqual({ unread: 2, attention: 4 });
  });

  it("chat_threads_list_shows_kind_unread_and_attention_counts", async () => {
    const api = mockApi();
    api.list.mockResolvedValue({ items: [legacy, projectA, general, projectB], next_cursor: null });
    const model = new ThreadsModel(api);
    await model.load();
    const html = renderToStaticMarkup(<ChatThreads model={model} onSelectThread={() => {}} />);
    expect(html.indexOf("統括 CoS")).toBeLessThan(html.indexOf("CoS: 案件A"));
    expect(html.indexOf("CoS: 案件A")).toBeLessThan(html.indexOf("CoS: 案件B"));
    expect(html).toContain("要対応 3");
    expect(html).toContain("未読 2");
    expect(html).toContain("要対応 1");
    expect(html).toContain("CoS 3 件・要対応 4・未読 2");
    expect(html).toContain("旧会話");
    // Nobody creates threads; only legacy threads can be archived.
    expect(html).not.toContain("新しい会話");
    expect(html).toContain('aria-label="以前の会話をアーカイブ"');
    expect(html).not.toContain('aria-label="CoS: 案件Aをアーカイブ"');
    expect(html).toContain('aria-label="会話一覧を開く"');
  });

  it("chat_threads_long_titles_truncate_without_shrinking_or_wrapping_badges", async () => {
    const title = "長い題名の会話".repeat(20);
    const api = mockApi();
    api.list.mockResolvedValue({
      items: [thread({ id: "legacy", kind: "legacy", title }), thread({ id: "g", title, attention_count: 123 })],
      next_cursor: null,
    });
    const model = new ThreadsModel(api);
    await model.load();
    const html = renderToStaticMarkup(<ChatThreads model={model} onSelectThread={() => {}} />);
    expect(html).toContain(`<span class="min-w-0 truncate">${title}</span>`);
    for (const label of ["旧会話", "要対応 123"]) {
      const badge = html.match(new RegExp(`<span[^>]*class="([^"]*)"[^>]*>${label}</span>`));
      expect(badge).not.toBeNull();
      expect(badge?.[1].split(" ")).toEqual(expect.arrayContaining(["shrink-0", "whitespace-nowrap"]));
      expect(badge?.[1].split(" ")).not.toContain("whitespace-normal");
    }
  });

  it("chat_threads_open_returns_the_fixed_thread_and_selects_it", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    const selected = vi.fn();
    api.list.mockResolvedValue({ items: [general], next_cursor: null });
    await model.load();
    api.open.mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ thread: projectA });
    await model.open("A", selected);
    expect(model.state.error).toMatch(/CoS の会話を開けません/);
    await model.open("A", selected);
    await model.open("A", selected);
    expect(api.open).toHaveBeenLastCalledWith({ project_id: "A" });
    expect(selected).toHaveBeenCalledWith("pa");
    expect(model.state.items.map((item) => item.id)).toEqual(["general", "pa"]);
    expect(model.general()?.id).toBe("general");
  });

  it("chat_threads_mark_read_raises_the_watermark_once", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    api.list.mockResolvedValue({ items: [projectA], next_cursor: null });
    await model.load();
    api.read.mockResolvedValue({ thread: { ...projectA, read_through_seq: 9, unread_count: 0 } });
    await model.markRead("pa", 9);
    await model.markRead("pa", 9);
    await model.markRead("pa", 4);
    expect(api.read).toHaveBeenCalledTimes(1);
    expect(api.read).toHaveBeenCalledWith("pa", { through_seq: 9 });
    expect(model.state.items[0].unread_count).toBe(0);
  });

  it("chat_threads_search_passes_q_and_uses_fts_result_in_list", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    api.list
      .mockResolvedValueOnce({ items: [general], next_cursor: null })
      .mockResolvedValueOnce({ items: [projectB], next_cursor: null });
    await model.load();
    await model.load("本文");
    expect(api.list).toHaveBeenLastCalledWith({ q: "本文", status: "open", limit: 100 });
    expect(model.state.items.map((item) => item.id)).toEqual(["pb"]);
  });

  it("chat_threads_archive_only_legacy_and_shows_queue_run_409", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    const old = thread({ id: "t", kind: "legacy", revision: 4, queued_count: 1 });
    api.list.mockResolvedValue({ items: [old, general], next_cursor: null });
    await model.load();
    expect(await model.archive(general)).toBe(false);
    expect(api.patch).not.toHaveBeenCalled();
    api.patch
      .mockRejectedValueOnce(conflict)
      .mockResolvedValueOnce({ thread: thread({ id: "t", kind: "legacy", status: "archived" }) });
    expect(await model.archive(old)).toBe(false);
    expect(model.state.error).toMatch(/実行中または待機中/);
    expect(api.patch).toHaveBeenCalledWith("t", { status: "archived", expected_revision: 4 });
    expect(await model.archive(old)).toBe(true);
    expect(model.state.items.map((item) => item.id)).toEqual(["general"]);
    expect(threadError(conflict, "archive")).toMatch(/アーカイブできません/);
  });

  it("chat_threads_drawer_has_focus_return_trigger_and_keyboard_controls", () => {
    const html = renderToStaticMarkup(<ChatThreads model={new ThreadsModel(mockApi())} onSelectThread={() => {}} />);
    expect(html).toContain('aria-label="会話一覧を開く"');
    expect(html).toContain('aria-expanded="true"');
    // Drawer の trigger は Radix Dialog.Trigger で、Escape 時の focus 復元は primitive の browser 試験で確認する。
  });
});
