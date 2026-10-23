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
  test("names the stalled stage and units of a plan that cannot dispatch", () => {
    expect(
      waitReason({
        kind: "plan_stalled",
        stage: "gap-c",
        unit_keys: ["chat-port", "parity-final"],
        detail: "計画の都合で起こせる unit が無い",
      }),
    ).toBe("計画が詰まっている: gap-c（chat-port, parity-final）");
  });
  test("tells the human gates apart", () => {
    expect(waitReason({ kind: "human_gate", gate: "plan_approval", stage: null })).toBe("計画の承認待ち");
    expect(waitReason({ kind: "human_gate", gate: "stage_review", stage: "edges" })).toBe("段の確認待ち: edges");
    expect(waitReason({ kind: "human_gate", gate: "phase_checkpoint", stage: "remove" })).toBe("途中確認待ち: remove");
    expect(waitReason({ kind: "human_gate", gate: "phase_checkpoint", stage: null })).toBe("途中確認待ち");
  });
});

describe("credential wait", () => {
  test("API キー待ちの unit を WU 停止中と区別する", () => {
    expect(waitReason({ kind: "units_blocked", reason: "waiting_for_auth", unit_keys: ["u1"] })).toBe(
      "API キーの登録待ち（登録で自動再開）",
    );
    expect(waitReason({ kind: "units_blocked", reason: "infra", unit_keys: ["u1"] })).toBe("WU 停止中");
  });
});
