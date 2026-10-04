import { describe, expect, it } from "vitest";
import type { TaskSummary } from "../../api/generated/types";
import { boardFilterFromSearch, boardGroup, boardHref, boardTasksPath } from "./board-model";

const card = (id: string, status: TaskSummary["status"], priority: number, created_at: string) =>
  ({
    id,
    status,
    priority,
    created_at,
    priority_label: priority >= 20 ? "P1" : "P2",
  }) as TaskSummary;

describe("board grouping and URL filter", () => {
  it("maps all six status columns and orders cards by priority then creation", () => {
    const groups = boardGroup([
      card("newer", "ready", 10, "2026-02-01"),
      card("urgent", "draft", 20, "2026-03-01"),
      card("older", "ready", 10, "2026-01-01"),
      card("run", "running", 10, "2026-01-01"),
      card("blocked", "blocked", 10, "2026-01-01"),
      card("done", "done", 10, "2026-01-01"),
      card("failed", "failed", 10, "2026-01-01"),
      card("cancelled", "cancelled", 10, "2026-01-01"),
    ]);
    expect(groups.map((group) => group.id)).toEqual([
      "waiting",
      "in_progress",
      "blocked",
      "done",
      "failed",
      "cancelled",
    ]);
    expect(groups[0].items.map((item) => item.id)).toEqual(["urgent", "older", "newer"]);
  });
  it("keeps filters in the URL and excludes display-only show_support from daemon query", () => {
    const filter = boardFilterFromSearch(new URLSearchParams("project=P1&q=docs&show_support=1"));
    expect(filter).toMatchObject({ project: "P1", q: "docs", show_support: true });
    const path = boardTasksPath(filter);
    expect(path).toContain("project=P1");
    expect(path).toContain("q=docs");
    expect(path).not.toContain("show_support");
  });
});

describe("board state filter", () => {
  it("reads only known columns and keeps it out of the daemon query", () => {
    expect(boardFilterFromSearch(new URLSearchParams("column=blocked")).column).toBe("blocked");
    expect(boardFilterFromSearch(new URLSearchParams("column=bogus")).column).toBeUndefined();
    const path = boardTasksPath(boardFilterFromSearch(new URLSearchParams("project=P1&column=done")));
    expect(path).toContain("project=P1");
    expect(path).not.toContain("column");
  });
  it("round-trips filters to the /board URL and swaps only the column", () => {
    const filter = boardFilterFromSearch(new URLSearchParams("project=P1&q=docs&show_support=1&column=done"));
    expect(boardHref(filter)).toBe("/board?project=P1&q=docs&show_support=1&column=done");
    expect(boardHref({ ...filter, column: undefined })).toBe("/board?project=P1&q=docs&show_support=1");
    expect(boardHref({})).toBe("/board");
  });
});
