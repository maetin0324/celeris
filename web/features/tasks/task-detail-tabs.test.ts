import { describe, expect, it } from "vitest";
import { taskKeys } from "../../api/queries/keys";
import { taskDetailPath, taskDetailQueryKey, taskTimelinePath, taskTimelineQueryKey } from "./task-detail-query";
import { parseTaskDetailTab, TASK_DETAIL_TABS } from "./task-detail-tabs";

describe("task detail tabs", () => {
  it("parses known tabs and falls back to overview", () => {
    expect(parseTaskDetailTab("timeline")).toBe("timeline");
    expect(parseTaskDetailTab("overview")).toBe("overview");
    expect(parseTaskDetailTab(undefined)).toBe("overview");
    expect(parseTaskDetailTab("bogus")).toBe("overview");
    expect(TASK_DETAIL_TABS.map((tab) => tab.key)).toEqual(["overview", "timeline"]);
  });
});

describe("task detail queries", () => {
  it("uses the shared keys so task events refetch only detail and timeline", () => {
    expect(taskDetailQueryKey("T 1")).toEqual(taskKeys.detail("T 1"));
    expect(taskTimelineQueryKey("T1").slice(0, 3)).toEqual(taskKeys.timelines("T1"));
    expect(taskDetailPath("T 1")).toBe("/api/tasks/T%201");
    expect(taskTimelinePath("T1")).toBe("/api/tasks/T1/timeline");
  });
});
