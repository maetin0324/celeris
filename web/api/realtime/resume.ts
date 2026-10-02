// 復帰の契機（visibilitychange・pageshow・online・focus）を 1 つの debounce した resume にまとめる。

export type ResumeTarget = {
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
};

export type ResumeOptions = {
  onResume: () => void;
  debounceMs?: number;
  /** window 相当（pageshow・online・focus）。 */
  windowTarget: ResumeTarget;
  /** document 相当（visibilitychange）。visible のときだけ数える。 */
  documentTarget: ResumeTarget & { visibilityState?: string };
};

export const RESUME_DEBOUNCE_MS = 300;

/** 戻り値で購読を外す。 */
export function bindResume(options: ResumeOptions): () => void {
  const wait = options.debounceMs ?? RESUME_DEBOUNCE_MS;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const trigger = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      options.onResume();
    }, wait);
  };
  const onVisibility = () => {
    if (options.documentTarget.visibilityState === "hidden") return;
    trigger();
  };
  const w = options.windowTarget;
  const d = options.documentTarget;
  d.addEventListener("visibilitychange", onVisibility);
  for (const t of ["pageshow", "online", "focus"]) w.addEventListener(t, trigger);
  return () => {
    if (timer !== undefined) clearTimeout(timer);
    d.removeEventListener("visibilitychange", onVisibility);
    for (const t of ["pageshow", "online", "focus"]) w.removeEventListener(t, trigger);
  };
}
