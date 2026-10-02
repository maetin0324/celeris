import { useEffect, useRef, useState } from "react";
import {
  appendChunk,
  emptyBuffer,
  flushPartial,
  MAX_POLL_ATTEMPTS,
  POLL_INTERVAL_MS,
  type RunLogBuffer,
  runFilePath,
} from "./run-log-buffer";

// 中継が応答しないときに読み込みを止める上限（1 回の読み取りごと）。
const REQUEST_TIMEOUT_MS = 5000;

// stdout.jsonl を gateway の file relay（R40）から `offset` で読み足す（P3-12）。
// - running が true（または未確定の undefined）の間だけ、上限付きの polling で追記を追う。
// - 終わった run は 1 回読んだら取り直さない。
export type RunLogState = {
  buffer: RunLogBuffer;
  status: "loading" | "ready" | "error";
  following: boolean;
};

export function useRunLog(taskId: string, runId: string, running: boolean | undefined): RunLogState {
  const [state, setState] = useState<RunLogState>({ buffer: emptyBuffer, status: "loading", following: false });
  const runningRef = useRef(running);
  runningRef.current = running;

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();
    const decoder = new TextDecoder();
    let buffer = emptyBuffer;
    let attempts = 0;
    let loaded = false;
    const tick = async () => {
      try {
        const res = await fetch(runFilePath(taskId, runId, buffer.offset), {
          credentials: "same-origin",
          cache: "no-store",
          signal: AbortSignal.any([controller.signal, AbortSignal.timeout(REQUEST_TIMEOUT_MS)]),
        });
        if (res.ok) {
          const bytes = new Uint8Array(await res.arrayBuffer());
          buffer = appendChunk(buffer, decoder.decode(bytes, { stream: true }), bytes.byteLength);
          loaded = true;
        } else if (res.status !== 416) {
          throw new Error(`status ${res.status}`);
        }
      } catch {
        if (cancelled) return;
        if (!loaded) {
          setState({ buffer, status: "error", following: false });
          return;
        }
      }
      if (cancelled) return;
      attempts += 1;
      const follow = runningRef.current !== false && attempts < MAX_POLL_ATTEMPTS;
      if (!follow) buffer = flushPartial(buffer);
      setState({ buffer, status: "ready", following: follow });
      if (follow) timer = setTimeout(() => void tick(), POLL_INTERVAL_MS);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      controller.abort();
    };
  }, [taskId, runId]);

  return state;
}
