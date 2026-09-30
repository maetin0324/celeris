// shell が 1 度だけ呼ぶ購読の組み立て。transport・resume・frame の反映をつなぐ。

import type { QueryClient } from "@tanstack/react-query";
import { recordHello } from "../../lib/time";
import { daemonKeys } from "../queries/keys";
import type { ConnectionStore } from "./connection-state";
import { connectionStore } from "./connection-state";
import type { Frame } from "./frames";
import { createInvalidator, type Invalidator } from "./invalidator";
import { bindResume } from "./resume";
import { createTransport, type Transport, type TransportOptions } from "./transport";

export type RealtimeOptions = {
  queryClient: QueryClient;
  store?: ConnectionStore;
  /** task.event と reset の反映（invalidate）。P2-05 の invalidation が渡される。 */
  onTaskEvent?: (frame: Extract<Frame, { type: "task.event" }>) => void;
  onReset?: (frame: Extract<Frame, { type: "reset" }>) => void;
  onResumed?: () => void;
} & Pick<TransportOptions, "createEventSource" | "probe" | "url" | "backoffBaseMs" | "backoffMaxMs">;

/**
 * hello / heartbeat は server query を invalidate しない。daemon（と hello.daemon）は ['daemon','stream'] だけを更新し、
 * ['daemon','rest'] には触れない。
 */
export function applyFrame(
  queryClient: QueryClient,
  frame: Frame,
  options: Pick<RealtimeOptions, "onTaskEvent" | "onReset">,
) {
  switch (frame.type) {
    case "hello":
      recordHello(frame.data.now); // 相対時刻の補正（P2-06）
      if (frame.data.daemon) queryClient.setQueryData(daemonKeys.stream(), frame.data.daemon);
      return;
    case "heartbeat":
      return;
    case "daemon":
      queryClient.setQueryData(daemonKeys.stream(), frame.data);
      return;
    case "task.event":
      options.onTaskEvent?.(frame);
      return;
    case "reset":
      options.onReset?.(frame);
      return;
  }
}

export type Realtime = Transport & { dispose(): void; invalidator: Invalidator };

export function createRealtime(options: RealtimeOptions): Realtime {
  const store = options.store ?? connectionStore;
  const invalidator = createInvalidator(options.queryClient);
  const onTaskEvent = options.onTaskEvent ?? ((f) => invalidator.onTaskEvent(f.data));
  const onReset = options.onReset ?? (() => invalidator.onReset());
  const transport = createTransport({
    store,
    url: options.url,
    createEventSource: options.createEventSource,
    probe: options.probe,
    backoffBaseMs: options.backoffBaseMs,
    backoffMaxMs: options.backoffMaxMs,
    onFrame: (frame) => applyFrame(options.queryClient, frame, { onTaskEvent, onReset }),
    onResume: () => {
      invalidator.onResume();
      options.onResumed?.();
    },
  });
  let unbind: (() => void) | undefined;
  if (typeof window !== "undefined" && typeof document !== "undefined") {
    unbind = bindResume({ onResume: () => transport.resume(), windowTarget: window, documentTarget: document });
  }
  return {
    ...transport,
    invalidator,
    dispose() {
      invalidator.dispose();
      unbind?.();
      transport.stop();
    },
  };
}
