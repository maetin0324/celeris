import { describe, expect, it } from "vitest";
import { eventKindLabel, formatSeconds, isNearBottom, runDuration, runHarness, runStatus } from "./run-header";
import { parseRunLog } from "./run-log";

describe("run header", () => {
  it("formats the duration of finished and running runs", () => {
    expect(formatSeconds(5)).toBe("5 秒");
    expect(formatSeconds(185)).toBe("3 分 5 秒");
    expect(formatSeconds(3720)).toBe("1 時間 2 分");
    expect(formatSeconds(-1)).toBeUndefined();
    const started_at = "2026-01-01T00:00:00Z";
    expect(runDuration({ started_at, finished_at: "2026-01-01T00:01:30Z" }, 0)).toBe("1 分 30 秒");
    expect(runDuration({ started_at, finished_at: null }, Date.parse(started_at) + 10_000)).toBe("10 秒（経過）");
    expect(runDuration({ started_at: "bad", finished_at: null }, 0)).toBeUndefined();
  });

  it("derives the status word and harness from the API values only", () => {
    expect(runStatus({ finished_at: null, outcome: null })).toBe("running");
    expect(runStatus({ finished_at: "2026-01-01T00:00:00Z", outcome: "done" })).toBe("done");
    expect(runStatus({ finished_at: "2026-01-01T00:00:00Z", outcome: null })).toBeUndefined();
    expect(runHarness({ adapter: "claude-code", model: "opus" })).toBe("claude-code（opus）");
    expect(runHarness({ adapter: "codex", model: "" })).toBe("codex");
  });

  it("labels every event kind with text, not only color", () => {
    const events = parseRunLog([
      JSON.stringify({ type: "assistant", message: { role: "assistant", content: [{ type: "text", text: "hi" }] } }),
      "not json",
    ]);
    expect(events.map(eventKindLabel)).toEqual(["agent", "未分類"]);
  });

  it("treats a position within the threshold as the bottom", () => {
    expect(isNearBottom({ scrollTop: 480, scrollHeight: 1000, clientHeight: 500 })).toBe(true);
    expect(isNearBottom({ scrollTop: 100, scrollHeight: 1000, clientHeight: 500 })).toBe(false);
  });
});
