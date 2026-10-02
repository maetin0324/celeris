import { type ReactNode, useEffect, useState } from "react";
import { buttonClassName } from "../ui/button";
import { createDelayTracker, type DelayPhase } from "./delay-tracker";

// 取得の状態を出す共通の枠（P2-03）。失敗や未取得を 0 件や空表示に置き換えない。
export type FetchStateLike = {
  data: unknown;
  isPending: boolean;
  isError: boolean;
  isFetching: boolean;
  refetch: () => unknown;
};

export type FetchView = "loading" | "error" | "ready";

export function fetchView(q: Pick<FetchStateLike, "data" | "isPending" | "isError">): FetchView {
  if (q.data === undefined && q.isError) return "error";
  if (q.data === undefined && q.isPending) return "loading";
  return "ready";
}

export function useDelayPhase(active: boolean): DelayPhase {
  const [phase, setPhase] = useState<DelayPhase>("quiet");
  useEffect(() => {
    if (!active) {
      setPhase("quiet");
      return;
    }
    const tracker = createDelayTracker(setPhase);
    tracker.start();
    return () => tracker.stop();
  }, [active]);
  return phase;
}

export function FetchFrameView({
  view,
  phase,
  refreshing,
  showError,
  onRetry,
  skeleton,
  children,
}: {
  view: FetchView;
  phase: DelayPhase;
  refreshing: boolean;
  /** データがあるまま再取得に失敗した（古い表示を残して知らせる）。 */
  showError: boolean;
  onRetry: () => void;
  skeleton?: ReactNode;
  children?: ReactNode;
}) {
  if (view === "loading") {
    return (
      <div aria-busy="true" data-fetch-state="loading">
        {skeleton ?? <div className="h-16 animate-pulse rounded bg-neutral-100" aria-hidden="true" />}
        {phase !== "quiet" && (
          <p role="status" className="text-sm text-neutral-600">
            読み込み中…
          </p>
        )}
        {phase === "slow" && <p className="text-sm text-neutral-700">時間がかかっています</p>}
      </div>
    );
  }
  if (view === "error") return <ErrorNotice onRetry={onRetry} />;
  return (
    <div data-fetch-state="ready">
      {showError && <ErrorNotice onRetry={onRetry} />}
      {refreshing && phase !== "quiet" && (
        <p role="status" className="text-xs text-neutral-600">
          更新中
        </p>
      )}
      {phase === "slow" && refreshing && <p className="text-xs text-neutral-700">時間がかかっています</p>}
      {children}
    </div>
  );
}

export function ErrorNotice({ onRetry }: { onRetry: () => void }) {
  return (
    <div role="alert" data-fetch-state="error" className="flex flex-wrap items-center gap-2 text-sm">
      <span>取得に失敗しました。</span>
      <button type="button" className={buttonClassName} onClick={onRetry}>
        再試行
      </button>
    </div>
  );
}

export function FetchFrame({
  query,
  skeleton,
  children,
}: {
  query: FetchStateLike;
  skeleton?: ReactNode;
  children: ReactNode;
}) {
  const view = fetchView(query);
  const phase = useDelayPhase(query.isFetching);
  return (
    <FetchFrameView
      view={view}
      phase={phase}
      refreshing={query.isFetching && view === "ready"}
      showError={view === "ready" && query.isError}
      onRetry={() => void query.refetch()}
      skeleton={skeleton}
    >
      {children}
    </FetchFrameView>
  );
}
