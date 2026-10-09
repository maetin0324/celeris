import { describe, expect, it } from "vitest";
import { taskKeys } from "../../api/queries/keys";
import { taskDetailPath, taskDetailQueryKey, taskTimelinePath, taskTimelineQueryKey } from "./task-detail-query";
import {
  MOBILE_SECTIONS,
  mobileSectionClass,
  parseTaskDetailTab,
  sectionForHash,
  TASK_DETAIL_TABS,
} from "./task-detail-tabs";

describe("task detail tabs", () => {
  it("parses known tabs and falls back to overview", () => {
    expect(parseTaskDetailTab("timeline")).toBe("timeline");
    expect(parseTaskDetailTab("overview")).toBe("overview");
    expect(parseTaskDetailTab("changes")).toBe("changes");
    expect(parseTaskDetailTab("files")).toBe("files");
    expect(parseTaskDetailTab("artifacts")).toBe("artifacts");
    expect(parseTaskDetailTab(undefined)).toBe("overview");
    expect(parseTaskDetailTab("bogus")).toBe("overview");
    expect(TASK_DETAIL_TABS.map((tab) => tab.key)).toEqual(["overview", "timeline", "changes", "files", "artifacts"]);
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

describe("task detail mobile sections", () => {
  it("opens the section that holds the hash target and keeps desktop layout", () => {
    expect(MOBILE_SECTIONS.map((section) => section.key)).toEqual(["summary", "decision", "execution", "tree"]);
    expect(sectionForHash("#decision-panel")).toBe("decision");
    expect(sectionForHash("#decision-01ABC")).toBe("decision");
    expect(sectionForHash("task-decisions")).toBe("decision");
    expect(sectionForHash("execution-panel")).toBe("execution");
    expect(sectionForHash("integration-repair")).toBe("tree");
    expect(sectionForHash("")).toBeNull();
    expect(sectionForHash(undefined)).toBeNull();
    expect(mobileSectionClass("tree", "tree")).toContain("md:contents");
    expect(mobileSectionClass("tree", "summary")).toBe("hidden md:contents");
    expect(mobileSectionClass("tree", undefined)).not.toContain("hidden");
  });
});
