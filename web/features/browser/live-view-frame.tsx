import { useEffect, useState } from "react";
import { type LiveUnavailableReason, liveUnavailableText, safeLivePath } from "./browser-model";
import type { BrowserRunItem, OwnerSession } from "./browser-query";

const KNOWN: readonly string[] = Object.keys(liveUnavailableText);
export type LiveViewState = { kind: "frame"; href: string } | { kind: "unavailable"; reason: LiveUnavailableReason };

export function liveViewState(input: {
  taskId: string;
  runId: string;
  run: Pick<BrowserRunItem, "state" | "live_path" | "live">;
  owner: Pick<OwnerSession, "available" | "isOwner"> | undefined;
  authInterval: boolean;
}): LiveViewState {
  const { run, owner } = input;
  if (owner && !owner.available) return { kind: "unavailable", reason: "owner_unavailable" };
  if (!owner?.isOwner) return { kind: "unavailable", reason: "not_owner" };
  if (run.live?.state === "disabled")
    return {
      kind: "unavailable",
      reason: KNOWN.includes(run.live.reason) ? (run.live.reason as LiveUnavailableReason) : "relay_unavailable",
    };
  if (run.state !== "RUNNING") return { kind: "unavailable", reason: "not_running" };
  const href = safeLivePath(run.live?.state === "link" ? run.live.href : run.live_path);
  return href === `/browser/live/${input.taskId}/${input.runId}`
    ? { kind: "frame", href }
    : { kind: "unavailable", reason: "not_configured" };
}

export function LiveViewFrame({
  state,
  taskLabel,
  credentialSession = false,
}: {
  state: LiveViewState;
  taskLabel: string;
  credentialSession?: boolean;
}) {
  const [imageUrl, setImageUrl] = useState<string | null>(null);
  const href = state.kind === "frame" ? state.href : null;
  useEffect(() => {
    if (!href) {
      setImageUrl(null);
      return;
    }
    let current: string | null = null;
    let disposed = false;
    const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${scheme}//${window.location.host}${href}/frames`);
    socket.binaryType = "blob";
    socket.onmessage = (event: MessageEvent<Blob>) => {
      if (!(event.data instanceof Blob) || disposed) return;
      const type = event.data.type === "image/png" ? "image/png" : "image/jpeg";
      const next = URL.createObjectURL(new Blob([event.data], { type }));
      setImageUrl(next);
      if (current) URL.revokeObjectURL(current);
      current = next;
    };
    return () => {
      disposed = true;
      socket.close();
      if (current) URL.revokeObjectURL(current);
      setImageUrl(null);
    };
  }, [href]);

  return (
    <section aria-labelledby="browser-live-heading" className="flex flex-col gap-2" data-testid="browser-live-view">
      <h2 id="browser-live-heading" tabIndex={-1} className="text-section font-semibold">
        Live View
      </h2>
      {credentialSession ? (
        <p className="text-label font-medium" data-testid="live-owner-only">
          credential session の映像は本人だけに表示されます。
        </p>
      ) : null}
      {state.kind === "frame" ? (
        <>
          <a
            href="#browser-live-events"
            className="inline-flex min-h-11 items-center self-start text-label text-primary underline"
          >
            Live View を飛ばしてイベントへ
          </a>
          <div
            className="flex aspect-live w-full items-center justify-center overflow-hidden rounded-md border border-border bg-muted"
            data-testid="browser-live-frame"
          >
            {imageUrl ? (
              <img
                src={imageUrl}
                alt={`ブラウザの Live View: ${taskLabel}`}
                className="max-h-full max-w-full object-contain"
                data-testid="browser-live-image"
              />
            ) : (
              <p className="text-label text-muted-foreground">映像を待っています…</p>
            )}
          </div>
          <p className="text-label text-muted-foreground">読み取り専用の映像です。入力や操作はできません。</p>
        </>
      ) : (
        <div
          className="flex aspect-live w-full flex-col items-center justify-center gap-2 rounded-md border border-border bg-muted p-4 text-center"
          data-testid="browser-live-unavailable"
          data-reason={state.reason}
        >
          <p className="font-semibold">映像なし — イベントで監視中</p>
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
