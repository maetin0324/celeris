// 接続状態の store と React hook。切断中は更新停止を表示するために使う（ADR-0081 D6）。

import { useSyncExternalStore } from "react";

export type ConnectionState = "connecting" | "open" | "reconnecting" | "unauthorized" | "closed";

export type ConnectionStore = {
  get(): ConnectionState;
  set(next: ConnectionState): void;
  subscribe(listener: () => void): () => void;
};

export function createConnectionStore(initial: ConnectionState = "closed"): ConnectionStore {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    get: () => state,
    set(next) {
      if (next === state) return;
      state = next;
      for (const l of [...listeners]) l();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** shell が 1 つだけ持つ既定の store。 */
export const connectionStore = createConnectionStore();

export function useConnectionState(store: ConnectionStore = connectionStore): ConnectionState {
  return useSyncExternalStore(store.subscribe, store.get, store.get);
}
