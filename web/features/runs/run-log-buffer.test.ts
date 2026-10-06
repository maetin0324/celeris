import { describe, expect, it } from "vitest";
import { parseRunLog } from "./run-log";
import { appendChunk, emptyBuffer, flushPartial, runFilePath } from "./run-log-buffer";

describe("run-log buffer", () => {
  it("splits lines across chunks and tracks the byte offset", () => {
    let b = appendChunk(emptyBuffer, '{"a":1}\n{"b"', 12);
    expect(b.lines).toEqual(['{"a":1}']);
    expect(b.offset).toBe(12);
    b = appendChunk(b, ":2}\n", 4);
    expect(b.lines).toEqual(['{"a":1}', '{"b":2}']);
    expect(b.offset).toBe(16);
    expect(appendChunk(b, "", 0)).toBe(b);
  });

  it("caps the kept lines", () => {
    const b = appendChunk(emptyBuffer, "1\n2\n3\n4\n", 8, 2);
    expect(b.lines).toEqual(["3", "4"]);
    expect(b.dropped).toBe(2);
  });

  it("flushes the last line without newline", () => {
    expect(flushPartial(appendChunk(emptyBuffer, "x\ny", 3)).lines).toEqual(["x", "y"]);
  });

  it("builds the relay path", () => {
    expect(runFilePath("T 1", "R1", 5)).toBe("/files/tasks/T%201/runs/R1/stdout?offset=5");
  });

  it("parses claude-code stream-json", () => {
    const events = parseRunLog([
      JSON.stringify({ type: "assistant", message: { content: [{ type: "text", text: "こんにちは" }] } }),
      "not json",
    ]);
    expect(events.some((e) => e.kind === "message")).toBe(true);
    expect(events.at(-1)?.kind).toBe("unknown");
  });
});
