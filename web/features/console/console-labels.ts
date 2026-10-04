import type { ConsoleBlock, ConsoleProgressLine, ConsoleReplyStep } from "../../api/generated/types";
import type { BadgeTone } from "../../components/ui/badge";

// Console の各 block の種類の文字 label と、progress・手順の行の文（純粋関数。表示部品は console-view.tsx）。
// 色だけで区別しないため、どの block にもこの文字を出す（run ログ画面の eventKindLabel と同じ考え方）。

export function blockKindLabel(block: ConsoleBlock): { label: string; tone: BadgeTone } {
  switch (block.kind) {
    case "human":
      return { label: "あなた", tone: "neutral" };
    case "reply":
      return block.state === "streaming"
        ? { label: "返事（生成中）", tone: "running" }
        : { label: "返事", tone: "info" };
    case "progress":
      return { label: "run の作業", tone: "running" };
    case "task":
      return { label: "タスク", tone: "neutral" };
    case "question":
      return block.answered ? { label: "質問（回答済み）", tone: "neutral" } : { label: "質問", tone: "warning" };
    case "approval":
      return { label: "承認待ち", tone: "warning" };
    case "milestone":
      return { label: "途中目標", tone: "neutral" };
    case "report":
      return { label: "報告", tone: "success" };
    case "knowledge":
      return { label: "知識", tone: "neutral" };
  }
}

/** progress・返事の手順の 1 行。tool は名前を前に付け、失敗は文字でも示す。 */
export function stepLine(
  step: Pick<ConsoleProgressLine | ConsoleReplyStep, "kind" | "tool" | "text" | "error">,
): string {
  const head =
    step.kind === "tool_use"
      ? `tool ${step.tool ?? "?"}`
      : step.kind === "tool_result"
        ? `出力${step.tool ? ` ${step.tool}` : ""}`
        : step.kind === "thinking"
          ? "思考"
          : step.kind === "text"
            ? "agent"
            : "状態";
  return `${head}${step.error ? "（失敗）" : ""}: ${step.text}`;
}

/** progress の先頭・末尾の行。間を省いた件数があれば「… N 行省略 …」を挟む。 */
export function progressPreview(progress: {
  count: number;
  first: readonly ConsoleProgressLine[];
  last: readonly ConsoleProgressLine[];
}): string[] {
  const seen = new Set<number>();
  const first = progress.first.filter((l) => !seen.has(l.seq) && seen.add(l.seq));
  const last = progress.last.filter((l) => !seen.has(l.seq) && seen.add(l.seq));
  const omitted = progress.count - first.length - last.length;
  return [
    ...first.map(stepLine),
    ...(omitted > 0 && last.length > 0 ? [`… ${omitted} 行省略 …`] : []),
    ...last.map(stepLine),
  ];
}

/** 追記の合図。件数・末尾の cursor・末尾の本文長が変われば変わる（育つ返事の増分も拾う）。 */
export function blocksVersion(blocks: readonly ConsoleBlock[]): string {
  const tail = blocks.at(-1);
  if (!tail) return "0";
  const size =
    tail.kind === "reply"
      ? tail.text.length + (tail.steps?.length ?? 0)
      : tail.kind === "human"
        ? tail.text.length
        : tail.kind === "progress"
          ? tail.progress.count
          : 0;
  return `${blocks.length}:${tail.cursor}:${size}`;
}

export type TextPart = { kind: "prose" | "code"; text: string };

/** 本文を ``` の囲みで地の文と code に分ける。閉じていない囲みは末尾まで code（育つ返事の途中）。 */
export function splitFences(text: string): TextPart[] {
  const parts: TextPart[] = [];
  const lines = text.split("\n");
  let buf: string[] = [];
  let code = false;
  const flush = () => {
    const joined = buf.join("\n");
    if (code || joined.trim() !== "")
      parts.push({ kind: code ? "code" : "prose", text: code ? joined : joined.trim() });
    buf = [];
  };
  for (const line of lines) {
    if (line.trimStart().startsWith("```")) {
      flush();
      code = !code;
    } else buf.push(line);
  }
  flush();
  return parts;
}
