import { describe, expect, it, vi } from "vitest";
import { thread } from "../data/fixtures.test-support";
import { OPEN_ERROR, type ThreadApi, ThreadsModel } from "./model";

function mockApi() {
  return {
    list: vi.fn<ThreadApi["list"]>(),
    open: vi.fn<ThreadApi["open"]>(),
    read: vi.fn<ThreadApi["read"]>(),
  };
}

const general = thread({ id: "general", title: "統括 CoS" });
const projectA = thread({ id: "pa", kind: "project", project_id: "A", title: "CoS: 案件A" });

describe("chat threads model", () => {
  it("chat_threads_open_returns_the_fixed_thread_and_selects_it", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    const selected = vi.fn();
    api.list.mockResolvedValue({ items: [projectA], next_cursor: null });
    await model.load();
    expect(model.general()).toBeNull();
    api.open.mockRejectedValueOnce(new Error("offline")).mockResolvedValue({ thread: general });
    await model.open(null, selected);
    expect(model.state.error).toBe(OPEN_ERROR);
    await model.open(null, selected);
    expect(api.open).toHaveBeenLastCalledWith({ project_id: null });
    expect(selected).toHaveBeenCalledWith("general");
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

  it("chat_threads_drop_the_review_thread_even_if_the_api_returns_it", async () => {
    const api = mockApi();
    const model = new ThreadsModel(api);
    const review = thread({ id: "rv", kind: "review", title: "統括 CoS（定期見直し）" });
    api.list.mockResolvedValue({ items: [general, review, projectA], next_cursor: null });
    await model.load();
    expect(model.state.items.map((item) => item.id)).toEqual(["general", "pa"]);
    api.read.mockResolvedValue({ thread: { ...review, read_through_seq: 3 } });
    await model.markRead("rv", 3);
    expect(model.state.items.map((item) => item.id)).toEqual(["general", "pa"]);
  });
});
