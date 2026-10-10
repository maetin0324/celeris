import { describe, expect, it } from "vitest";
import type { EventRow, EventsPage } from "../../api/generated/types";
import { liveEventItems, mergeLiveEvents } from "./live-events";

function row(seq: number, taskId: string, event: Record<string, unknown>): EventRow {
  return { id: seq, seq, task_id: taskId, ts: `2026-10-10T00:00:${String(seq).padStart(2, "0")}Z`, event } as EventRow;
}

function page(items: EventRow[]): Pick<EventsPage, "items"> {
  return { items };
}

describe("liveEventItems", () => {
  it("mixes browser_updated and browser.* tool_result progress in seq order for one run", () => {
    const items = liveEventItems(
      page([
        row(1, "T1", { type: "browser_updated", browser: { run_id: "R1", state: "RUNNING" } }),
        row(2, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.credential_use: success",
          kind: "tool_result",
          tool: "browser.credential_use",
          summary: "success",
          error: false,
        }),
        row(3, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.post_login: failure",
          kind: "tool_result",
          tool: "browser.post_login",
          summary: "failure",
          error: true,
        }),
      ]),
      "T1",
      "R1",
    );
    expect(items.map((item) => [item.seq, item.text])).toEqual([
      [1, "状態: 実行中"],
      [2, "browser.credential_use: success"],
      [3, "browser.post_login: failure"],
    ]);
  });

  it("drops other runs, other tasks, non-browser tools and non tool_result kinds", () => {
    const items = liveEventItems(
      page([
        row(1, "T1", {
          type: "worker_progress",
          run_id: "R2",
          msg: "browser.extract: success",
          kind: "tool_result",
          tool: "browser.extract",
        }),
        row(2, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "bash: ls",
          kind: "tool_result",
          tool: "bash",
        }),
        row(3, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.click: started",
          kind: "tool_use",
          tool: "browser.click",
        }),
        row(4, "T2", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.extract: success",
          kind: "tool_result",
          tool: "browser.extract",
        }),
        row(5, "T1", { type: "browser_updated", browser: { run_id: "R2", state: "RUNNING" } }),
        row(6, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.extract: success",
          kind: "tool_result",
          tool: "browser.extract",
        }),
      ]),
      "T1",
      "R1",
    );
    expect(items.map((item) => item.seq)).toEqual([6]);
    expect(items[0]?.text).toBe("browser.extract: success");
  });

  it("does not put live_view_url or other event values into the text", () => {
    const items = liveEventItems(
      page([
        row(1, "T1", {
          type: "browser_updated",
          browser: { run_id: "R1", state: "RUNNING", live_view_url: "https://example.invalid/secret" },
        }),
      ]),
      "T1",
      "R1",
    );
    expect(items).toHaveLength(1);
    expect(items[0]?.text).not.toContain("example.invalid");
  });

  it("returns seq-ordered items even when the page arrives out of order", () => {
    const items = liveEventItems(
      page([
        row(9, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.download: success",
          kind: "tool_result",
          tool: "browser.download",
        }),
        row(4, "T1", { type: "browser_updated", browser: { run_id: "R1", state: "PAUSED" } }),
      ]),
      "T1",
      "R1",
    );
    expect(items.map((item) => item.seq)).toEqual([4, 9]);
  });
});

describe("mergeLiveEvents with mixed rows", () => {
  it("appends only rows after the last seen seq", () => {
    const first = liveEventItems(
      page([
        row(1, "T1", { type: "browser_updated", browser: { run_id: "R1", state: "RUNNING" } }),
        row(2, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.extract: success",
          kind: "tool_result",
          tool: "browser.extract",
        }),
      ]),
      "T1",
      "R1",
    );
    const next = liveEventItems(
      page([
        row(2, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.extract: success",
          kind: "tool_result",
          tool: "browser.extract",
        }),
        row(3, "T1", {
          type: "worker_progress",
          run_id: "R1",
          msg: "browser.post_login: success",
          kind: "tool_result",
          tool: "browser.post_login",
        }),
      ]),
      "T1",
      "R1",
    );
    expect(mergeLiveEvents(first, next).map((item) => item.seq)).toEqual([1, 2, 3]);
  });
});
