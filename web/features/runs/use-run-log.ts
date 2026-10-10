import { useCallback, useEffect, useRef, useState } from "react";
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
// - 取得に失敗したとき・polling の上限で追うのを止めたときは retry で最初から読み直せる（ページの再読み込み不要）。
export type RunLogState = {
  buffer: RunLogBuffer;
  status: "loading" | "ready" | "error";
  following: boolean;
  /** 実行中のまま polling の上限に達して追記を追うのを止めた。 */
  capped: boolean;
};

export type RunLogHandle = RunLogState & { retry: () => void };

const initialState: RunLogState = { buffer: emptyBuffer, status: "loading", following: false, capped: false };

export function useRunLog(taskId: string, runId: string, running: boolean | undefined, enabled = true): RunLogHandle {
  const [state, setState] = useState<RunLogState>(initialState);
  const [attempt, setAttempt] = useState(0);
  const retry = useCallback(() => {
    setState(initialState);
    setAttempt((n) => n + 1);
  }, []);
  const runningRef = useRef(running);
  runningRef.current = running;
  // 終わったと分かったら次の polling を待たずに最後の 1 回を読む（header の状態と「実行中」の表示を食い違わせない）。
  const finishRef = useRef<() => void>(() => {});
  useEffect(() => {
    if (!enabled) {
      setState({ buffer: emptyBuffer, status: "ready", following: false, capped: false });
      return;
    }
    if (running === false) finishRef.current();
  }, [running, enabled]);

  // attempt は retry の合図（最初から読み直す）。
  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt は読み直しの合図として使う。
  useEffect(() => {
    if (!enabled) {
      setState({ buffer: emptyBuffer, status: "ready", following: false, capped: false });
      return;
    }
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
          setState({ buffer, status: "error", following: false, capped: false });
          return;
        }
      }
      if (cancelled) return;
      attempts += 1;
      const stillRunning = runningRef.current !== false;
      const follow = stillRunning && attempts < MAX_POLL_ATTEMPTS;
      if (!follow) buffer = flushPartial(buffer);
      setState({ buffer, status: "ready", following: follow, capped: stillRunning && !follow });
      if (follow)
        timer = setTimeout(() => {
          timer = undefined;
          void tick();
        }, POLL_INTERVAL_MS);
    };
    finishRef.current = () => {
      if (!timer || cancelled) return;
      clearTimeout(timer);
      timer = undefined;
      void tick();
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      controller.abort();
    };
  }, [taskId, runId, attempt, enabled]);

  return { ...state, retry };
}
