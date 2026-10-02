// 時刻の表示（P2-06・X7）。絶対時刻はブラウザの時刻帯（好みで上書き可）、相対時刻は server との時計のずれで補正する。
// ずれは SSE の hello.now から測る（skew = Date.parse(hello.now) - Date.now()）。

import { useSyncExternalStore } from "react";
import { readPreferences } from "./preferences";

type Clock = { offsetMs: number; lastHello: string | null };
let clock: Clock = { offsetMs: 0, lastHello: null };
const listeners = new Set<() => void>();

export function setServerClockOffset(offsetMs: number, lastHello: string | null = null) {
  clock = { offsetMs: Number.isFinite(offsetMs) ? offsetMs : 0, lastHello };
  for (const l of [...listeners]) l();
}

export function getServerClockOffset(): number {
  return clock.offsetMs;
}

/** hello.now を受けたときに呼ぶ。不正な時刻は無視する。 */
export function recordHello(now: string, localNowMs: number = Date.now()) {
  const server = Date.parse(now);
  if (Number.isNaN(server)) return;
  setServerClockOffset(server - localNowMs, now);
}

export function serverNowMs(localNowMs: number = Date.now()): number {
  return localNowMs + clock.offsetMs;
}

export function useLastHello(): string | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => clock.lastHello,
    () => clock.lastHello,
  );
}

export function resolveTimeZone(override?: string): string {
  return override ?? readPreferences().timeZone ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
}

export function formatAbsolute(value: string | number | Date, timeZone?: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return new Intl.DateTimeFormat("ja-JP", {
    dateStyle: "medium",
    timeStyle: "medium",
    timeZone: resolveTimeZone(timeZone),
  }).format(date);
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
  ["second", 1000],
];

/** `nowMs` は server 時計に補正した現在（既定は serverNowMs()）。 */
export function formatRelative(value: string | number | Date, nowMs: number = serverNowMs()): string {
  const t = new Date(value).getTime();
  if (Number.isNaN(t)) return "";
  const diff = t - nowMs;
  const rtf = new Intl.RelativeTimeFormat("ja", { numeric: "auto" });
  for (const [unit, ms] of UNITS) {
    if (Math.abs(diff) >= ms || unit === "second") return rtf.format(Math.trunc(diff / ms), unit);
  }
  return "";
}
