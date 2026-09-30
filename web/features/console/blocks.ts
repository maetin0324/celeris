import type { ConsoleBlock } from "../../api/generated/types";

// Console の block の並びと重複排除（P3-01）。gui/app/lib/console.ts の appendConsoleBlock と同じ規則。
// - progress は run_id ごと、reply は (task_id, run_id) ごとに 1 つ。同じ cursor は 1 度だけ。
// - 育つ返事（streaming）の増分は積み上げ、確定（done）は置き換える。

export const MAX_BLOCKS = 300;

export function appendBlock(items: readonly ConsoleBlock[], incoming: ConsoleBlock): ConsoleBlock[] {
  if (incoming.kind === "progress") {
    const idx = items.findIndex((b) => b.kind === "progress" && b.progress.run_id === incoming.progress.run_id);
    if (idx >= 0) return replaceAt(items, idx, incoming);
  } else if (incoming.kind === "reply" && incoming.run_id) {
    const idx = items.findIndex(
      (b) => b.kind === "reply" && b.run_id === incoming.run_id && b.task_id === incoming.task_id,
    );
    if (idx >= 0) {
      const existing = items[idx];
      if (existing?.kind === "reply" && existing.cursor === incoming.cursor) return items.slice();
      if (incoming.state === "streaming" && existing?.kind === "reply") {
        return replaceAt(items, idx, {
          ...incoming,
          text: existing.text + incoming.text,
          thinking: incoming.thinking ?? existing.thinking,
          steps: [...(existing.steps ?? []), ...(incoming.steps ?? [])],
        });
      }
      return replaceAt(items, idx, incoming);
    }
  } else if (items.some((b) => b.cursor === incoming.cursor)) {
    return items.slice();
  }
  const next = [...items, incoming];
  return next.length > MAX_BLOCKS ? next.slice(next.length - MAX_BLOCKS) : next;
}

function replaceAt(items: readonly ConsoleBlock[], idx: number, block: ConsoleBlock): ConsoleBlock[] {
  const next = items.slice();
  next[idx] = block;
  return next;
}

export function appendBlocks(items: readonly ConsoleBlock[], incoming: readonly ConsoleBlock[]): ConsoleBlock[] {
  return incoming.reduce<ConsoleBlock[]>((acc, block) => appendBlock(acc, block), items.slice());
}
