import { QueryClient, type QueryKey, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import schemaJson from "../generated/schema.json";
import type { EventRow } from "../generated/types";
import { daemonKeys, projectKeys, taskKeys } from "../queries/keys";
import { createConnectionStore } from "./connection-state";
import { EVENT_KINDS } from "./event-kinds";
import { FakeEventSource, taskEventRow } from "./fake-event-source";
import { EVENT_INVALIDATION, keysForTaskEvent, resolveProjectId } from "./invalidation-map";
import { createInvalidator } from "./invalidator";
import { createRealtime } from "./realtime";

const schema = schemaJson as unknown as {
  $defs: { Event: { oneOf: Array<{ properties: { type: { const: string } } }> } };
};
const schemaKinds = schema.$defs.Event.oneOf.map((o) => o.properties.type.const);

const has = (keys: QueryKey[], key: QueryKey) => keys.some((k) => JSON.stringify(k) === JSON.stringify(key));
const hasPrefix = (keys: QueryKey[], head: string) => keys.some((k) => k[0] === head);

describe("event kind coverage", () => {
  it("every kind in schema.json is in the map and in EVENT_KINDS (and nothing else)", () => {
    const missing = schemaKinds.filter((k) => !(k in EVENT_INVALIDATION));
    expect(missing).toEqual([]);
    expect([...EVENT_KINDS].sort()).toEqual([...schemaKinds].sort());
    expect(Object.keys(EVENT_INVALIDATION).sort()).toEqual([...schemaKinds].sort());
  });
});

// ADR-0081 D6 の表を独立に書き直したもの（36 種）。T は全種類の timeline を含むので省略している。
const ADR_TABLE: Record<string, string[]> = {
  created: ["T", "L", "P"],
  transitioned: ["T", "L", "P", "N", "E"],
  worker_started: ["T", "R", "E", "L", "P"],
  artifact_produced: ["T", "R", "artifacts", "P"],
  worker_finished: ["T", "R", "E", "L", "P", "N", "changes", "files", "artifacts", "metrics"],
  review_verdict: ["T", "R", "E", "L"],
  approval_requested: ["T", "L", "N", "E", "P"],
  approval_decided: ["T", "L", "N", "E", "P"],
  approvals_withdrawn: ["T", "L", "N", "E", "P"],
  answered: ["T", "L", "N", "R", "E"],
  question_raised: ["T", "L", "N", "R", "E"],
  delegated: ["T", "R", "E", "L", "P"],
  cluster_unavailable: ["T", "L", "clusters", "daemonRest"],
  cluster_master_exited: ["T", "L", "clusters", "daemonRest"],
  provider_throttled: ["T", "providers", "accounts", "daemonRest"],
  retried: ["T", "L", "P"],
  edited: ["T", "L", "P", "E"],
  assigned: ["T", "L", "P", "E"],
  workspace_mode_downgraded: ["T", "files", "changes", "artifacts", "metrics"],
  workspace_pruned: ["T", "files", "changes", "artifacts", "metrics"],
  routing_decided: ["T", "R"],
  checkpoint_saved: ["T", "R", "E"],
  execution_planned: ["T", "E", "R", "L", "P"],
  work_unit_transitioned: ["T", "E", "R", "L", "P"],
  execution_gated: ["T", "E", "R", "L", "P"],
  execution_hint_set: ["T", "E", "R", "L", "P"],
  repair_scheduled: ["T", "E", "R", "L", "P"],
  quota_estimated: ["T", "R", "E", "accounts", "providers", "metrics", "P"],
  work_unit_committed: ["T", "E", "changes", "files", "artifacts", "P"],
  phase_integrated: ["T", "E", "changes", "files", "artifacts", "P"],
  work_units_serialized: ["T", "E"],
  pause_points_resolved: ["T", "E"],
  phase_reported: ["T", "E", "artifacts", "L", "N", "P"],
  project_plan_proposed: ["T", "L", "N"],
  project_plan_decided: ["T", "L", "N"],
};

function expectedKeys(tokens: string[]): QueryKey[] {
  const t = "T1";
  const out: QueryKey[] = [taskKeys.timelines(t)];
  const table: Record<string, QueryKey[]> = {
    T: [taskKeys.detail(t)],
    L: [taskKeys.lists(), ["inbox"], ["board"]],
    N: [["reports"], ["approvals"], daemonKeys.rest()],
    R: [taskKeys.runs(t), taskKeys.runsOf(t)],
    E: [taskKeys.execution(t)],
    files: [taskKeys.files(t)],
    changes: [taskKeys.changes(t)],
    artifacts: [taskKeys.artifacts(t)],
    metrics: [["metrics"]],
    clusters: [["clusters"]],
    providers: [["providers"]],
    accounts: [["accounts"]],
    daemonRest: [daemonKeys.rest()],
    P: [projectKeys.lists(), ["projects", "detail"], ["projects", "tasks"], ["projects", "plan"]],
  };
  for (const token of tokens) out.push(...(table[token] ?? []));
  return out;
}

function fixtureEvent(kind: string): EventRow["event"] {
  const extra: Record<string, unknown> = {
    run_id: "R1",
    task_ids: ["C1"],
    from: "T0",
    project_id: "P1",
    child_task_id: "C2",
  };
  return { type: kind, ...extra } as unknown as EventRow["event"];
}

describe("D6 table fixtures", () => {
  for (const [kind, tokens] of Object.entries(ADR_TABLE)) {
    it(`${kind} invalidates the D6 set`, () => {
      // 所属不明（fallback）で評価する。
      const keys = keysForTaskEvent({ taskId: "T1", event: fixtureEvent(kind), projectId: null });
      for (const key of expectedKeys(tokens)) expect(has(keys, key), JSON.stringify(key)).toBe(true);
      if (!tokens.includes("P") && kind !== "project_plan_proposed" && kind !== "project_plan_decided") {
        expect(hasPrefix(keys, "projects")).toBe(false);
      }
    });
  }
  it("covers 36 documented kinds and every other schema kind still maps to timeline", () => {
    expect(Object.keys(ADR_TABLE)).toHaveLength(35);
    for (const kind of schemaKinds) {
      const keys = keysForTaskEvent({ taskId: "T1", event: fixtureEvent(kind), projectId: null });
      expect(has(keys, taskKeys.timelines("T1")), kind).toBe(true);
    }
  });
  it("worker_progress touches only the run, its log and the task timeline", () => {
    const keys = keysForTaskEvent({ taskId: "T1", event: fixtureEvent("worker_progress"), projectId: "P1" });
    expect(keys).toEqual([
      ["tasks", "timeline", "T1"],
      ["tasks", "run", "T1", "R1"],
      ["tasks", "log", "T1", "R1"],
    ]);
  });
  it("project_plan_* uses the explicit project_id", () => {
    const keys = keysForTaskEvent({ taskId: "T1", event: fixtureEvent("project_plan_decided"), projectId: "P1" });
    expect(has(keys, projectKeys.detail("P1"))).toBe(true);
    expect(has(keys, projectKeys.plan("P1"))).toBe(true);
    expect(has(keys, ["projects", "detail"])).toBe(false);
  });
  it("known project narrows P; unknown falls back to aggregates but never docs", () => {
    const known = keysForTaskEvent({ taskId: "T1", event: fixtureEvent("transitioned"), projectId: "P1" });
    expect(has(known, projectKeys.tasksOf("P1"))).toBe(true);
    expect(has(known, ["projects", "tasks"])).toBe(false);
    const unknown = keysForTaskEvent({ taskId: "T1", event: fixtureEvent("transitioned"), projectId: null });
    expect(has(unknown, ["projects", "tasks"])).toBe(true);
    expect(unknown.some((k) => k[1] === "docs")).toBe(false);
  });
});

describe("H1 project resolution from the Query cache", () => {
  it("uses detail, then lists, then unknown", () => {
    const qc = new QueryClient();
    expect(resolveProjectId(qc, "T1", fixtureEvent("transitioned"))).toBeNull();
    qc.setQueryData(taskKeys.list({}), { items: [{ id: "T1", project_id: "PL" }] });
    expect(resolveProjectId(qc, "T1", fixtureEvent("transitioned"))).toBe("PL");
    qc.setQueryData(taskKeys.detail("T1"), { id: "T1", project_id: "PD" });
    expect(resolveProjectId(qc, "T1", fixtureEvent("transitioned"))).toBe("PD");
    const created = { type: "created", task: { project_id: "PC" } } as unknown as EventRow["event"];
    expect(resolveProjectId(qc, "T1", created)).toBe("PC");
  });
});

describe("coalescing and in-flight limits", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function observe(
    qc: QueryClient,
    queryKey: QueryKey,
    delayMs: number,
    counter: { n: number; active: number; max: number },
  ) {
    const observer = new QueryObserver(qc, {
      queryKey,
      staleTime: 60_000,
      queryFn: async () => {
        counter.n++;
        counter.active++;
        counter.max = Math.max(counter.max, counter.active);
        await new Promise((r) => setTimeout(r, delayMs));
        counter.active--;
        return counter.n;
      },
    });
    return observer.subscribe(() => {});
  }
  const newCounter = () => ({ n: 0, active: 0, max: 0 });

  it("a burst of 20 events in 1 s with a 10 s fetch keeps one in-flight fetch per key", async () => {
    const qc = new QueryClient();
    const timeline = newCounter();
    const detail = newCounter();
    const list = newCounter();
    observe(qc, taskKeys.timeline("T1", {}), 10_000, timeline);
    observe(qc, taskKeys.detail("T1"), 10_000, detail);
    observe(qc, taskKeys.list({}), 10_000, list);
    await vi.advanceTimersByTimeAsync(10_000);
    const base = { t: timeline.n, d: detail.n, l: list.n };
    const inv = createInvalidator(qc);
    for (let i = 1; i <= 20; i++) {
      inv.onTaskEvent(taskEventRow(i, "transitioned") as unknown as EventRow);
      await vi.advanceTimersByTimeAsync(50);
    }
    await vi.advanceTimersByTimeAsync(60_000);
    for (const c of [timeline, detail, list]) expect(c.max).toBe(1);
    // 初回 1 本 + 取得中に来た分を完了後に 1 回だけ。
    expect(timeline.n - base.t).toBeLessThanOrEqual(2);
    expect(detail.n - base.d).toBeLessThanOrEqual(2);
    expect(list.n - base.l).toBeLessThanOrEqual(2);
    expect(timeline.n - base.t).toBeGreaterThanOrEqual(1);
    inv.dispose();
  });

  it("same key within 250 ms is fetched once", async () => {
    const qc = new QueryClient();
    const c = newCounter();
    observe(qc, taskKeys.detail("T1"), 0, c);
    await vi.advanceTimersByTimeAsync(10);
    const inv = createInvalidator(qc);
    inv.enqueue([taskKeys.detail("T1")]);
    await vi.advanceTimersByTimeAsync(100);
    inv.enqueue([taskKeys.detail("T1")]);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(c.n).toBe(2);
    inv.dispose();
  });

  it("duplicate and reversed event ids do not refetch twice (through the transport)", async () => {
    const qc = new QueryClient();
    const c = newCounter();
    observe(qc, taskKeys.detail("T1"), 0, c);
    await vi.advanceTimersByTimeAsync(10);
    FakeEventSource.reset();
    const rt = createRealtime({
      queryClient: qc,
      store: createConnectionStore(),
      createEventSource: FakeEventSource.create,
      probe: async () => "ok",
    });
    rt.start();
    const es = FakeEventSource.last();
    es.emit("task.event", taskEventRow(5, "transitioned"));
    await vi.advanceTimersByTimeAsync(1_000);
    expect(c.n).toBe(2);
    es.emit("task.event", taskEventRow(5, "transitioned"));
    es.emit("task.event", taskEventRow(4, "transitioned"));
    await vi.advanceTimersByTimeAsync(1_000);
    expect(c.n).toBe(2);
    rt.dispose();
  });

  it("worker_progress does not refetch project, list or settings queries", async () => {
    const qc = new QueryClient();
    const project = newCounter();
    const list = newCounter();
    const config = newCounter();
    const timeline = newCounter();
    observe(qc, projectKeys.detail("P1"), 0, project);
    observe(qc, taskKeys.list({}), 0, list);
    observe(qc, ["config", {}], 0, config);
    observe(qc, taskKeys.timeline("T1", {}), 0, timeline);
    await vi.advanceTimersByTimeAsync(10);
    const inv = createInvalidator(qc);
    inv.onTaskEvent(taskEventRow(1, "worker_progress", "T1", { run_id: "R1" }) as unknown as EventRow);
    await vi.advanceTimersByTimeAsync(1_000);
    expect([project.n, list.n, config.n, timeline.n]).toEqual([1, 1, 1, 2]);
    inv.dispose();
  });

  it("reset stales everything and refetches only active queries, leaving daemon/stream alone", async () => {
    const qc = new QueryClient();
    const active = newCounter();
    observe(qc, taskKeys.detail("T1"), 0, active);
    const inactive = newCounter();
    // inactive: 購読なしで取得済みにする
    await qc.fetchQuery({
      queryKey: projectKeys.detail("P1"),
      queryFn: async () => {
        inactive.n++;
        return 1;
      },
    });
    qc.setQueryData(daemonKeys.stream(), { tick: 1 });
    await vi.advanceTimersByTimeAsync(10);
    const inv = createInvalidator(qc);
    inv.onReset();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(active.n).toBe(2);
    expect(inactive.n).toBe(1);
    expect(qc.getQueryState(projectKeys.detail("P1"))?.isInvalidated).toBe(true);
    expect(qc.getQueryState(daemonKeys.stream())?.isInvalidated).toBe(false);
    inv.dispose();
  });
});

describe("P4-02 project membership from project detail", () => {
  it("resolves via ProjectDetail.tasks and marks known non-members as false", () => {
    const qc = new QueryClient();
    qc.setQueryData(projectKeys.detail("P1"), { project: { id: "P1" }, tasks: [{ id: "T1", title: "t" }] });
    expect(resolveProjectId(qc, "T1", fixtureEvent("transitioned"))).toBe("P1");
    expect(resolveProjectId(qc, "X9", fixtureEvent("transitioned"))).toBeNull();
    qc.setQueryData(taskKeys.list({}), { items: [{ id: "X9", title: "outside" }] });
    expect(resolveProjectId(qc, "X9", fixtureEvent("transitioned"))).toBe(false);
    const keys = keysForTaskEvent({ taskId: "X9", event: fixtureEvent("transitioned"), projectId: false });
    expect(keys.some((k) => k[0] === "projects")).toBe(false);
  });
});
