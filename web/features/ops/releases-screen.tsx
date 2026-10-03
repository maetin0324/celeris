import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { ReleaseItem, ReleasePromoteAccepted, Releases } from "../../api/generated/types";
import { releaseKeys } from "../../api/queries/keys";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import {
  canPromote,
  judgePromotion,
  type PromotionOutcome,
  type PromotionTrack,
  pollDelayMs,
} from "./releases-promotion";

const releasesQuery = {
  queryKey: releaseKeys.list(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Releases>("/api/releases", signal),
} as const;

type Tracked = { track: PromotionTrack; outcome: PromotionOutcome; reconnecting: boolean };

/** 昇格の追跡。結果が確定するまで GET /releases を回し、接続が切れても backoff で続ける。 */
function usePromotionTracker() {
  const queryClient = useQueryClient();
  const [tracked, setTracked] = useState<Tracked | null>(null);
  const [startError, setStartError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const generation = useRef(0);
  const track = tracked?.track;
  const settled = tracked !== null && tracked.outcome.state !== "pending";

  useEffect(() => {
    if (!track || settled) return;
    const mine = generation.current;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    const tick = async () => {
      let delay: number;
      try {
        const data = await apiGet<Releases>("/api/releases", controller.signal);
        if (controller.signal.aborted || mine !== generation.current) return;
        failures = 0;
        queryClient.setQueryData(releaseKeys.list(), data);
        const outcome = judgePromotion(
          data.items.find((item) => item.sha12 === track.sha12),
          track,
        );
        setTracked({ track, outcome, reconnecting: false });
        if (outcome.state !== "pending") return;
        delay = pollDelayMs(0);
      } catch {
        if (controller.signal.aborted || mine !== generation.current) return;
        failures += 1;
        setTracked((previous) => (previous ? { ...previous, reconnecting: true } : previous));
        delay = pollDelayMs(failures);
      }
      timer = setTimeout(() => void tick(), delay);
    };
    timer = setTimeout(() => void tick(), pollDelayMs(0));
    return () => {
      controller.abort();
      if (timer) clearTimeout(timer);
    };
  }, [track, settled, queryClient]);

  async function start(item: ReleaseItem) {
    if (starting || (tracked && tracked.outcome.state === "pending")) return;
    generation.current += 1;
    setStarting(true);
    setStartError(null);
    const base = { sha12: item.sha12, baselinePromotedAt: item.promoted_at ?? null };
    const pressedAt = new Date().toISOString();
    try {
      const accepted = await apiMutate<ReleasePromoteAccepted>(
        "POST",
        `/api/releases/${encodeURIComponent(item.sha12)}/promote`,
        {},
      );
      const next: PromotionTrack = { ...base, startedAt: accepted?.started_at ?? pressedAt };
      setTracked({ track: next, outcome: { state: "pending", detail: "昇格を起動しました" }, reconnecting: false });
    } catch (error) {
      if (error instanceof ApiError && (error.kind === "network" || error.kind === "timeout")) {
        // 応答を受けられなかった。起動したかは分からないので、状態を見て確かめる。
        setTracked({
          track: { ...base, startedAt: pressedAt },
          outcome: { state: "pending", detail: "起動したか確認しています" },
          reconnecting: true,
        });
      } else {
        const body = error instanceof ApiError ? error.body : undefined;
        const detail =
          body && typeof body === "object"
            ? ((body as Record<string, unknown>).error ?? (body as Record<string, unknown>).message)
            : body;
        setStartError(typeof detail === "string" ? detail : "昇格を起動できませんでした");
        setTracked(null);
      }
    } finally {
      setStarting(false);
    }
  }
  return { tracked, startError, starting, start };
}

function PromotionStatus({ tracked, startError }: { tracked: Tracked | null; startError: string | null }) {
  if (startError)
    return (
      <p role="alert" className="text-red-800">
        昇格を起動できませんでした: {startError}
      </p>
    );
  if (!tracked) return null;
  const { outcome, track, reconnecting } = tracked;
  if (outcome.state === "pending")
    return (
      <p role="status" data-testid="promote-pending">
        {track.sha12} を昇格中です（結果を確認するまで完了ではありません）。{outcome.detail}
        {reconnecting && " 接続を待っています。再接続を試みています。"}
      </p>
    );
  if (outcome.state === "failed")
    return (
      <p role="alert" className="text-red-800" data-testid="promote-failed">
        {track.sha12} の昇格に失敗しました: {outcome.error}
      </p>
    );
  return (
    <p role="status" className="text-green-800" data-testid="promote-succeeded">
      {track.sha12} の昇格が完了しました。
    </p>
  );
}

function ReleaseRow({
  item,
  busy,
  onPromote,
}: {
  item: ReleaseItem;
  busy: boolean;
  onPromote: (item: ReleaseItem) => void;
}) {
  return (
    <li className="min-w-0 rounded border p-3 space-y-2" data-testid={`release-${item.sha12}`}>
      <div className="flex flex-wrap items-center gap-2">
        <code className="font-mono break-all">{item.sha12}</code>
        {item.is_current && <span className="rounded border px-1 text-sm">current</span>}
        {item.is_previous && <span className="rounded border px-1 text-sm">previous</span>}
        {item.promoting && <span className="rounded border px-1 text-sm">昇格中</span>}
        <span className="text-sm">{item.gate_ok ? "gate 通過" : "gate 未通過"}</span>
      </div>
      <p className="text-sm break-words">
        {item.ref ? `ref ${item.ref} / ` : ""}ビルド {item.built_at ?? "-"} / 昇格 {item.promoted_at ?? "-"}
      </p>
      {item.problem && <p className="text-red-800 break-words">問題: {item.problem}</p>}
      {item.promote_failed && !item.promoting && (
        <p role="alert" className="text-red-800 break-words">
          直近の昇格の失敗（{item.promote_failed.failed_at}）: {item.promote_failed.error}
        </p>
      )}
      {item.changes && (
        <p className="text-sm">
          変更 {item.changes.commit_count} commit / {item.changes.file_count} file
        </p>
      )}
      <Button disabled={busy || !canPromote(item)} onClick={() => onPromote(item)}>
        {item.sha12} を昇格
      </Button>
    </li>
  );
}

export function ReleasesScreen() {
  const query = useQuery(releasesQuery);
  const promotion = usePromotionTracker();
  const busy = promotion.starting || promotion.tracked?.outcome.state === "pending";
  const data = query.data;
  return (
    <ScreenFrame title="リリース" route="/releases">
      <FetchFrame query={query}>
        {data && (
          <div className="space-y-4 min-w-0">
            <PromotionStatus tracked={promotion.tracked} startError={promotion.startError} />
            <p className="text-sm">
              稼働中 {data.running.release}（{data.running.role}） / current {data.current ?? "-"} / previous{" "}
              {data.previous ?? "-"}
            </p>
            {data.items.length === 0 ? (
              <p>リリースはありません。</p>
            ) : (
              <ul className="space-y-3">
                {data.items.map((item) => (
                  <ReleaseRow key={item.sha12} item={item} busy={busy} onPromote={(i) => void promotion.start(i)} />
                ))}
              </ul>
            )}
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
