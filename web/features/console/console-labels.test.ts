import { describe, expect, it } from "vitest";
import type { ConsoleBlock } from "../../api/generated/types";
import { blockKindLabel, blocksVersion, progressPreview, splitFences, stepLine } from "./console-labels";

const reply = (text: string, state: "streaming" | "done" = "done"): ConsoleBlock => ({
  kind: "reply",
  at: "2026-01-01T00:00:00Z",
  cursor: "r1",
  message_id: "m",
  node_id: "cos",
  text,
  state,
});
const line = (seq: number, text: string) => ({ seq, at: "2026-01-01T00:00:00Z", text, kind: "text" as const });

describe("console labels", () => {
  it("種類の label は文字で区別し、生成中・回答済みも文字に出す", () => {
    expect(blockKindLabel(reply("a"))).toEqual({ label: "返事", tone: "info" });
    expect(blockKindLabel(reply("a", "streaming")).label).toBe("返事（生成中）");
    const q: ConsoleBlock = {
      kind: "question",
      at: "",
      cursor: "q",
      answered: true,
      run_id: "R",
      task_id: "T",
      text: "?",
    };
    expect(blockKindLabel(q).label).toBe("質問（回答済み）");
  });

  it("手順の行は tool 名と失敗を文字で出す", () => {
    expect(stepLine({ kind: "tool_use", tool: "Bash", text: "ls" })).toBe("tool Bash: ls");
    expect(stepLine({ kind: "tool_result", tool: "Bash", text: "x", error: true })).toBe("出力 Bash（失敗）: x");
    expect(stepLine({ kind: "status", text: "s" })).toBe("状態: s");
  });

  it("progress の先頭・末尾は重複を除き、間の省略件数を挟む", () => {
    expect(progressPreview({ count: 10, first: [line(1, "a")], last: [line(9, "b"), line(10, "c")] })).toEqual([
      "agent: a",
      "… 7 行省略 …",
      "agent: b",
      "agent: c",
    ]);
    expect(progressPreview({ count: 2, first: [line(1, "a"), line(2, "b")], last: [line(2, "b")] })).toEqual([
      "agent: a",
      "agent: b",
    ]);
  });

  it("``` の囲みを code に分け、閉じていない囲みは末尾まで code", () => {
    expect(splitFences("説明\n```sh\nls -la\n```\n後")).toEqual([
      { kind: "prose", text: "説明" },
      { kind: "code", text: "ls -la" },
      { kind: "prose", text: "後" },
    ]);
    expect(splitFences("途中\n```\nabc")).toEqual([
      { kind: "prose", text: "途中" },
      { kind: "code", text: "abc" },
    ]);
    expect(splitFences("囲みなし")).toEqual([{ kind: "prose", text: "囲みなし" }]);
  });

  it("追記の合図は末尾の本文が育つと変わる", () => {
    expect(blocksVersion([])).toBe("0");
    expect(blocksVersion([reply("a")])).not.toBe(blocksVersion([reply("ab")]));
  });
});
