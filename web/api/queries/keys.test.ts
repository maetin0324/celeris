import { hashKey } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { daemonBadges, inboxCountsBadge } from "./badges";
import { daemonKeys, inboxKeys, projectKeys, queryKeys, taskKeys } from "./keys";
import { normalizeFilters } from "./normalize";
import { STALE_TIME_DEFAULTS, staleTimeFor } from "./stale-time";

describe("key factory", () => {
  it("key に ID を含める", () => {
    expect(taskKeys.detail("t1")).toEqual(["tasks", "detail", "t1"]);
    expect(taskKeys.run("t1", "r2")).toEqual(["tasks", "run", "t1", "r2"]);
    expect(projectKeys.tasks("p1", { status: "open" })).toEqual(["projects", "tasks", "p1", { status: "open" }]);
    expect(projectKeys.docs("p1", "a/b.md")).toEqual(["projects", "docs", "p1", "a/b.md"]);
  });

  it("同じ条件の search / filter は同じ key になる", () => {
    const a = taskKeys.list({ q: " build ", status: ["running", "queued", "running"], page: 2, owner: "" });
    const b = taskKeys.list({ page: 2, status: ["queued", "running"], q: "build", tag: undefined, label: [] });
    expect(hashKey(a)).toBe(hashKey(b));
    expect(a[2]).toEqual({ page: 2, q: "build", status: ["queued", "running"] });
    expect(hashKey(taskKeys.list({ page: 3 }))).not.toBe(hashKey(taskKeys.list({ page: 2 })));
    expect(taskKeys.list()).toEqual(taskKeys.list({}));
  });

  it("normalizeFilters は NaN と null を落とし boolean を残す", () => {
    expect(normalizeFilters({ offset: Number.NaN, archived: false, x: null })).toEqual({ archived: false });
  });

  it("一覧 key は domain の prefix の下にある（invalidate の単位）", () => {
    expect(taskKeys.list({ a: "1" }).slice(0, 2)).toEqual(taskKeys.lists());
    expect(taskKeys.timeline("t1", { kind: "x" }).slice(0, 3)).toEqual(taskKeys.timelines("t1"));
    expect(taskKeys.run("t1", "r1").slice(0, 3)).toEqual(taskKeys.runsOf("t1"));
    expect(projectKeys.tasks("p1", {}).slice(0, 3)).toEqual(projectKeys.tasksOf("p1"));
  });

  it("バッジは画面と同じ key を使い、別の cache を作らない", () => {
    expect(inboxCountsBadge.queryKey).toEqual(inboxKeys.list());
    expect(daemonBadges.queryKey).toEqual(daemonKeys.rest());
    expect(daemonBadges.queryKey).not.toEqual(daemonKeys.stream());
  });
});

describe("staleTime（ADR-0081 D5）", () => {
  it.each([
    [taskKeys.list(), 5_000],
    [taskKeys.detail("t"), 5_000],
    [taskKeys.timeline("t"), 1_000],
    [taskKeys.runs("t"), 1_000],
    [taskKeys.run("t", "r"), 1_000],
    [taskKeys.execution("t"), 5_000],
    [taskKeys.files("t"), 5_000],
    [taskKeys.changes("t"), 5_000],
    [taskKeys.artifacts("t"), 5_000],
    [projectKeys.list(), 10_000],
    [projectKeys.detail("p"), 10_000],
    [projectKeys.tasks("p"), 10_000],
    [projectKeys.plan("p"), 10_000],
    [projectKeys.docs("p", "x"), 60_000],
    [queryKeys.inbox.list(), 5_000],
    [queryKeys.board.list(), 5_000],
    [queryKeys.reports.list(), 5_000],
    [queryKeys.approvals.list(), 5_000],
    [daemonKeys.rest(), 5_000],
    [queryKeys.health.all, 5_000],
    [daemonKeys.stream(), 0],
    [queryKeys.providers.list(), 10_000],
    [queryKeys.accounts.list(), 10_000],
    [queryKeys.clusters.list(), 10_000],
    [queryKeys.releases.list(), 10_000],
    [queryKeys.metrics.list(), 10_000],
    [queryKeys.org.list(), 60_000],
    [queryKeys.knowledge.list(), 60_000],
    [queryKeys.skills.list(), 60_000],
    [queryKeys.config.list(), 60_000],
    [queryKeys.mcp.list(), 60_000],
    [queryKeys.console.conversation("task", "c1"), 0],
  ] as const)("%j は %i ms", (key, ms) => {
    expect(staleTimeFor(key)).toBe(ms);
  });

  it("表の domain はすべて key factory にある", () => {
    const domains = new Set(STALE_TIME_DEFAULTS.map(([prefix]) => prefix[0]));
    const factories = new Set(Object.values(queryKeys).map((k) => (k.all as readonly string[])[0]));
    expect(domains).toEqual(factories);
  });
});
