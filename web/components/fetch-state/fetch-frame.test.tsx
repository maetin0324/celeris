import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
