// key ごとに 250 ms 束ね、key ごとに進行中の取得を 1 本に保つ invalidate 実行器（ADR-0081 D5・D6）。
// - 窓の間に来た同じ key は 1 回にまとめる。
// - 取得の最中に来た同じ key は cancel/restart せず、完了後に 1 回だけ取り直す（要求を積み上げない）。
// - reset は全 server query（['daemon','stream'] を除く）を stale にし、active だけ取り直す。

import { hashKey, type QueryClient, type QueryKey } from "@tanstack/react-query";
import type { EventRow } from "../generated/types";
import { daemonKeys } from "../queries/keys";
import { dedupeKeys, keysForTaskEvent, resolveProjectId } from "./invalidation-map";

export const COALESCE_MS = 250;
const SEEN_LIMIT = 4_096;
const RESET_SLOT = "\u0000reset";

type Slot = { timer?: ReturnType<typeof setTimeout>; inflight: boolean; dirty: boolean };

export type Invalidator = {
  onTaskEvent(row: EventRow): void;
  onReset(): void;
  /** 復帰: stale かつ active な query だけ取り直す。 */
  onResume(): void;
  enqueue(keys: readonly QueryKey[]): void;
  dispose(): void;
};

export function createInvalidator(queryClient: QueryClient, windowMs = COALESCE_MS): Invalidator {
  const slots = new Map<string, Slot>();
  const seenEvents = new Set<string>();
  let disposed = false;

  function slotFor(id: string): Slot {
    let slot = slots.get(id);
    if (!slot) {
      slot = { inflight: false, dirty: false };
      slots.set(id, slot);
    }
    return slot;
  }

  function schedule(id: string, run: () => Promise<unknown>) {
    if (disposed) return;
    const slot = slotFor(id);
    if (slot.timer !== undefined) return; // 窓の中の重複
    slot.timer = setTimeout(() => {
      slot.timer = undefined;
      fire(id, run);
    }, windowMs);
  }

  function fire(id: string, run: () => Promise<unknown>) {
    const slot = slotFor(id);
    if (slot.inflight) {
      slot.dirty = true;
      return;
    }
    slot.inflight = true;
    void run()
      .catch(() => undefined)
      .finally(() => {
        slot.inflight = false;
        if (slot.dirty) {
          slot.dirty = false;
          schedule(id, run);
        } else if (slot.timer === undefined) {
          slots.delete(id);
        }
      });
  }

  function enqueueOne(queryKey: QueryKey) {
    const id = hashKey(queryKey);
    const slot = slots.get(id);
    if (slot?.inflight) {
      // 取得中: 進行中の取得は cancel せず、完了後に 1 回だけ。
      slot.dirty = true;
      return;
    }
    schedule(id, () => queryClient.invalidateQueries({ queryKey, refetchType: "active" }, { cancelRefetch: false }));
  }

  function remember(row: EventRow): boolean {
    const id = `${row.id}`;
    const pair = `${row.task_id}:${row.seq}`;
    if (seenEvents.has(id) || seenEvents.has(pair)) return false;
    seenEvents.add(id);
    seenEvents.add(pair);
    if (seenEvents.size > SEEN_LIMIT) {
      const first = seenEvents.values().next();
      if (!first.done) seenEvents.delete(first.value);
    }
    return true;
  }

  const invalidator: Invalidator = {
    enqueue(keys) {
      for (const key of dedupeKeys(keys)) enqueueOne(key);
    },
    onTaskEvent(row) {
      if (!remember(row)) return;
      const projectId = resolveProjectId(queryClient, row.task_id, row.event);
      for (const key of keysForTaskEvent({ taskId: row.task_id, event: row.event, projectId })) enqueueOne(key);
    },
    onReset() {
      // 束ね待ちの個別 invalidate は reset に包含される。
      for (const [id, slot] of slots) {
        if (id !== RESET_SLOT && slot.timer !== undefined) {
          clearTimeout(slot.timer);
          slot.timer = undefined;
        }
      }
      const streamId = hashKey(daemonKeys.stream());
      const slot = slotFor(RESET_SLOT);
      if (slot.inflight) {
        slot.dirty = true;
        return;
      }
      slot.inflight = true;
      void queryClient
        .invalidateQueries(
          { predicate: (q) => hashKey(q.queryKey) !== streamId, refetchType: "active" },
          { cancelRefetch: false },
        )
        .catch(() => undefined)
        .finally(() => {
          slot.inflight = false;
          if (slot.dirty) {
            slot.dirty = false;
            invalidator.onReset();
          }
        });
    },
    onResume() {
      void queryClient.refetchQueries({ type: "active", stale: true }, { cancelRefetch: false }).catch(() => undefined);
    },
    dispose() {
      disposed = true;
      for (const slot of slots.values()) if (slot.timer !== undefined) clearTimeout(slot.timer);
      slots.clear();
    },
  };
  return invalidator;
}
