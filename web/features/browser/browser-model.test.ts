import { describe, expect, it } from "vitest";
import {
  browserRunBadge,
  browserWaitKind,
  controlPhaseText,
  leaseSecondsRemaining,
  liveUnavailableText,
  safeLivePath,
  shouldEmphasizeRenewal,
} from "./browser-model";

describe("browser display model", () => {
  it("names the six control states including both lease holders", () => {
    expect(controlPhaseText("agent_running")).toBe("監視のみ — エージェントが操作中");
    expect(controlPhaseText("pausing", { inFlight: 2 })).toBe("一時停止を待っています（実行中 2 件）");
    expect(controlPhaseText("paused")).toBe("一時停止中 — 引き継げます");
    expect(controlPhaseText("human_control", { isMine: true, remainingSeconds: 15 })).toBe(
      "あなたが操作中 — 残り 15 秒",
    );
    expect(controlPhaseText("human_control", { isMine: false })).toBe("他のセッションが操作中");
    expect(controlPhaseText("stopped")).toBe("停止");
    expect(browserRunBadge({ state: "RUNNING" }, "paused")).toBe("一時停止中（人の返却待ち）");
  });

  it("has a message for all seven live unavailability reasons", () => {
    expect(Object.keys(liveUnavailableText).sort()).toEqual([
      "grant_expired",
      "launcher_protocol_no_live_frames",
      "not_configured",
      "not_owner",
      "not_running",
      "owner_unavailable",
      "relay_unavailable",
    ]);
    for (const message of Object.values(liveUnavailableText)) expect(message.length).toBeGreaterThan(0);
  });

  it("allows only an exact same-origin live path", () => {
    expect(safeLivePath("/browser/live/T_1/R-2")).toBe("/browser/live/T_1/R-2");
    for (const href of [
      "https://example.com/browser/live/T/R",
      "//example.com/browser/live/T/R",
      "/browser/live/T/R/extra",
      "/browser/live/T/../R",
      "/browser/live/T/R?x=1",
      "/api/browser/live/T/R",
      "https://upstream.invalid",
      null,
    ])
      expect(safeLivePath(href)).toBeNull();
  });

  it("counts down without going below zero and marks the last 15 seconds", () => {
    expect(leaseSecondsRemaining(null, 100)).toBeNull();
    expect(leaseSecondsRemaining(116, 100)).toBe(16);
    expect(leaseSecondsRemaining(115, 100)).toBe(15);
    expect(leaseSecondsRemaining(99, 100)).toBe(0);
    expect(shouldEmphasizeRenewal(16)).toBe(false);
    expect(shouldEmphasizeRenewal(15)).toBe(true);
    expect(shouldEmphasizeRenewal(0)).toBe(true);
    expect(shouldEmphasizeRenewal(null)).toBe(false);
  });

  it("routes only approval waits to decisions and auth waits to credentials", () => {
    expect(browserWaitKind({ reason: "waiting_for_approval" })).toBe("decision");
    expect(browserWaitKind({ reason: "waiting_for_auth" })).toBe("credential");
  });
});
