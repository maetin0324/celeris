import { describe, expect, test } from "vitest";
import type { TaskWait } from "../../api/generated/types";
import { waitReason, waitTargets } from "./wait-reason";

describe("task wait reasons", () => {
  test("summarizes dependency waits and keeps navigable task ids", () => {
    const wait: TaskWait = { kind: "depends_on", task_ids: ["T2", "T3"] };
    expect(waitReason(wait)).toBe("依存待ち: T2");
    expect(waitTargets(wait)).toEqual(["T2", "T3"]);
  });
  test("summarizes child and work unit waits", () => {
    expect(waitReason({ kind: "children", task_ids: ["T2", "T3"], unit_keys: ["build"] })).toBe("子 2 件待ち");
    expect(
      waitReason({
        kind: "work_units",
        units: [{ key: "dispatch-prune", stage: "remove", waiting_on: ["cos-route"], waiting_stage: null }],
      }),
    ).toBe("WU 待ち: dispatch-prune ← cos-route");
  });
});
