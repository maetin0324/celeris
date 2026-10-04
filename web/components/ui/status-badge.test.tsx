import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { badgeTones } from "./badge";
import { StatusBadge, statusLabel, statusTone, statusView } from "./status-badge";

const taskStatuses = ["draft", "ready", "running", "blocked", "reviewing", "done", "failed", "cancelled"];
const workUnitStatuses = [
  "pending",
  "ready",
  "needs_continuation",
  "running",
  "done",
  "failed",
  "blocked",
  "superseded",
  "cancelled",
];
const runOutcomes = ["done", "question", "error", "requeue", "lease_expired", "interrupted", "continued"];

describe("statusTone / statusLabel", () => {
  it("task・WorkUnit・run の状態語をすべて 6 tone のどれかと日本語ラベルへ写す", () => {
    for (const s of [...taskStatuses, ...workUnitStatuses, ...runOutcomes]) {
      const view = statusView(s);
      expect(badgeTones).toContain(view.tone);
      expect(view.label).not.toBe(s);
      expect(view.label.length).toBeGreaterThan(0);
    }
    expect(Object.keys(statusTone).sort()).toEqual(Object.keys(statusLabel).sort());
  });

  it("代表的な状態の写像", () => {
    expect(statusView("running")).toEqual({ tone: "running", label: "実行中" });
    expect(statusView("done")).toEqual({ tone: "success", label: "完了" });
    expect(statusView("failed")).toEqual({ tone: "danger", label: "失敗" });
    expect(statusView("error")).toEqual({ tone: "danger", label: "エラー" });
    expect(statusView("blocked")).toEqual({ tone: "warning", label: "停止中" });
    expect(statusView("reviewing")).toEqual({ tone: "info", label: "レビュー中" });
    expect(statusView("draft")).toEqual({ tone: "neutral", label: "下書き" });
    expect(statusView("needs_continuation")).toEqual({ tone: "warning", label: "続きが必要" });
  });

  it("未知の状態は neutral「未確認」で出し、原文は data-status と title に残す", () => {
    expect(statusView("未知の値")).toEqual({ tone: "neutral", label: "未確認" });
    expect(statusView("mystery_state")).toEqual({ tone: "neutral", label: "未確認" });
    // Object の prototype の名前も未知の状態として扱う
    expect(statusView("toString")).toEqual({ tone: "neutral", label: "未確認" });
    const html = renderToStaticMarkup(<StatusBadge status="未知の値" />);
    expect(html).toContain('data-tone="neutral"');
    expect(html).toContain('data-status="未知の値"');
    expect(html).toContain('title="未知の値"');
    expect(html).toContain(">未確認</span>");
    expect(html).not.toContain(">未知の値</span>");
  });
});

describe("StatusBadge", () => {
  it("可視ラベルを必ず出す（色だけで意味を伝えない）", () => {
    const html = renderToStaticMarkup(<StatusBadge status="failed" />);
    expect(html).toContain('data-tone="danger"');
    expect(html).toContain('data-status="failed"');
    expect(html).toContain("失敗");
  });

  it("running は静止した印と「実行中」の文字で出し、動かさない", () => {
    const html = renderToStaticMarkup(<StatusBadge status="running" />);
    expect(html).toContain('data-tone="running"');
    expect(html).toContain('data-slot="status-mark"');
    expect(html).toContain('aria-hidden="true"');
    expect(html).toContain("実行中");
    const mark = html.match(/<span[^>]*data-slot="status-mark"[^>]*>/)?.[0] ?? "";
    expect(mark).not.toBe("");
    const markClass = mark.match(/class="([^"]*)"/)?.[1] ?? "";
    expect(markClass).not.toBe("");
    expect(markClass).not.toMatch(/animate/);
  });

  it("既知の状態の title は原文を持つ", () => {
    const html = renderToStaticMarkup(<StatusBadge status="running" />);
    expect(html).toContain('title="running"');
  });

  it("running 以外には印を出さない", () => {
    const html = renderToStaticMarkup(<StatusBadge status="done" />);
    expect(html).not.toContain("status-mark");
  });
});
