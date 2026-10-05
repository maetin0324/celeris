import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ControlStatus } from "./browser-query";
import { ControlBarView, type ControlBarViewProps, controlAnnouncement, controlButtons } from "./control-bar";
import { liveEventItems, mergeLiveEvents } from "./live-events";
import { liveViewState } from "./live-view-frame";

const NOW = 1_790_000_000;
const status = (over: Partial<ControlStatus> = {}): ControlStatus => ({
  phase: "agent_running",
  version: 1,
  lease_expires_at: null,
  lease_holder: null,
  in_flight: 0,
  auth_section: false,
  ...over,
});

function render(over: Partial<ControlBarViewProps> = {}) {
  return renderToStaticMarkup(
    <ControlBarView
      status={status()}
      isMine={false}
      canOperate
      disabledReason={null}
      nowSeconds={NOW}
      pending={false}
      message={null}
      expired={false}
      onCommand={() => undefined}
      {...over}
    />,
  );
}

/** 表示の順にボタンの disabled を取り出す（一時停止・引き継ぐ・延長・返す・停止）。 */
function disabledFlags(html: string): boolean[] {
  return [...html.matchAll(/<button[^>]*>/g)].map((m) => /\sdisabled=""/.test(m[0]));
}

describe("ControlBarView", () => {
  it("shows the monitor-only phase with a polite status and only pause/stop enabled", () => {
    const html = render();
    expect(html).toContain("監視のみ — エージェントが操作中");
    expect(html).toMatch(/role="status" aria-live="polite"/);
    expect(html).toContain('data-operating="false"');
    expect(disabledFlags(html)).toEqual([false, true, true, true, false]);
  });

  it("frames the bar with the warning token while a human operates and shows remaining seconds", () => {
    const html = render({
      status: status({ phase: "human_control", lease_expires_at: NOW + 42 }),
      isMine: true,
    });
    expect(html).toContain("あなたが操作中 — 残り 42 秒");
    expect(html).toContain("border-warning-foreground bg-warning");
    expect(html).toContain('data-emphasis="false"');
    // 延長は押せる。返すは確認 checkbox が付くまで押せない。
    expect(disabledFlags(html)).toEqual([true, true, false, true, false]);
  });

  it("emphasizes renew in the last 15 seconds", () => {
    const html = render({ status: status({ phase: "human_control", lease_expires_at: NOW + 15 }), isMine: true });
    expect(html).toContain('data-emphasis="true"');
    expect(html).toContain("延長（60 秒） — 残り 15 秒");
  });

  it("names the other holder and disables every lease button for that session", () => {
    const html = render({ status: status({ phase: "human_control", lease_expires_at: NOW + 30 }), isMine: false });
    expect(html).toContain("他のセッションが操作中");
    expect(disabledFlags(html)).toEqual([true, true, true, true, true]);
  });

  it("disables all buttons during an auth section and says why", () => {
    const html = render({ status: status({ phase: "paused", auth_section: true }) });
    expect(html).toContain("認証を扱っている間は、すべての操作を止めています。");
    expect(disabledFlags(html)).toEqual([true, true, true, true, true]);
  });

  it("shows pausing, paused (takeover enabled), stopped and the expiry note", () => {
    expect(render({ status: status({ phase: "pausing", in_flight: 2 }) })).toContain(
      "一時停止を待っています（実行中 2 件）",
    );
    const paused = render({ status: status({ phase: "paused" }), expired: true });
    expect(paused).toContain("一時停止中 — 引き継げます");
    expect(paused).toContain("操作の期限が切れたため一時停止しました");
    expect(disabledFlags(paused)).toEqual([true, false, true, true, false]);
    expect(disabledFlags(render({ status: status({ phase: "stopped" }) }))).toEqual([true, true, true, true, true]);
  });
});

describe("controlButtons", () => {
  it("enables resume only after both confirmations, and nothing while a request is pending", () => {
    const paused = status({ phase: "paused" });
    const base = { status: paused, isMine: false, canOperate: true, pending: false, resumeConfirmed: false };
    expect(controlButtons(base).resume).toBe(false);
    expect(controlButtons({ ...base, resumeConfirmed: true }).resume).toBe(true);
    expect(Object.values(controlButtons({ ...base, pending: true })).some(Boolean)).toBe(false);
    expect(Object.values(controlButtons({ ...base, canOperate: false })).some(Boolean)).toBe(false);
  });
});

describe("controlAnnouncement", () => {
  it("changes the spoken remaining time only every 15 seconds", () => {
    const say = (s: number) => controlAnnouncement("human_control", { isMine: true, inFlight: 0, remainingSeconds: s });
    expect(say(60)).toBe("あなたが操作中 — 残り約 60 秒");
    expect(say(59)).toBe(say(46));
    expect(say(45)).toBe("あなたが操作中 — 残り約 45 秒");
    expect(say(14)).toBe("あなたが操作中 — 残り約 15 秒");
  });
});

describe("liveViewState", () => {
  const owner = { available: true, isOwner: true };
  it("uses only the same-origin live path", () => {
    expect(
      liveViewState({ run: { state: "RUNNING", live_path: "/browser/live/T1/R1" }, owner, authInterval: false }),
    ).toEqual({ kind: "frame", href: "/browser/live/T1/R1" });
    expect(
      liveViewState({ run: { state: "RUNNING", live_path: "https://evil.example/x" }, owner, authInterval: false }),
    ).toEqual({ kind: "unavailable", reason: "not_configured" });
  });
  it("gives the reason in order: owner, running, auth interval", () => {
    const run = { state: "RUNNING" as const, live_path: "/browser/live/T1/R1" };
    expect(liveViewState({ run, owner: { available: false, isOwner: false }, authInterval: false })).toMatchObject({
      reason: "owner_unavailable",
    });
    expect(liveViewState({ run, owner: { available: true, isOwner: false }, authInterval: false })).toMatchObject({
      reason: "not_owner",
    });
    expect(liveViewState({ run: { ...run, state: "COMPLETED" }, owner, authInterval: false })).toMatchObject({
      reason: "not_running",
    });
    expect(liveViewState({ run, owner, authInterval: true })).toMatchObject({ reason: "auth_interval" });
  });
});

describe("live events", () => {
  it("keeps only this run's browser_updated rows and resumes after the last seen seq", () => {
    const row = (seq: number, run_id: string) => ({
      id: seq,
      seq,
      task_id: "T1",
      ts: "2026-10-05T00:00:00Z",
      event: { type: "browser_updated", browser: { run_id, state: "RUNNING", live_view_url: null } },
    });
    const items = liveEventItems({ items: [row(1, "R1"), row(2, "R0"), row(3, "R1")] as never }, "T1", "R1");
    expect(items.map((i) => i.seq)).toEqual([1, 3]);
    expect(items[0]?.text).toBe("状態: 実行中");
    expect(mergeLiveEvents(items, [...items, { seq: 4, ts: "", text: "x" }]).map((i) => i.seq)).toEqual([1, 3, 4]);
  });
});
