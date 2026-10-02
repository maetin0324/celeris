import { describe, expect, test } from "vitest";
import type { TaskList, TaskSummary } from "../../api/generated/types";
import { mergeTaskPages } from "./merge-pages";
import { taskListPath } from "./task-list-query";

const item = (id: string) => ({ id }) as TaskSummary;
const page = (ids: string[], cursor: string | null): TaskList => ({
  items: ids.map(item),
  next_cursor: cursor,
  total: 3,
  counts_by_status: {},
});

describe("task list pagination", () => {
  test("preserves loaded pages across a first-page refresh and removes duplicate ids", () => {
    const extra = [page(["T2", "T3"], null)];
    expect(mergeTaskPages(page(["T1"], "cursor"), extra).items.map((task) => task.id)).toEqual(["T1", "T2", "T3"]);
    expect(mergeTaskPages(page(["T2", "T1"], "new-cursor"), extra)).toMatchObject({
      items: [{ id: "T2" }, { id: "T1" }, { id: "T3" }],
      nextCursor: null,
    });
  });

  test("passes GET filters and cursor without changing their meaning", () => {
    expect(taskListPath({ q: "GPU", status: ["ready", "blocked"], order: "dispatch", limit: "20" }, "next")).toBe(
      "/api/tasks?q=GPU&status=ready&status=blocked&order=dispatch&limit=20&cursor=next",
    );
  });
});
