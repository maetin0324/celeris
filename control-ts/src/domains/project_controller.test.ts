// project_controller の方針の期待値（ADR 2026-11-04-project-controller D7）。ここは control-ts/src の中なので、
// 方針の変更は TS だけの経路（cargo 0 回）で変えられる。Rust が守らせる不変条件は fixtures/project_controller/ にある。
import { describe, expect, it } from "vitest";
import {
  decide,
  parseInput,
  projectController,
  type ProjectSnapshot,
  type SnapshotDecision,
  type SnapshotRoot,
} from "./project_controller";

function snapshot(): ProjectSnapshot {
  return {
    now: "2026-11-04T00:00:00Z",
    truncated: false,
    project: { id: "P1", title: "案件", status: "active", request: "依頼", request_sha256: "a".repeat(64), auto_integrate: false },
    brief: null,
    outcomes: [
      { id: "o1", title: "中間", acceptance: ["A1"], depends_on: [], priority: 0, source: "request_block" },
      { id: "o2", title: "最終", acceptance: ["B1", "B2"], depends_on: ["o1"], priority: 0, source: "request_block" },
    ],
    outcomes_invalid: false,
    roots: [],
    waits: [],
    stagnation: [],
    wakes: [],
    decisions: [],
    controller: { mode: "enabled", phase: "active", revision: 2, human_epoch: 1, retry_count: 0, progress_epoch: 0 },
    limits: {
      max_open_roots: 2,
      max_root_attempts_per_outcome: 2,
      max_no_progress_decisions: 5,
      cos_inputs_remaining_today: 10,
      roots_remaining_today: 5,
    },
    roadmap: null,
    authority: null,
  };
}

function root(id: string, key: string, over: Partial<SnapshotRoot> = {}): SnapshotRoot {
  const o = key.split(":")[1]!;
  return {
    id,
    status: "done",
    title: id,
    controller_key: key,
    outcome_ids: [o],
    acceptance: [],
    reviewer_passed: true,
    integrated: true,
    failure_class: null,
    attempts: 0,
    open_waits: [],
    leaf_done: 1,
    leaf_total: 1,
    continues_root_ids: [],
    ...over,
  };
}

function decision(kind: string, over: Partial<SnapshotDecision> = {}): SnapshotDecision {
  return {
    id: `d-${kind}`,
    kind,
    rule_id: "x",
    effect_state: "applied",
    root_key: null,
    reason_code: null,
    outcome_id: null,
    request_sha256: "a".repeat(64),
    progress_epoch: 0,
    at: "2026-11-04T00:00:00Z",
    ...over,
  };
}

describe("project_controller", () => {
  it("launches the first outcome with its acceptance verbatim", () => {
    const p = decide(snapshot());
    expect(p.action.kind).toBe("launch_root");
    if (p.action.kind !== "launch_root") return;
    expect(p.action.root.key).toBe("outcome:o1:1");
    expect(p.action.root.acceptance.map((a) => a.text)).toEqual(["A1"]);
    expect(p.action.root.outcome_ids).toEqual(["o1"]);
    expect(p.based_on).toEqual({ revision: 2, human_epoch: 1, request_sha256: "a".repeat(64) });
    expect(p.gap).toEqual({ reached: [], open: ["o1", "o2"], unknown: [], progress_epoch: 0 });
  });

  it("marks the reached milestone first, then continues with root2", () => {
    const s = snapshot();
    s.roots = [root("R1", "outcome:o1:1")];
    const p1 = decide(s);
    expect(p1.action).toEqual({ kind: "mark_milestone", outcome_id: "o1", evidence_refs: ["task:R1"] });
    s.decisions = [decision("mark_milestone", { outcome_id: "o1" })];
    const p2 = decide(s);
    expect(p2.action.kind).toBe("launch_root");
    if (p2.action.kind !== "launch_root") return;
    expect(p2.action.root.key).toBe("outcome:o2:1");
    expect(p2.action.root.continues_root_ids).toEqual(["R1"]);
    expect(p2.action.root.acceptance.map((a) => a.text)).toEqual(["B1", "B2"]);
    expect(p2.gap.reached).toEqual(["o1"]);
  });

  it("waits as achieved when every outcome is reached and recorded", () => {
    const s = snapshot();
    s.roots = [root("R1", "outcome:o1:1"), root("R2", "outcome:o2:1")];
    s.decisions = [decision("mark_milestone", { outcome_id: "o1" }), decision("mark_milestone", { outcome_id: "o2" })];
    expect(decide(s).action).toEqual({ kind: "wait", waiting_on: [], next_wake_after_secs: 0, phase: "achieved" });
  });

  it("does not count a done but unintegrated root and watches it", () => {
    const s = snapshot();
    s.roots = [root("R1", "outcome:o1:1", { integrated: false })];
    const p = decide(s);
    expect(p.action).toMatchObject({ kind: "continue", watch_root_ids: ["R1"] });
    expect(p.gap.reached).toEqual([]);
  });

  it("asks the project CoS once after a review failure", () => {
    const s = snapshot();
    s.roots = [root("R1", "outcome:o1:1", { status: "failed", reviewer_passed: false, failure_class: "review_failed" })];
    expect(decide(s).action).toMatchObject({ kind: "replan", reason_code: "review_failed" });
    s.decisions = [decision("replan", { root_key: "outcome:o1:1", reason_code: "review_failed" })];
    expect(decide(s).action.kind).toBe("wait");
  });

  it("waits with backoff after an infrastructure failure", () => {
    const s = snapshot();
    s.roots = [root("R1", "outcome:o1:1", { status: "failed", reviewer_passed: false, failure_class: "infra" })];
    expect(decide(s).action).toMatchObject({ kind: "wait", next_wake_after_secs: 1800, phase: "waiting" });
  });

  it("asks for outcomes once per request version", () => {
    const s = snapshot();
    s.outcomes = [];
    expect(decide(s).action).toMatchObject({ kind: "replan", reason_code: "no_outcomes" });
    s.decisions = [decision("replan", { reason_code: "no_outcomes" })];
    expect(decide(s).action.kind).toBe("wait");
    s.project.request_sha256 = "b".repeat(64);
    s.decisions = [decision("replan", { reason_code: "no_outcomes" })];
    expect(decide(s).action.kind).toBe("replan");
  });

  it("waits on an open human decision instead of launching", () => {
    const s = snapshot();
    s.waits = [{ kind: "decision", id: "q1", task_id: "T1", age_secs: 10 }];
    expect(decide(s).action).toMatchObject({ kind: "wait", waiting_on: [{ kind: "human", target: "T1" }] });
  });

  it("does not launch on a paused project and escalates exhausted attempts once", () => {
    const s = snapshot();
    s.project.status = "paused";
    expect(decide(s).action.kind).toBe("wait");
    const t = snapshot();
    t.roots = [
      root("R1", "outcome:o1:1", { status: "done", reviewer_passed: false, integrated: false }),
      root("R2", "outcome:o1:2", { status: "done", reviewer_passed: false, integrated: false }),
    ];
    expect(decide(t).action).toMatchObject({ kind: "escalate", category: "explicit_human", question_key: "attempts_exhausted:o1" });
    t.decisions = [decision("escalate", { root_key: "attempts_exhausted:o1" })];
    expect(decide(t).action.kind).toBe("wait");
  });

  it("rejects malformed input without echoing the request", () => {
    expect(() => parseInput([])).toThrow("not an object");
    const bad = { ...snapshot(), extra: 1 } as unknown;
    expect(() => projectController(bad)).toThrow("unknown field extra");
    const s = snapshot() as unknown as Record<string, unknown>;
    delete s.limits;
    expect(() => projectController(s)).toThrow("missing limits");
  });
});
