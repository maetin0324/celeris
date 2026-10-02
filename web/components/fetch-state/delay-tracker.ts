// 取得の待ち時間の段階（ADR-0081 D7）。1 s 未満は何も出さず、1 s で待機表示、5 s で「時間がかかっています」。

export const SHOW_AFTER_MS = 1_000;
export const SLOW_AFTER_MS = 5_000;

export type DelayPhase = "quiet" | "waiting" | "slow";

export function createDelayTracker(onChange: (phase: DelayPhase) => void) {
  let waiting: ReturnType<typeof setTimeout> | undefined;
  let slow: ReturnType<typeof setTimeout> | undefined;
  const clear = () => {
    clearTimeout(waiting);
    clearTimeout(slow);
    waiting = undefined;
    slow = undefined;
  };
  return {
    start() {
      clear();
      onChange("quiet");
      waiting = setTimeout(() => onChange("waiting"), SHOW_AFTER_MS);
      slow = setTimeout(() => onChange("slow"), SLOW_AFTER_MS);
    },
    stop() {
      clear();
      onChange("quiet");
    },
  };
}
