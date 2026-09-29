import type { BrowserRun, BrowserWait, RunSummary, Status } from "~/celeris/types";
import type { LiveViewState } from "~/components/BrowserRunsPanel";

/** Dashboard URLs are operator-configured HTTPS endpoints, never browser page URLs. */
export function safeBrowserLiveUrl(value: string | null | undefined): string | null {
  if (!value || value.trim() !== value) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) return null;
    // biome-ignore lint/suspicious/noControlCharactersInRegex: reject URL parser control-character normalization
    if (/[\u0000-\u0020\u007f\\]/.test(value)) return null;
    return url.href;
  } catch {
    return null;
  }
}

export function activeBrowserRunIds(runs: RunSummary[], status: Status): string[] {
  if (status !== "running") return [];
  return runs.filter((run) => !run.finished_at && !run.end && !run.outcome).map((run) => run.run_id);
}

/**
 * ADR-0080 D6: dashboard URL（全 session が見える）は画面・loader data・SSE に出さない。
 * `live_view_url` の値を null に置き換えた複製を返す（入れ子の events / timeline も含む）。
 */
export function redactLiveViewUrls<T>(value: T): T {
  if (Array.isArray(value)) return value.map((v) => redactLiveViewUrls(v)) as T;
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      out[k] = k === "live_view_url" ? null : redactLiveViewUrls(v);
    }
    return out as T;
  }
  return value;
}

const LIVE_VIEW_URL_JSON = /"live_view_url"\s*:\s*"(?:[^"\\]|\\.)*"/g;

/** SSE の 1 行（JSON 文字列を含む）から `live_view_url` の値を消す。 */
export function redactLiveViewUrlText(text: string): string {
  return text.includes("live_view_url") ? text.replace(LIVE_VIEW_URL_JSON, '"live_view_url":null') : text;
}

/** 本人の同一 origin の Live View 経路（raw dashboard URL は使わない）。 */
export function liveViewPath(taskId: string, runId: string): string {
  return `/browser/live/${encodeURIComponent(taskId)}/${encodeURIComponent(runId)}`;
}

/** 認証要求の区間（credential を扱う session）にある run か。区間中は Live View を開かない。 */
export function inAuthInterval(waits: BrowserWait[], runId: string): boolean {
  return waits.some(
    (w) =>
      w.run_id === runId &&
      (w.reason === "waiting_for_auth" || w.credential != null || w.operation?.action === "credential_use"),
  );
}

/** 画面に出す Live View の状態（本人かどうかは呼び出し側の owner 判定）。URL は出さない。 */
export function liveViewLinkFor(
  run: BrowserRun,
  opts: {
    isOwner: boolean;
    ownerAvailable: boolean;
    active: boolean;
    waits: BrowserWait[];
    relayAvailable?: boolean;
    href: string;
  },
): LiveViewState {
  if (!opts.ownerAvailable) return { state: "disabled", reason: "owner_unavailable" };
  if (!opts.isOwner) return { state: "disabled", reason: "not_owner" };
  if (run.state !== "RUNNING" || !opts.active) return { state: "disabled", reason: "not_running" };
  if (!safeBrowserLiveUrl(run.live_view_url)) return { state: "disabled", reason: "not_configured" };
  if (inAuthInterval(opts.waits, run.run_id)) return { state: "disabled", reason: "auth_interval" };
  // relay（`CELERIS_GUI_LIVE_VIEW_UPSTREAM`）が未設定なら本人にも開かない。呼び出し側がサーバで判定して渡す。
  if (!opts.relayAvailable) return { state: "disabled", reason: "relay_unavailable" };
  return { state: "link", href: opts.href };
}

/** 画面へ渡す本人状態（秘密・ID・URL を含まない。ADR-0080 D6）。 */
export interface BrowserOwnerView {
  /** 本人を区別できる構成か（認証が有効）。false なら credential UI と Live View は使えない */
  available: boolean;
  /** この session が本人か */
  isOwner: boolean;
  /** この session に未確定の challenge があるか */
  challengePending: boolean;
  /** 変更系の CSRF token（本人のときだけ） */
  csrfToken: string | null;
}

export const NO_BROWSER_OWNER: BrowserOwnerView = {
  available: false,
  isOwner: false,
  challengePending: false,
  csrfToken: null,
};
