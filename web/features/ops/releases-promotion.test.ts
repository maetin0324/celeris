import { describe, expect, it } from "vitest";
import type { ReleaseItem } from "../../api/generated/types";
import { canPromote, judgePromotion, pollDelayMs } from "./releases-promotion";

const item = (patch: Partial<ReleaseItem> = {}): ReleaseItem => ({
  sha12: "aaaaaaaaaaaa",
  gate_ok: true,
  is_current: false,
  is_previous: false,
  promoting: false,
  promoted_at: null,
  ...patch,
});
const track = { sha12: "aaaaaaaaaaaa", baselinePromotedAt: null, startedAt: "2026-09-30T00:00:10Z" };

describe("releases", () => {
  it("202 直後（promoting も結果も無い）は成功と出さない", () => {
    expect(judgePromotion(item(), track).state).toBe("pending");
    expect(judgePromotion(undefined, track).state).toBe("pending");
    expect(judgePromotion(item({ promoting: true }), track).state).toBe("pending");
  });
  it("promoted_at が変わって current なら成功", () => {
    expect(judgePromotion(item({ is_current: true, promoted_at: "2026-09-30T00:00:20Z" }), track).state).toBe(
      "succeeded",
    );
    expect(
      judgePromotion(item({ is_current: true, promoted_at: "x" }), { ...track, baselinePromotedAt: "x" }).state,
    ).toBe("pending");
  });
  it("開始後の失敗記録は失敗、開始前の古い記録は無視", () => {
    const failed = (at: string) => item({ promote_failed: { failed_at: at, error: "boom" } });
    expect(judgePromotion(failed("2026-09-30T00:00:11Z"), track)).toEqual({ state: "failed", error: "boom" });
    expect(judgePromotion(failed("2026-09-29T00:00:00Z"), track).state).toBe("pending");
  });
  it("待ち時間は失敗が続くと延びて頭打ち", () => {
    expect(pollDelayMs(0)).toBe(1000);
    expect(pollDelayMs(2)).toBe(4000);
    expect(pollDelayMs(9)).toBe(5000);
  });
  it("昇格できるのは gate を通った current でないもの", () => {
    expect(canPromote(item())).toBe(true);
    expect(canPromote(item({ gate_ok: false }))).toBe(false);
    expect(canPromote(item({ is_current: true }))).toBe(false);
    expect(canPromote(item({ promoting: true }))).toBe(false);
  });
});
