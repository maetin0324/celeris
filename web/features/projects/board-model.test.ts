import { describe, expect, it } from "vitest";
import type { TaskSummary } from "../../api/generated/types";
import {
  boardFilterFromSearch,
  boardGroup,
  boardGroupCountLabel,
  boardHref,
  boardLiveTasksPath,
  boardTerminalTasksPath,
  fetchBoard,
  terminalLimit,
} from "./board-model";

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
    const path = boardLiveTasksPath(filter);
    expect(path).toContain("project=P1");
    expect(path).toContain("q=docs");
    expect(path).not.toContain("show_support");
    expect(path).not.toContain("with=");
  });
});

describe("board state filter", () => {
  it("reads only known columns and keeps it out of the daemon query", () => {
    expect(boardFilterFromSearch(new URLSearchParams("column=blocked")).column).toBe("blocked");
    expect(boardFilterFromSearch(new URLSearchParams("column=bogus")).column).toBeUndefined();
    const path = boardLiveTasksPath(boardFilterFromSearch(new URLSearchParams("project=P1&column=done")));
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

/** daemon の GET /api/tasks を真似る: status（カンマ区切り）・作成の新しい順・limit・cursor（offset）・total。 */
function fakeDaemon(tasks: TaskSummary[]) {
  const paths: string[] = [];
  const get = async (path: string) => {
    paths.push(path);
    const params = new URL(path, "http://board.test").searchParams;
    const statuses = params.get("status")?.split(",") ?? [];
    const matched = tasks
      .filter((task) => statuses.length === 0 || statuses.includes(task.status))
      .sort((a, b) => b.created_at.localeCompare(a.created_at));
    const limit = Number(params.get("limit"));
    const offset = Number(params.get("cursor") ?? 0);
    const end = offset + limit;
    return {
      items: matched.slice(offset, end),
      next_cursor: end < matched.length ? String(end) : null,
      total: matched.length,
    };
  };
  return { get, paths };
}

const minute = (n: number) => new Date(Date.UTC(2026, 9, 27, 0, 0) + n * 60_000).toISOString();

describe("board loading beyond the newest 200", () => {
  it("shows an old running task in 進行中 even when 200+ newer terminal tasks exist", async () => {
    const tasks = [card("old-running", "running", 10, minute(0))];
    for (let i = 1; i <= 260; i++) tasks.push(card(`done-${i}`, "done", 10, minute(i)));
    for (let i = 1; i <= 5; i++) tasks.push(card(`failed-${i}`, "failed", 10, minute(300 + i)));
    const daemon = fakeDaemon(tasks);
    const data = await fetchBoard({}, daemon.get);
    const groups = boardGroup(data.items, data.terminal);
    const byId = Object.fromEntries(groups.map((group) => [group.id, group]));
    expect(byId.in_progress.items.map((item) => item.id)).toEqual(["old-running"]);
    // 終端は新しい 200 件だけ読み、列の件数は全件の数。
    expect(byId.done.items).toHaveLength(terminalLimit);
    expect(byId.done.items.some((item) => item.id === "done-260")).toBe(true);
    expect(byId.done.items.some((item) => item.id === "done-1")).toBe(false);
    expect(byId.done.count).toBe(260);
    expect(byId.done.truncated).toEqual({ loaded: 200, total: 260 });
    expect(boardGroupCountLabel(byId.done)).toBe("全 260 件のうち新しい 200 件を表示");
    // 読み切った列は切っていない。
    expect(byId.failed.truncated).toBeUndefined();
    expect(byId.failed.count).toBe(5);
    expect(boardGroupCountLabel(byId.failed)).toBe("5 件");
    expect(groups.reduce((sum, group) => sum + group.count, 0)).toBe(266);
  });

  it("follows next_cursor so that every live task is loaded", async () => {
    const tasks: TaskSummary[] = [];
    for (let i = 0; i < 1200; i++) tasks.push(card(`ready-${i}`, "ready", 10, minute(i)));
    const daemon = fakeDaemon(tasks);
    const data = await fetchBoard({ project: "P1" }, daemon.get);
    const waiting = boardGroup(data.items, data.terminal).find((group) => group.id === "waiting");
    expect(waiting?.items).toHaveLength(1200);
    expect(waiting?.truncated).toBeUndefined();
    const livePaths = daemon.paths.filter((path) => path.includes("status=draft"));
    expect(livePaths).toHaveLength(3);
    expect(livePaths.every((path) => path.includes("project=P1"))).toBe(true);
  });

  it("asks live statuses and each terminal status separately and skips terminal with run_state", async () => {
    expect(decodeURIComponent(boardLiveTasksPath({}))).toContain("status=draft,ready,running,reviewing,blocked");
    expect(boardTerminalTasksPath({ q: "x" }, "done")).toContain("status=done");
    expect(boardTerminalTasksPath({ q: "x" }, "done")).toContain(`limit=${terminalLimit}`);
    const daemon = fakeDaemon([card("r", "ready", 10, minute(0)), card("d", "done", 10, minute(1))]);
    const data = await fetchBoard({ run_state: "waiting" }, daemon.get);
    expect(daemon.paths).toHaveLength(1);
    expect(daemon.paths[0]).toContain("run_state=waiting");
    expect(data.terminal).toEqual({});
  });
});
