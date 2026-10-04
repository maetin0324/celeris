/// <reference lib="dom" />
import { expect, type Page } from "@playwright/test";

// 試験側だけの観測点（画面の実装は変えない）。addInitScript で app の module より先に入れ、
// - EventSource の listener を包み、app の listener が frame を処理し終えた数を種類ごとに数える。
// - window.fetch を包み、app が呼んだ取得を daemon の path（/api/v1/...）で呼んだ順に記録し、進行中の本数を数える。
// 固定時間を待たず「app が SSE を処理し終えた」「app の取得が全部終わった」という出来事を待つために使う。
export type RealtimeProbeState = {
  handled: Record<string, number>;
  fetches: string[];
  inflight: number;
};

declare global {
  interface Window {
    __e2eRealtime?: RealtimeProbeState;
  }
}

export async function installRealtimeProbe(page: Page) {
  await page.addInitScript(() => {
    const state: RealtimeProbeState = { handled: {}, fetches: [], inflight: 0 };
    window.__e2eRealtime = state;
    const addListener = EventTarget.prototype.addEventListener;
    Object.defineProperty(EventSource.prototype, "addEventListener", {
      configurable: true,
      writable: true,
      value(
        this: EventSource,
        type: string,
        listener: EventListenerOrEventListenerObject | null,
        options?: boolean | AddEventListenerOptions,
      ) {
        if (typeof listener !== "function") return addListener.call(this, type, listener, options);
        const wrapped = (event: Event) => {
          try {
            listener.call(this, event);
          } finally {
            state.handled[type] = (state.handled[type] ?? 0) + 1;
          }
        };
        return addListener.call(this, type, wrapped, options);
      },
    });
    const nativeFetch = window.fetch.bind(window);
    window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
      const raw = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const url = new URL(raw, window.location.href);
      // gateway の中継先（daemon の path）に揃える: `/api/x` と `/files/x` は daemon の `/api/v1/x`。
      const relayed = url.pathname.replace(/^\/(api(?!\/v1\/)|files)\//, "/api/v1/");
      if (relayed.startsWith("/api/")) state.fetches.push(relayed);
      state.inflight++;
      return nativeFetch(input, init).finally(() => {
        state.inflight--;
      });
    };
  });
}

export function readRealtimeProbe(page: Page): Promise<RealtimeProbeState> {
  return page.evaluate(() => {
    const state = window.__e2eRealtime;
    if (!state) throw new Error("realtime probe is not installed");
    return { handled: { ...state.handled }, fetches: [...state.fetches], inflight: state.inflight };
  });
}

/** app の listener が `type` の frame を `count` 件処理し終えるまで待つ（page の rAF は clock が差し替えるので node 側で poll する）。 */
export async function waitHandled(page: Page, type: string, count: number) {
  await expect
    .poll(() => page.evaluate((name) => window.__e2eRealtime?.handled[name] ?? 0, type), {
      message: `SSE ${type} handled`,
    })
    .toBeGreaterThanOrEqual(count);
}

/**
 * 進行中の fetch が 0 になるまで待つ。MessageChannel の macrotask を挟み（Playwright の clock は
 * 差し替えない）、取りかけの promise の続きが fetch を呼び終えてから数える。
 */
export async function waitFetchesSettled(page: Page) {
  await expect
    .poll(
      () =>
        page.evaluate(async () => {
          await new Promise<void>((resolve) => {
            const channel = new MessageChannel();
            channel.port1.onmessage = () => resolve();
            channel.port2.postMessage(null);
          });
          return window.__e2eRealtime?.inflight;
        }),
      { message: "fetches in flight" },
    )
    .toBe(0);
}
