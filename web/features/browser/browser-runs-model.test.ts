import { describe, expect, it } from "vitest";
import type { BrowserWait } from "../../api/generated/types";
import type { BrowserRunItem } from "./browser-query";
import { buildRunRows, leaseBadge, liveCell } from "./browser-runs-model";

function run(
  task: string,
  id: string,
  state: BrowserRunItem["state"],
  extra: Partial<BrowserRunItem> = {},
): BrowserRunItem {
  return {
    task_id: task,
    run_id: id,
    session_id: `S-${id}`,
    state,
    live_path: `/browser/live/${task}/${id}`,
    ...extra,
  };
}

function wait(task: string, runId: string, reason: BrowserWait["reason"], state: BrowserWait["state"] = "pending") {
  return { wait_id: `W-${runId}-${reason}`, task_id: task, run_id: runId, reason, state } as BrowserWait;
}

describe("browser runs list model", () => {
  it("names the three lease badges and hides the badge for unknown phases", () => {
    expect(leaseBadge("agent_running")).toEqual({ label: "監視のみ", tone: "neutral" });
    expect(leaseBadge("pausing")?.label).toBe("監視のみ");
    expect(leaseBadge("human_control")).toEqual({ label: "操作中", tone: "warning" });
    expect(leaseBadge("paused")).toEqual({ label: "一時停止中（人の返却待ち）", tone: "warning" });
    expect(leaseBadge("stopped")?.label).toBe("停止");
    expect(leaseBadge(undefined)).toBeNull();
  });

  it("orders runs needing a human first, then running, then ended (newest run first in each group)", () => {
    const rows = buildRunRows(
      [
        run("T1", "R0", "COMPLETED"),
        run("T1", "R1", "RUNNING"),
        run("T2", "R2", "RUNNING"),
        run("T3", "R3", "RUNNING"),
      ],
      [wait("T1", "R1", "waiting_for_approval"), wait("T2", "R2", "waiting_for_auth", "registered")],
      { phases: new Map([["T3/R3", "paused" as const]]), titles: new Map([["T1", "請求書フォームの入力"]]) },
    );
    expect(rows.map((r) => `${r.taskId}/${r.runId}`)).toEqual(["T3/R3", "T1/R1", "T2/R2", "T1/R0"]);
    const r1 = rows.find((r) => r.runId === "R1");
    expect(r1).toMatchObject({ taskTitle: "請求書フォームの入力", openWaits: 1, stateLabel: "実行中", lease: null });
    // 解決済みの待ちは数えない。
    expect(rows.find((r) => r.runId === "R2")?.openWaits).toBe(0);
    const r3 = rows.find((r) => r.runId === "R3");
    expect(r3).toMatchObject({ stateLabel: "一時停止中（人の返却待ち）", stateTone: "warning" });
    expect(r3?.lease?.label).toBe("一時停止中（人の返却待ち）");
    // 終了した run は lease を持たない。
    expect(rows.find((r) => r.runId === "R0")).toMatchObject({ active: false, lease: null, stateLabel: "終了" });
  });

  it("tells a loading lease from a failed control lookup", () => {
    const runs = [run("T1", "R1", "RUNNING"), run("T2", "R2", "RUNNING"), run("T1", "R0", "COMPLETED")];
    const rows = buildRunRows(runs, [], { controlFailed: new Set(["T2/R2"]) });
    expect(rows.find((r) => r.runId === "R1")?.leasePending).toBe("loading");
    expect(rows.find((r) => r.runId === "R2")?.leasePending).toBe("unavailable");
    expect(rows.find((r) => r.runId === "R0")?.leasePending).toBeNull();
    const known = buildRunRows(runs, [], { phases: new Map([["T1/R1", "human_control" as const]]) });
    expect(known.find((r) => r.runId === "R1")).toMatchObject({ leasePending: null, lease: { label: "操作中" } });
  });

  it("explains why the live view is unavailable", () => {
    expect(liveCell(run("T1", "R1", "RUNNING"))).toEqual({ available: true, label: "Live View を開けます" });
    expect(liveCell(run("T1", "R0", "COMPLETED"))).toMatchObject({ available: false, reason: "not_running" });
    expect(liveCell(run("T1", "R1", "RUNNING"))).toMatchObject({ available: true });
    expect(liveCell(run("T1", "R1", "RUNNING", { live_path: "https://upstream.invalid/x" }))).toMatchObject({
      available: false,
      reason: "not_configured",
    });
    expect(
      liveCell(run("T1", "R1", "RUNNING", { live: { state: "disabled", reason: "relay_unavailable" } })),
    ).toMatchObject({ available: false, reason: "relay_unavailable", label: expect.stringContaining("中継") });
    // 知らない理由は中継不可として扱う（任意の文字列を画面に出さない）。
    expect(liveCell(run("T1", "R1", "RUNNING", { live: { state: "disabled", reason: "<x>" } }))).toMatchObject({
      reason: "relay_unavailable",
    });
    // gateway が frame 経路の grant・guard の拒否理由を返したら、中継不可に丸めずその理由を出す。
    for (const reason of ["observation_stopped", "run_ended", "grant_denied", "attestation_unavailable"])
      expect(liveCell(run("T1", "R1", "RUNNING", { live: { state: "disabled", reason } }))).toMatchObject({
        available: false,
        reason,
      });
  });

  it("never carries a raw live_view_url into the rows", () => {
    const raw = { ...run("T1", "R1", "RUNNING"), live_view_url: "https://upstream.invalid/raw" } as BrowserRunItem;
    expect(JSON.stringify(buildRunRows([raw], []))).not.toContain("upstream.invalid");
  });
});
