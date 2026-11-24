import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ProjectDetail, ProjectOverview, ProjectOverviewItem } from "../../api/generated/types";
import {
  entrySummary,
  markViews,
  overviewItems,
  ProjectOverviewPanel,
  strongWarnings,
  warningText,
} from "./project-overview-panel";

// 守る不変条件（ADR 2026-11-23-project-observability-publish D1.2・D1.3・D1.4）: 14 項目を決めた順で出す、印 4 種を文字で区別する、
// 記録の無い項目に印を付けない、強い警告だけを先頭にまとめる、外部の URL は https だけを noopener で開く。
// 既存の試験は案件の詳細の概要（request・status）しか見ず、この panel が無い base では全部落ちる。

vi.mock("@tanstack/react-router", () => ({
  Link: ({
    children,
    to,
    params,
    search,
    ...rest
  }: {
    children: ReactNode;
    to: string;
    params?: Record<string, string>;
    search?: Record<string, string>;
  }) => {
    const path = Object.entries(params ?? {}).reduce((acc, [key, value]) => acc.replace(`$${key}`, value), to);
    const query = new URLSearchParams(search ?? {}).toString();
    return (
      <a href={query ? `${path}?${query}` : path} {...rest}>
        {children}
      </a>
    );
  },
}));

const detail = {
  project: { id: "P1", title: "案件", request: "依頼", status: "active", created_at: "", updated_at: "" },
  tasks: [{ id: "T9", title: "実験", status: "running" }],
} as unknown as ProjectDetail;

const recorded = (key: string, mark: ProjectOverviewItem["mark"], entries: ProjectOverviewItem["entries"]) => ({
  key,
  recorded: true,
  mark,
  entries,
});

function overview(over: Partial<ProjectOverview> = {}): ProjectOverview {
  const items: ProjectOverviewItem[] = overviewItems.map(([key]) => ({
    key,
    recorded: false,
    mark: null,
    entries: [],
  }));
  items[0] = recorded("original_goal", "confirmed", [
    { id: "goal", mark: "confirmed", detail: { request_sha256: "f".repeat(64) }, links: [] },
  ]);
  items[3] = recorded("hypotheses", "contradicted", [
    { id: "h1", mark: "estimated", detail: { statement: "提出の回数で律速する" }, links: [] },
    { id: "h2", mark: "contradicted", detail: { statement: "page cache が効く" }, links: [] },
  ]);
  items[5] = recorded("refutations", "estimated", [
    {
      id: "ev",
      mark: "estimated",
      detail: { hypothesis: "h2", statement: "drop_caches でも同じ", references: 1 },
      links: [
        { kind: "literature", url: "https://example.org/p" },
        { kind: "literature", url: "http://example.org/plain" },
        { kind: "git", repo: "web", sha: "a".repeat(40), task_id: "T9" },
      ],
    },
  ]);
  items[8] = recorded("experiments_and_tasks", "confirmed", [
    {
      id: "T9",
      mark: "confirmed",
      detail: { task_id: "T9", strategy_name: "strategy/io/v2.json", candidate: "c1" },
      links: [
        { kind: "task", task_id: "T9" },
        { kind: "log", task_id: "T9", run_id: "R4" },
        { kind: "notebook", task_id: "T9", name: "sweep.ipynb" },
      ],
    },
  ]);
  items[11] = recorded("pending_human_decisions", "needs_decision", [
    {
      id: "D3",
      mark: "needs_decision",
      detail: { decision_id: "D3", key_kind: "strategy-approve" },
      links: [
        { kind: "chat", thread_id: "TH1", message_id: "M1", revision: 2, sha256: "d".repeat(64), state: "edited" },
      ],
    },
  ]);
  return {
    schema: "celeris.project-overview/1",
    project_id: "P1",
    generated_at: "2026-11-23T00:00:00Z",
    request_sha256: "f".repeat(64),
    items,
    warnings: [
      { code: "contradicted", item: "hypotheses", strong: true, entry: "h2", links: [] },
      { code: "needs_decision", item: "pending_human_decisions", strong: true, entry: "D3", links: [] },
      { code: "cost_unknown", item: "budget_spent", strong: false, entry: null, links: [] },
    ],
    truncated: [],
    ...over,
  };
}

function render(value: ProjectOverview): string {
  return renderToStaticMarkup(<ProjectOverviewPanel overview={value} detail={detail} />);
}

/** 1 項目の HTML（`data-item="key"` から次の項目の手前まで）。 */
function itemHtml(html: string, key: string): string {
  const start = html.indexOf(`data-item="${key}"`);
  const next = html.indexOf('data-item="', start + 1);
  return start < 0 ? "" : html.slice(start, next < 0 ? undefined : next);
}

/** 項目の見出しの印（見出しの badge は項目の中で最初の `data-mark`）。 */
function headingMark(html: string, key: string): string | undefined {
  const part = itemHtml(html, key);
  return part.slice(0, part.indexOf("</h3>")).match(/data-mark="([a-z_]+)"[^>]*>([^<]+)</)?.[2];
}

function attrs(html: string, name: string): string[] {
  return [...html.matchAll(new RegExp(`${name}="([^"]*)"`, "g"))].map((m) => m[1] ?? "");
}

function hrefsOf(html: string, kind: string): string[] {
  return [
    ...html.matchAll(
      new RegExp(`<a href="([^"]*)"[^>]*data-link="${kind}"|<a [^>]*data-link="${kind}"[^>]*href="([^"]*)"`, "g"),
    ),
  ].map((m) => (m[1] ?? m[2] ?? "").replaceAll("&amp;", "&"));
}

describe("ProjectOverviewPanel", () => {
  it("project_overview_panel_lists_fourteen_items_in_adr_order", () => {
    const html = render(overview());
    expect(attrs(html, "data-item")).toEqual(overviewItems.map(([key]) => key));
    expect(attrs(html, "data-item")).toHaveLength(14);
    expect(itemHtml(html, "original_goal")).toContain("O1 原目的");
    expect(itemHtml(html, "publish_readiness")).toContain("O14 公開の準備度");
  });

  it("project_overview_panel_marks_are_distinct_text_and_unrecorded_items_have_no_mark", () => {
    const labels = Object.values(markViews).map((view) => view.label);
    expect(new Set(labels).size).toBe(4);
    expect(new Set(Object.values(markViews).map((view) => view.tone)).size).toBe(4);
    const html = render(overview());
    expect(headingMark(html, "hypotheses")).toBe("矛盾・反証");
    expect(headingMark(html, "original_goal")).toBe("確認済み");
    expect(headingMark(html, "pending_human_decisions")).toBe("要判断");
    expect(itemHtml(html, "hypotheses")).toMatch(/data-entry="h1" data-mark="estimated">.*?>推定</);
    const unrecorded = itemHtml(html, "publish_readiness");
    expect(unrecorded).not.toContain("data-mark");
    expect(unrecorded).toContain("記録無し");
  });

  it("project_overview_panel_collects_only_strong_warnings_at_the_top", () => {
    const value = overview();
    expect(strongWarnings(value).map((w) => w.code)).toEqual(["contradicted", "needs_decision"]);
    const html = render(value);
    const top = html.indexOf('data-testid="overview-strong-warnings"');
    expect(top).toBeGreaterThan(0);
    expect(top).toBeLessThan(html.indexOf('data-item="'));
    const section = html.slice(top, html.indexOf("</section>", top));
    expect(attrs(section, "data-warning")).toEqual(["contradicted", "needs_decision"]);
    expect(section).toContain('data-count="2"');
    expect(section).toContain("強い警告 2 件");
    expect(section).toMatch(/強い警告<\/span>（仮説）: (<!-- -->)?矛盾・反証・失効がある/);
    expect(section).not.toContain(warningText("cost_unknown"));
    expect(render(overview({ warnings: [] }))).toContain("強い警告はありません");
  });

  it("project_overview_panel_links_drill_down_to_existing_routes_and_https_only", () => {
    const html = render(overview());
    expect(hrefsOf(html, "task")).toContain("/tasks/T9");
    expect(hrefsOf(html, "log")).toEqual(["/tasks/T9/runs/R4"]);
    expect(hrefsOf(html, "notebook")).toEqual(["/tasks/T9?tab=artifacts"]);
    expect(hrefsOf(html, "files")).toEqual(["/tasks/T9/files"]);
    expect(hrefsOf(html, "git")).toEqual(["/tasks/T9/changes"]);
    expect(hrefsOf(html, "chat")).toEqual(["/?thread=TH1"]);
    expect(html).toContain("引用: edited");
    expect(html).toContain(
      'href="https://example.org/p" target="_blank" rel="noopener noreferrer" data-link="literature"',
    );
    expect(html).not.toContain('href="http://example.org/plain"');
    expect(html).toContain('<code class="break-all">http://example.org/plain</code>');
  });

  it("project_overview_panel_reads_task_status_from_project_detail", () => {
    const part = itemHtml(render(overview()), "experiments_and_tasks");
    expect(part).toContain('data-status="running"');
    expect(part).toContain("strategy/io/v2.json の候補 c1");
  });

  it("entry_summary_formats_item_details", () => {
    expect(
      entrySummary("verification", { hypothesis: "h1", support: 2, refute: 1, unknown: 0, experiment_tasks: ["T"] }),
    ).toBe("h1: 支持 2・反証 1・未確認 0、識別実験の task 1 件");
    expect(entrySummary("latest_findings", { axis: "goal", value: "unknown" })).toBe("目的（評価の値）の軸: unknown");
    expect(
      entrySummary("discussion_sources", { source_label: "ChatGPT", untrusted: true, review_state: "pending" }),
    ).toBe("ChatGPT（外部の文・untrusted）、採否 pending");
    expect(entrySummary("risks", { by_code: [{ code: "budget_exceeded", count: 2 }] })).toBe("消費が予算を超えた 2 件");
    expect(warningText("unknown_code")).toBe("unknown_code");
  });
});
