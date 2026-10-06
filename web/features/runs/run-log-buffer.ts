// run ログの追記 buffer（P3-12、R26）。`offset` はバイト数、未完の最後の行は次の取得まで持つ。
// 保持する行には上限があり、超えた分は古い行から捨てる（捨てた行数は dropped に数える）。

export const MAX_LINES = 5000;
export const POLL_INTERVAL_MS = 1000;
export const MAX_POLL_ATTEMPTS = 900;

export type RunLogBuffer = {
  lines: string[];
  /** 次の要求の `offset`（受け取ったバイト数）。 */
  offset: number;
  /** 改行で終わっていない最後の断片。 */
  partial: string;
  dropped: number;
};

export const emptyBuffer: RunLogBuffer = { lines: [], offset: 0, partial: "", dropped: 0 };

export function appendChunk(
  buffer: RunLogBuffer,
  text: string,
  byteLength: number,
  maxLines: number = MAX_LINES,
): RunLogBuffer {
  if (byteLength === 0) return buffer;
  const parts = (buffer.partial + text).split("\n");
  const partial = parts.pop() ?? "";
  const added = parts.filter((line) => line.trim() !== "");
  let lines = added.length ? [...buffer.lines, ...added] : buffer.lines;
  let dropped = buffer.dropped;
  if (lines.length > maxLines) {
    dropped += lines.length - maxLines;
    lines = lines.slice(lines.length - maxLines);
  }
  return { lines, offset: buffer.offset + byteLength, partial, dropped };
}

/** 終わった run の最後の断片（改行の無い最終行）を行として確定する。 */
export function flushPartial(buffer: RunLogBuffer): RunLogBuffer {
  if (buffer.partial.trim() === "") return buffer;
  return { ...buffer, lines: [...buffer.lines, buffer.partial], partial: "" };
}

export function runFilePath(taskId: string, runId: string, offset: number): string {
  return `/files/tasks/${encodeURIComponent(taskId)}/runs/${encodeURIComponent(runId)}/stdout?offset=${offset}`;
}
