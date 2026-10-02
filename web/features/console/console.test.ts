import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import type { ConsoleBlock } from "../../api/generated/types";
import { appendBlock, appendBlocks } from "./blocks";
import { applyStreamBlock, buildInstructBody, type ConsoleCache, consoleQueryKey, mergePage } from "./cache";
import { consoleStreamUrl, type EventSourceLike, openConsoleStream } from "./stream";

const human = (cursor: string): ConsoleBlock => ({
  kind: "human",
  at: "2026-01-01T00:00:00Z",
  cursor,
  message_id: cursor,
  node_id: "cos",
  text: cursor,
});
const reply = (cursor: string, text: string, state: "streaming" | "done"): ConsoleBlock => ({
  kind: "reply",
  at: "2026-01-01T00:00:00Z",
  cursor,
  message_id: "m",
  node_id: "cos",
  run_id: "R1",
  task_id: "T1",
  text,
  state,
});

describe("blocks", () => {
  it("keeps order and drops duplicate cursors", () => {
    const out = appendBlocks([], [human("1"), human("2"), human("1"), human("3")]);
    expect(out.map((b) => b.cursor)).toEqual(["1", "2", "3"]);
  });
  it("accumulates streaming replies and replaces with done", () => {
    let items = appendBlock([], reply("1", "he", "streaming"));
    items = appendBlock(items, reply("2", "llo", "streaming"));
    items = appendBlock(items, reply("2", "llo", "streaming"));
    expect(items).toHaveLength(1);
    expect((items[0] as { text: string }).text).toBe("hello");
    items = appendBlock(items, reply("3", "hello!", "done"));
    expect((items[0] as { text: string }).text).toBe("hello!");
  });
});

describe("cache", () => {
  it("merges REST page and stream deltas into one key without duplicates", () => {
    const client = new QueryClient();
    const key = consoleQueryKey("all", "0");
    applyStreamBlock(client, key, human("2"));
    client.setQueryData<ConsoleCache>(
      key,
      mergePage(client.getQueryData(key), { items: [human("1"), human("2")], next_cursor: "2" }),
    );
    const data = client.getQueryData<ConsoleCache>(key);
    expect(data?.blocks.map((b) => b.cursor)).toEqual(["1", "2"]);
    expect(data?.cursor).toBe("2");
    expect(key).toEqual(["console", "all", "0"]);
  });
  it("omits scope for @mention and all", () => {
    expect(buildInstructBody("@cos hi", "node:x")).toEqual({ text: "@cos hi" });
    expect(buildInstructBody("hi", "all")).toEqual({ text: "hi" });
    expect(buildInstructBody("hi", "node:x")).toEqual({ text: "hi", scope: "node:x" });
  });
});

describe("stream", () => {
  it("resumes from the last cursor after an error", () => {
    vi.useFakeTimers();
    const sources: (EventSourceLike & { urls: string; fire: (t: string, d: unknown) => void })[] = [];
    const seen: string[] = [];
    const stream = openConsoleStream({
      scope: "node:cos",
      retryMs: 10,
      onBlock: (b) => seen.push(b.cursor),
      createSource: (url) => {
        const l = new Map<string, (e: MessageEvent<string>) => void>();
        const s = {
          urls: url,
          onerror: null as ((e: Event) => void) | null,
          onopen: null,
          addEventListener: (t: string, f: (e: MessageEvent<string>) => void) => void l.set(t, f),
          close: () => {},
          fire: (t: string, d: unknown) =>
            l.get(t)?.({ data: typeof d === "string" ? d : JSON.stringify(d) } as MessageEvent<string>),
        };
        sources.push(s);
        return s;
      },
    });
    expect(sources[0]?.urls).toBe(consoleStreamUrl("node:cos", null));
    sources[0]?.fire("console.block", human("c5"));
    sources[0]?.fire("console.block", "{broken");
    sources[0]?.onerror?.(new Event("error"));
    vi.advanceTimersByTime(20);
    expect(sources[1]?.urls).toBe("/console/stream?scope=node%3Acos&since=c5");
    expect(seen).toEqual(["c5"]);
    stream.close();
    vi.useRealTimers();
  });
});
