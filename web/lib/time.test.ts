import { afterEach, describe, expect, it } from "vitest";
import { formatAbsolute, formatRelative, recordHello, serverNowMs, setServerClockOffset } from "./time";

afterEach(() => setServerClockOffset(0));

describe("time", () => {
  it("formats absolute time in the given time zone", () => {
    const iso = "2026-09-30T00:00:00Z";
    expect(formatAbsolute(iso, "Asia/Tokyo")).toContain("9:00:00");
    expect(formatAbsolute(iso, "America/New_York")).toContain("20:00:00");
    expect(formatAbsolute("bad", "Asia/Tokyo")).toBe("");
  });

  it("relative time is corrected by server clock skew", () => {
    const local = Date.parse("2026-09-30T00:00:00Z");
    recordHello("2026-09-30T01:00:00Z", local); // server は 1 時間進んでいる
    expect(serverNowMs(local)).toBe(local + 3_600_000);
    const eventAt = "2026-09-30T00:59:00Z"; // server 時計で 1 分前
    expect(formatRelative(eventAt, serverNowMs(local))).toBe("1 分前");
    expect(formatRelative(eventAt, local)).toBe("59 分後"); // 補正しないと未来になる
  });

  it("ignores an invalid hello time", () => {
    recordHello("x", 0);
    expect(serverNowMs(5)).toBe(5);
  });
});
