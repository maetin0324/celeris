import type { BadgeTone } from "../../components/ui/badge";

// 差分の 1 行の種類。色は token の組（success / danger / info）で付け、行頭の +/− を原文のまま残して色だけに頼らない。
export type DiffLineKind = "add" | "remove" | "hunk" | "meta" | "context";

export function diffLineKind(line: string): DiffLineKind {
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("diff ") || line.startsWith("index ") || line.startsWith("\\ ")) return "meta";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "remove";
  return "context";
}

/** 原文を行に分ける。末尾の改行で空行を 1 つ足さない。 */
export function diffLines(diff: string): { kind: DiffLineKind; text: string }[] {
  const lines = diff.split("\n");
  if (lines.at(-1) === "") lines.pop();
  return lines.map((text) => ({ kind: diffLineKind(text), text }));
}

// git の status 1 文字（M/A/D/R/C/?）を可視ラベルと tone へ。未知の文字は原文のまま neutral。
const fileStatus: Record<string, { label: string; tone: BadgeTone }> = {
  M: { label: "変更", tone: "info" },
  A: { label: "追加", tone: "success" },
  D: { label: "削除", tone: "danger" },
  R: { label: "名前変更", tone: "neutral" },
  C: { label: "複製", tone: "neutral" },
  "?": { label: "未追跡", tone: "warning" },
};

export function fileStatusView(status: string): { label: string; tone: BadgeTone } {
  return fileStatus[status.trim().charAt(0).toUpperCase()] ?? { label: status, tone: "neutral" };
}

// 取り込みの状態語を tone へ。状態語そのものは可視ラベルとして出す。
export function integrationTone(state: string): BadgeTone {
  if (state === "merged" || state === "done") return "success";
  if (state === "conflict" || state === "failed" || state === "error") return "danger";
  if (state === "open" || state === "pending") return "info";
  return "neutral";
}
