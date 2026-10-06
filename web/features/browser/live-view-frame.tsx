import { type LiveUnavailableReason, liveUnavailableText, safeLivePath } from "./browser-model";
import type { BrowserRunItem, OwnerSession } from "./browser-query";

// D3.2: Live View は gateway が発行した同一 origin の /browser/live/{task}/{run} だけを iframe に入れる。
// raw の live_view_url は使わない。不可のときは理由を出し、イベントでの監視に切り替えたことを明示する。

const KNOWN: readonly string[] = Object.keys(liveUnavailableText);

export type LiveViewState = { kind: "frame"; href: string } | { kind: "unavailable"; reason: LiveUnavailableReason };

/** 理由の判定順: 本人 → 実行中 → 認証区間 → gateway の理由 → href の検査。 */
export function liveViewState(input: {
  taskId: string;
  runId: string;
  run: Pick<BrowserRunItem, "state" | "live_path" | "live">;
  owner: Pick<OwnerSession, "available" | "isOwner"> | undefined;
  authInterval: boolean;
}): LiveViewState {
  const { run, owner, authInterval } = input;
  if (owner && !owner.available) return { kind: "unavailable", reason: "owner_unavailable" };
  if (!owner?.isOwner) return { kind: "unavailable", reason: "not_owner" };
  if (run.state === "COMPLETED" || run.state === "FAILED") return { kind: "unavailable", reason: "not_running" };
  if (authInterval) return { kind: "unavailable", reason: "auth_interval" };
  if (run.live?.state === "disabled")
    return {
      kind: "unavailable",
      reason: KNOWN.includes(run.live.reason) ? (run.live.reason as LiveUnavailableReason) : "relay_unavailable",
    };
  const href = safeLivePath(run.live?.state === "link" ? run.live.href : run.live_path);
  return href === `/browser/live/${input.taskId}/${input.runId}`
    ? { kind: "frame", href }
    : { kind: "unavailable", reason: "not_configured" };
}

export function LiveViewFrame({ state, taskLabel }: { state: LiveViewState; taskLabel: string }) {
  return (
    <section aria-labelledby="browser-live-heading" className="flex flex-col gap-2" data-testid="browser-live-view">
      <h2 id="browser-live-heading" tabIndex={-1} className="text-section font-semibold">
        Live View
      </h2>
      {state.kind === "frame" ? (
        <>
          {/* iframe の前後に focus できる link を置き、キーボードで iframe を飛ばし・抜けられるようにする（D3.6）。 */}
          <a
            href="#browser-live-events"
            className="inline-flex min-h-11 items-center self-start text-label text-primary underline"
          >
            Live View を飛ばしてイベントへ
          </a>
          <iframe
            src={state.href}
            title={`ブラウザの Live View: ${taskLabel}`}
            sandbox="allow-scripts allow-same-origin"
            referrerPolicy="no-referrer"
            className="aspect-live w-full rounded-md border border-border bg-muted"
            data-testid="browser-live-iframe"
          />
          <a
            href={state.href}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex min-h-11 items-center self-start text-label text-primary underline"
          >
            Live View を別タブで開く
          </a>
        </>
      ) : (
        <div
          className="flex aspect-live w-full flex-col items-center justify-center gap-2 rounded-md border border-border bg-muted p-4 text-center"
          data-testid="browser-live-unavailable"
          data-reason={state.reason}
        >
          <p className="font-semibold">映像なし・イベントで監視中</p>
          <p className="text-label text-muted-foreground">{liveUnavailableText[state.reason]}</p>
          <a
            href="#browser-live-events"
            className="inline-flex min-h-11 items-center text-label text-primary underline"
          >
            イベントへ移る
          </a>
        </div>
      )}
    </section>
  );
}
