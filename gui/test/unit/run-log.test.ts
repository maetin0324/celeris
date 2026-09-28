import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { countByKind, parseRunLog, type RunLogEvent, simpleDiff, summarizeToolInput } from "~/lib/run-log";

/**
 * `~/lib/run-log.ts`（docs/adr/0013-run-log-conversation-view.md D1）。`*-real.jsonl` は本番の run の
 * `stdout.jsonl` から行を選んで長い文字列を切り詰めたもの（2026-09-28。claude-code / codex / opencode ACP）。
 * `acp-synthetic.jsonl` は本番にまだ本文の出た ACP の run が無いので、ACP の仕様どおりに作ったもの。
 */
const dir = join(import.meta.dirname, "../fixtures");
function lines(name: string): string[] {
  return readFileSync(join(dir, name), "utf-8").split("\n");
}
function nonEmpty(name: string): string[] {
  return lines(name).filter((l) => l.trim().length > 0);
}
/** 変換で行が 1 つも失われない（各行がちょうど 1 つのイベントの raw に入る）。 */
function expectLossless(name: string, events: RunLogEvent[]) {
  const all = events.flatMap((e) => e.raw);
  expect([...all].sort()).toEqual([...nonEmpty(name)].sort());
}
function of<K extends RunLogEvent["kind"]>(events: RunLogEvent[], kind: K) {
  return events.filter((e): e is Extract<RunLogEvent, { kind: K }> => e.kind === kind);
}

describe("parseRunLog — claude-code（本番の実ログ）", () => {
  const name = "run-log/claude-code-real.jsonl";
  const events = parseRunLog(lines(name));

  it("元の行を全部どれかのイベントに持つ", () => expectLossless(name, events));

  it("assistant の本文・思考・ツール・使用量・system を見分ける", () => {
    const kinds = countByKind(events);
    expect(kinds.message).toBe(2);
    expect(kinds.thinking).toBe(2);
    expect(kinds.tool).toBe(5);
    expect(kinds.usage).toBe(1);
    expect(kinds.unknown).toBeUndefined();
  });

  it("tool_use と tool_result を 1 つにまとめ、Bash は command を要約にする", () => {
    const bash = of(events, "tool").find((t) => t.name === "Bash");
    expect(bash?.summary?.startsWith("cd /var/lib/celeris/workspaces/")).toBe(true);
    expect(bash?.pending).toBe(false);
    expect(bash?.raw).toHaveLength(2);
    expect(typeof bash?.result).toBe("string");
  });

  it("Edit は差分（structuredPatch / old_string→new_string）を持つ", () => {
    const edit = of(events, "tool").find((t) => t.name === "Edit");
    expect(edit?.diffs?.[0].path).toMatch(/gui\/app\/celeris\/types\.ts$/);
    const diffLines = edit?.diffs?.[0].hunks.flatMap((h) => h.lines) ?? [];
    expect(diffLines.some((l) => l.startsWith("+"))).toBe(true);
  });

  it("Bash の bashEditDiff をファイル差分として出す", () => {
    const withDiff = of(events, "tool").filter((t) => t.name === "Bash" && (t.diffs?.length ?? 0) > 0);
    expect(withDiff).toHaveLength(1);
    expect(withDiff[0].diffs?.[0].hunks[0].header).toMatch(/^@@ -\d+,\d+ \+\d+,\d+ @@$/);
  });

  it("is_error の tool_result はエラーとして出す", () => {
    const failed = of(events, "tool").filter((t) => t.isError);
    expect(failed).toHaveLength(1);
    expect(failed[0].name).toBe("Read");
    expect(failed[0].result).toContain("InputValidationError");
  });

  it("result は費用・所要時間・トークンの使用量になる", () => {
    const [usage] = of(events, "usage");
    const labels = usage.stats.map((s) => s.label);
    expect(labels).toEqual(expect.arrayContaining(["費用", "所要時間", "turn", "出力 tokens"]));
    expect(usage.stats.find((s) => s.label === "費用")?.value).toBe("$2.2509");
    expect(usage.isError).toBe(false);
    expect(usage.text).toBeTruthy();
  });

  it("続く同じ system（thinking_tokens・heartbeat）は 1 行にまとめて件数を出す", () => {
    const thinking = of(events, "system").find((s) => s.label === "思考中");
    expect(thinking?.count).toBe(2);
    expect(thinking?.raw).toHaveLength(2);
  });
});

describe("parseRunLog — codex（本番の実ログ）", () => {
  const name = "run-log/codex-real.jsonl";
  const events = parseRunLog(lines(name));

  it("元の行を全部どれかのイベントに持つ", () => expectLossless(name, events));

  it("item.started と item.completed を同じコマンドにまとめ、終了コードと出力を持つ", () => {
    const cmds = of(events, "command");
    expect(cmds).toHaveLength(6);
    const first = cmds[0];
    expect(first.raw).toHaveLength(2);
    expect(first.status).toBe("completed");
    expect(first.exitCode).toBe(0);
    expect(first.output).toContain("total");
  });

  it("agent_message は assistant の発言、file_change はファイル変更、turn.completed は使用量", () => {
    expect(of(events, "message")).toHaveLength(3);
    const [fc] = of(events, "file_change");
    expect(fc.diffs.map((d) => d.kind)).toEqual(["add", "add"]);
    expect(fc.status).toBe("completed");
    const [usage] = of(events, "usage");
    expect(usage.stats.find((s) => s.label === "出力 tokens")?.value).toBe("4,613");
  });
});

describe("parseRunLog — opencode ACP", () => {
  it("本番の実ログ: 初期化・セッション・JSON-RPC エラーを見分ける", () => {
    const name = "run-log/opencode-acp-real.jsonl";
    const events = parseRunLog(lines(name));
    expectLossless(name, events);
    expect(of(events, "system").map((s) => s.label)).toContain("ACP 初期化");
    const [err] = of(events, "error");
    expect(err.text).toContain("Cannot connect to API");
  });

  it("chunk を連結し、tool_call と tool_call_update を 1 つにまとめて差分を出す", () => {
    const name = "run-log/acp-synthetic.jsonl";
    const events = parseRunLog(lines(name));
    expectLossless(name, events);
    expect(of(events, "thinking")[0].text).toBe("まず構成を確かめる。");
    expect(of(events, "message")[0].text).toBe("README を直します。");
    const [tool] = of(events, "tool");
    expect(tool.pending).toBe(false);
    expect(tool.summary).toBe("Edit README.md");
    expect(tool.diffs?.[0].hunks[0].lines).toEqual(["-# old", "+# new", " body"]);
    // 知らない sessionUpdate は捨てずに unknown。
    expect(of(events, "unknown").map((u) => u.label)).toEqual(["ACP: brand_new_update"]);
    expect(of(events, "usage")[0].stats[0]).toEqual({ label: "stopReason", value: "end_turn" });
  });
});

describe("parseRunLog — 旧 fixture と知らない形式", () => {
  it("claude-code の最小ログ", () => {
    const events = parseRunLog(lines("stream-json/claude-code.jsonl"));
    expect(events.map((e) => e.kind)).toEqual(["message", "tool", "usage"]);
    expect(of(events, "tool")[0]).toMatchObject({ name: "Bash", pending: true });
  });

  it("codex の最小ログ（id の無い item と turn.failed）", () => {
    const events = parseRunLog(lines("stream-json/codex.jsonl"));
    expect(events.map((e) => e.kind)).toEqual(["system", "command", "usage", "error"]);
    expect(of(events, "error")[0].text).toBe("sandbox denied write");
  });

  it("celeris 独自プロトコル・JSON でない行・配列は unknown として残す", () => {
    const name = "run-log/mixed-unknown.jsonl";
    const events = parseRunLog(lines(name));
    expectLossless(name, events);
    expect(events.map((e) => e.kind)).toEqual(["unknown", "unknown", "unknown", "message", "unknown"]);
    expect(of(events, "unknown").map((u) => u.label)).toEqual(["progress", "テキスト", "JSON", "done"]);
  });

  it("追記の途中（最後の行が書きかけ）は unknown になり、続きが来たら正しく変換される", () => {
    const full = nonEmpty("run-log/codex-real.jsonl");
    const partial = [...full.slice(0, 3), full[3].slice(0, 40)];
    expect(parseRunLog(partial).at(-1)?.kind).toBe("unknown");
    expect(parseRunLog(full.slice(0, 4)).at(-1)?.kind).toBe("command");
  });
});

describe("補助", () => {
  it("simpleDiff は共通の前後を文脈にして変わった行だけ -/+ にする", () => {
    expect(simpleDiff("a\nb\nc", "a\nB\nc").lines).toEqual([" a", "-b", "+B", " c"]);
    expect(simpleDiff("", "x\ny").lines).toEqual(["+x", "+y"]);
  });

  it("summarizeToolInput は主な引数を 1 行にする", () => {
    expect(summarizeToolInput("Read", { file_path: "/a/b.rs", limit: 10 })).toBe("/a/b.rs");
    expect(summarizeToolInput("Grep", { pattern: "foo", path: "src" })).toBe("foo (src)");
    expect(summarizeToolInput("Bash", { command: "cargo\n  test" })).toBe("cargo test");
    expect(summarizeToolInput("X", { n: 1 })).toBeUndefined();
  });
});
