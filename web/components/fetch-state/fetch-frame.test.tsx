import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../../api/client";
import { badgeView, daemonBadges } from "../../api/queries/badges";
import { createDelayTracker, type DelayPhase } from "./delay-tracker";
import { ErrorNotice, FetchFrameView, fetchView } from "./fetch-frame";

describe("delay tracker", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("1 s で待機表示、5 s で slow、stop で戻る", () => {
    const seen: DelayPhase[] = [];
    const t = createDelayTracker((p) => seen.push(p));
    t.start();
    vi.advanceTimersByTime(999);
    expect(seen.at(-1)).toBe("quiet");
    vi.advanceTimersByTime(1);
    expect(seen.at(-1)).toBe("waiting");
    vi.advanceTimersByTime(3_999);
    expect(seen.at(-1)).toBe("waiting");
    vi.advanceTimersByTime(1);
    expect(seen.at(-1)).toBe("slow");
    t.stop();
    expect(seen.at(-1)).toBe("quiet");
  });

  it("1 s より前に stop すると何も出さない", () => {
    const seen: DelayPhase[] = [];
    const t = createDelayTracker((p) => seen.push(p));
    t.start();
    vi.advanceTimersByTime(500);
    t.stop();
    vi.advanceTimersByTime(10_000);
    expect(seen.every((p) => p === "quiet")).toBe(true);
  });
});

const html = (el: ReactElement) => renderToStaticMarkup(el);
const base = { refreshing: false, showError: false, onRetry: () => {} };

describe("FetchFrameView", () => {
  it("初回は skeleton。1 s 前は文言なし、後は読み込み中、5 s 後は時間がかかっています", () => {
    const quiet = html(<FetchFrameView {...base} view="loading" phase="quiet" />);
    expect(quiet).toContain('aria-busy="true"');
    expect(quiet).not.toContain("読み込み中");
    expect(html(<FetchFrameView {...base} view="loading" phase="waiting" />)).toContain("読み込み中");
    expect(html(<FetchFrameView {...base} view="loading" phase="waiting" />)).not.toContain("時間がかかっています");
    expect(html(<FetchFrameView {...base} view="loading" phase="slow" />)).toContain("時間がかかっています");
  });

  it("再取得中は更新中（1 s 後）", () => {
    const shown = html(
      <FetchFrameView {...base} view="ready" phase="waiting" refreshing>
        <b>x</b>
      </FetchFrameView>,
    );
    expect(shown).toContain("更新中");
    expect(shown).toContain("<b>x</b>");
    expect(html(<FetchFrameView {...base} view="ready" phase="quiet" refreshing />)).not.toContain("更新中");
  });

  it("失敗は再試行ボタンで refetch を呼ぶ", () => {
    const onRetry = vi.fn();
    const view = FetchFrameView({ ...base, view: "error", phase: "quiet", onRetry }) as ReactElement<{
      onRetry: () => void;
    }>;
    expect(view.type).toBe(ErrorNotice);
    const el = ErrorNotice(view.props) as ReactElement<{ children: ReactElement<{ onClick?: () => void }>[] }>;
    const markup = html(el);
    expect(markup).toContain('role="alert"');
    expect(markup).toContain("再試行");
    const button = el.props.children.find((c) => c.type === "button");
    button?.props.onClick?.();
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("fetchView: エラーでデータ無しは error、データがあれば ready", () => {
    expect(fetchView({ data: undefined, isPending: false, isError: true })).toBe("error");
    expect(fetchView({ data: undefined, isPending: true, isError: false })).toBe("loading");
    expect(fetchView({ data: 1, isPending: false, isError: true })).toBe("ready");
  });

  it("6 状態をそれぞれの data-fetch-state と回復手段で描画する", () => {
    const views = ["loading", "empty", "error", "stale", "disconnected", "permission-denied"] as const;
    for (const view of views) {
      const markup = html(
        <FetchFrameView
          {...base}
          view={view}
          phase="slow"
          emptyMessage="判断待ちはありません"
          emptyAction={<a href="/tasks">一覧へ</a>}
          lastFetchedAt={1_700_000_000_000}
        >
          <b>保存済みの内容</b>
        </FetchFrameView>,
      );
      expect(markup).toContain(`data-fetch-state="${view}"`);
      if (view === "error" || view === "stale" || view === "disconnected" || view === "permission-denied") {
        expect(markup).toContain('role="alert"');
      }
      if (view === "stale") {
        expect(markup).toContain("保存済みの内容");
        expect(markup).toContain("最終取得:");
        expect(markup).toContain("再取得");
      }
      if (view === "empty") {
        expect(markup).toContain("判断待ちはありません");
        expect(markup).toContain("一覧へ");
      }
      if (view === "disconnected") expect(markup).toContain("再接続中");
      if (view === "permission-denied") expect(markup).not.toContain("保存済みの内容");
    }
  });

  it("ApiError.kind だけで切断・認証不足・権限不足を分け、0 件は明示した時だけ empty にする", () => {
    const error = (kind: "network" | "unauthorized" | "forbidden") =>
      new ApiError(kind, { method: "GET", path: "/api/tasks" });
    const failed = { data: undefined, isPending: false, isError: true };
    expect(fetchView({ ...failed, error: error("network") })).toBe("disconnected");
    expect(fetchView({ ...failed, error: error("unauthorized") })).toBe("permission-denied");
    expect(fetchView({ ...failed, error: error("forbidden") })).toBe("permission-denied");
    expect(fetchView({ ...failed, error: new Error("oops") })).toBe("error");
    expect(fetchView({ data: [], isPending: false, isError: false })).toBe("ready");
    expect(fetchView({ data: [], isPending: false, isError: false }, true)).toBe("empty");
    expect(fetchView({ data: undefined, isPending: false, isError: false }, true)).toBe("ready");
    expect(fetchView({ data: [1], isPending: false, isError: true, error: error("network") })).toBe("stale");
    expect(fetchView({ data: [1], isPending: false, isError: true, error: error("unauthorized") })).toBe(
      "permission-denied",
    );
  });

  it("401 ではログイン、403 では戻り先を示し、保護データは描画しない", () => {
    const unauthorized = html(
      <FetchFrameView {...base} view="permission-denied" phase="quiet" unauthorized>
        <b>secret</b>
      </FetchFrameView>,
    );
    expect(unauthorized).toContain('href="/login"');
    expect(unauthorized).not.toContain("secret");
    const forbidden = html(<FetchFrameView {...base} view="permission-denied" phase="quiet" />);
    expect(forbidden).toContain('href="/"');
  });

  it("最終取得時刻が無い・不正なときは未確認とし、日時を捏造しない", () => {
    const markup = html(<FetchFrameView {...base} view="stale" phase="quiet" lastFetchedAt={Number.NaN} />);
    expect(markup).toContain("未確認");
    expect(markup).not.toContain("dateTime=");
  });
});

describe("badge", () => {
  it("不明は 0 ではなく ? と label", () => {
    for (const v of [null, undefined, Number.NaN]) {
      const b = badgeView(v, "承認待ち");
      expect(b?.text).toBe("?");
      expect(b?.label).toContain("不明");
    }
    expect(badgeView(0, "x")).toBeNull();
    expect(badgeView(3, "x")?.text).toBe("3");
  });

  it("approvals_pending が無いとき null（0 にしない）", () => {
    expect(daemonBadges.select({ awaiting_human: [] } as never).approvalsPending).toBeNull();
    expect(daemonBadges.select({ awaiting_human: [], approvals_pending: 0 } as never).approvalsPending).toBe(0);
  });
});
