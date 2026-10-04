import { type ReactNode, useEffect, useState } from "react";
import { isApiError } from "../../api/client";
import { buttonClassName } from "../ui/button";
import { createDelayTracker, type DelayPhase } from "./delay-tracker";

// 取得失敗を空結果に変換しない。dataUpdatedAt は対象の更新時刻ではなく最終成功取得時刻。
export type FetchStateLike = {
  data: unknown;
  isPending: boolean;
  isError: boolean;
  isFetching: boolean;
  refetch: () => unknown;
  error?: unknown;
  dataUpdatedAt?: number;
};

export type FetchView = "loading" | "error" | "ready" | "empty" | "stale" | "disconnected" | "permission-denied";

type ViewInput = Pick<FetchStateLike, "data" | "isPending" | "isError"> & Partial<Pick<FetchStateLike, "error">>;

/** empty は取得成功が確定した画面だけが指定する。古い呼び出しの ready 判定は維持する。 */
export function fetchView(q: ViewInput, empty = false): FetchView {
  if (q.isError && isApiError(q.error) && (q.error.kind === "unauthorized" || q.error.kind === "forbidden")) {
    return "permission-denied";
  }
  if (q.data === undefined && q.isError) {
    if (isApiError(q.error) && (q.error.kind === "network" || q.error.kind === "timeout")) return "disconnected";
    return "error";
  }
  if (q.data === undefined && q.isPending) return "loading";
  if (q.data !== undefined && q.isError && q.error !== undefined) return "stale";
  if (empty && q.data !== undefined && !q.isError) return "empty";
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

type RetryProps = { onRetry: () => void };

function RetryButton({ onRetry, label = "再取得" }: RetryProps & { label?: string }) {
  return (
    <button type="button" className={buttonClassName} onClick={onRetry}>
      {label}
    </button>
  );
}

export function LoadingState({ phase, skeleton, onRetry }: { phase: DelayPhase; skeleton?: ReactNode } & RetryProps) {
  return (
    <div aria-busy="true" data-fetch-state="loading" className="min-w-0">
      {skeleton ?? <div aria-hidden="true" className="h-16 rounded bg-neutral" />}
      {phase !== "quiet" && (
        <p role="status" className="mt-2 text-label text-muted-foreground">
          読み込み中…
        </p>
      )}
      {phase === "slow" && (
        <div className="mt-2 flex flex-wrap items-center gap-2 text-label text-muted-foreground">
          <span>時間がかかっています。接続を確認してください。</span>
          <RetryButton onRetry={onRetry} />
        </div>
      )}
    </div>
  );
}

export function EmptyState({
  message = "表示する項目はありません。",
  action,
}: {
  message?: string;
  action?: ReactNode;
}) {
  return (
    <div
      data-fetch-state="empty"
      role="status"
      className="flex min-w-0 flex-wrap items-center gap-2 border-t border-border py-3 text-body text-muted-foreground"
    >
      <span>{message}</span>
      {action}
    </div>
  );
}

/** ErrorNotice は旧 API と data-fetch-state="error" を維持する。 */
export function ErrorNotice({ onRetry, subject = "データ" }: RetryProps & { subject?: string }) {
  return (
    <div
      role="alert"
      data-fetch-state="error"
      className="flex min-w-0 flex-wrap items-center gap-2 border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
    >
      <span>
        {subject === "データ" ? "取得に失敗しました。" : `${subject}を取得できません。`}
        表示を更新できません。再取得してください。
      </span>
      <button type="button" className={buttonClassName} onClick={onRetry}>
        再試行
      </button>
    </div>
  );
}

export { ErrorNotice as ErrorState };

/** 最終成功取得時刻だけを示す。データを子として残し、失敗中の操作は画面側で抑止する。 */
export function StaleState({
  onRetry,
  lastFetchedAt,
  children,
}: RetryProps & { lastFetchedAt?: number; children?: ReactNode }) {
  const validTime = lastFetchedAt !== undefined && Number.isFinite(lastFetchedAt) && lastFetchedAt > 0;
  const fetched = validTime
    ? new Intl.DateTimeFormat("ja-JP", { dateStyle: "short", timeStyle: "short" }).format(lastFetchedAt)
    : "未確認";
  return (
    <div data-fetch-state="stale" className="min-w-0">
      <div
        role="alert"
        className="flex flex-wrap items-center gap-2 border-l-2 border-warning-foreground bg-warning px-3 py-2 text-body text-warning-foreground"
      >
        <span>
          更新を確認できません。最終取得:{" "}
          <time dateTime={validTime ? new Date(lastFetchedAt).toISOString() : undefined}>{fetched}</time>
        </span>
        <RetryButton onRetry={onRetry} />
      </div>
      {children}
    </div>
  );
}

export function DisconnectedState({ onRetry }: RetryProps) {
  return (
    <div
      data-fetch-state="disconnected"
      role="alert"
      className="flex min-w-0 flex-wrap items-center gap-2 border-l-2 border-warning-foreground bg-warning px-3 py-2 text-body text-warning-foreground"
    >
      <span>接続が切れています。再接続中です。接続後に自動で再取得します。</span>
      <RetryButton onRetry={onRetry} />
    </div>
  );
}

export function PermissionDeniedState({
  unauthorized = false,
  action,
  subject = "この内容",
}: {
  unauthorized?: boolean;
  action?: ReactNode;
  subject?: string;
}) {
  return (
    <div
      data-fetch-state="permission-denied"
      role="alert"
      className="flex min-w-0 flex-wrap items-center gap-2 border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
    >
      <span>
        {unauthorized
          ? `ログインが必要なため、${subject}を表示できません。`
          : `${subject}を表示する権限がありません。管理者に権限を確認してください。`}
      </span>
      {action ?? (
        <a className={buttonClassName} href={unauthorized ? "/login" : "/"}>
          {unauthorized ? "ログインへ" : "一覧へ戻る"}
        </a>
      )}
    </div>
  );
}

export type FetchFrameViewProps = {
  view: FetchView;
  phase: DelayPhase;
  refreshing: boolean;
  /** 旧呼び出し用。ready の古いデータを残し、stale 表示へ移す。 */
  showError: boolean;
  onRetry: () => void;
  skeleton?: ReactNode;
  children?: ReactNode;
  emptyMessage?: string;
  emptyAction?: ReactNode;
  subject?: string;
  lastFetchedAt?: number;
  unauthorized?: boolean;
  permissionAction?: ReactNode;
};

export function FetchFrameView({
  view,
  phase,
  refreshing,
  showError,
  onRetry,
  skeleton,
  children,
  emptyMessage,
  emptyAction,
  subject,
  lastFetchedAt,
  unauthorized,
  permissionAction,
}: FetchFrameViewProps) {
  if (view === "loading") return <LoadingState phase={phase} skeleton={skeleton} onRetry={onRetry} />;
  if (view === "error") return <ErrorNotice onRetry={onRetry} subject={subject} />;
  if (view === "empty") return <EmptyState message={emptyMessage} action={emptyAction} />;
  if (view === "stale")
    return (
      <StaleState onRetry={onRetry} lastFetchedAt={lastFetchedAt}>
        {children}
      </StaleState>
    );
  if (view === "disconnected") return <DisconnectedState onRetry={onRetry} />;
  if (view === "permission-denied")
    return <PermissionDeniedState unauthorized={unauthorized} action={permissionAction} subject={subject} />;
  return (
    <div data-fetch-state="ready" className="min-w-0">
      {showError && <StaleState onRetry={onRetry} lastFetchedAt={lastFetchedAt} />}
      {refreshing && phase !== "quiet" && (
        <p role="status" className="text-label text-muted-foreground">
          更新中
        </p>
      )}
      {phase === "slow" && refreshing && <p className="text-label text-muted-foreground">時間がかかっています</p>}
      {children}
    </div>
  );
}

export type FetchFrameProps = {
  query: FetchStateLike;
  skeleton?: ReactNode;
  children: ReactNode;
  /** 取得成功の空結果だけに指定する。 */
  empty?: boolean;
  emptyMessage?: string;
  emptyAction?: ReactNode;
  subject?: string;
  permissionAction?: ReactNode;
};

export function FetchFrame({
  query,
  skeleton,
  children,
  empty = false,
  emptyMessage,
  emptyAction,
  subject,
  permissionAction,
}: FetchFrameProps) {
  const view = fetchView(query, empty);
  const phase = useDelayPhase(query.isFetching);
  return (
    <FetchFrameView
      view={view}
      phase={phase}
      refreshing={query.isFetching && view === "ready"}
      showError={view === "ready" && query.isError}
      onRetry={() => void query.refetch()}
      skeleton={skeleton}
      emptyMessage={emptyMessage}
      emptyAction={emptyAction}
      subject={subject}
      lastFetchedAt={query.dataUpdatedAt}
      unauthorized={isApiError(query.error) && query.error.kind === "unauthorized"}
      permissionAction={permissionAction}
    >
      {children}
    </FetchFrameView>
  );
}
