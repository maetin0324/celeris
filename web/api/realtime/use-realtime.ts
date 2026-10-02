// shell から呼ぶ hook。認証済み shell で 1 本だけ購読する（task 詳細では呼ばない）。

import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { createRealtime, type RealtimeOptions } from "./realtime";

export { useConnectionState } from "./connection-state";

export function useRealtimeSubscription(options: Omit<RealtimeOptions, "queryClient"> = {}): void {
  const queryClient = useQueryClient();
  // biome-ignore lint/correctness/useExhaustiveDependencies: 購読は shell の寿命で 1 回だけ作る
  useEffect(() => {
    const realtime = createRealtime({ queryClient, ...options });
    realtime.start();
    return () => realtime.dispose();
  }, [queryClient]);
}
