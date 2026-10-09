import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { DecisionDetail, TaskDecisionsPanel } from "./decision-detail";
import { decisionView } from "./decision-fixtures.test-support";
import { decisionKeys } from "./decision-model";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
  useLocation: () => "",
}));

function render(node: ReactNode, setup?: (client: QueryClient) => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  setup?.(client);
  const html = renderToStaticMarkup(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
  client.clear();
  return html;
}

describe("decision detail", () => {
  it("decision_detail_answered_choice_shows_answer_history_and_revise_form", () => {
    const view = decisionView();
    const html = render(
      <DecisionDetail
        view={view}
        history={[
          { at: "2026-10-09T01:00:00Z", option: "explicit", label: "(i) explicit-account", note: null, by: "cos" },
          {
            at: "2026-10-09T02:00:00Z",
            option: "explicit",
            label: "(i) explicit-account",
            note: "最初の答え",
            by: "human",
          },
        ]}
      />,
    );
    for (const text of [
      "今の答え",
      "(i) explicit-account",
      "理由: 最初の答え",
      "答えた人",
      "日時",
      "(ii) implicit-account",
      "（推奨）",
      "（今の答え）",
      "回答の履歴（最後の回答が有効）",
      "（有効）",
      "（置き換え済み）",
      "答えを変える…",
      "その他（理由に記述）",
    ])
      expect(html).toContain(text);
    expect(html).toContain('data-testid="decision-revise-form"');
    // 選ぶまでは送れない（確認の段の手前で止める）。
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>答えを変える…/);
    expect(html).not.toContain("decision-revise-unavailable");
  });

  it("decision_detail_non_revisable_shows_reason_without_form", () => {
    const cases = [
      [
        decisionView({ status: "withdrawn", answer: null, withdrawn_reason: "不要になった" }, { answered_at: null }),
        "取り下げ済み",
      ],
      [decisionView({ kind: "leaf_too_large" }), "daemon の決定"],
      [decisionView({ status: "open", answer: null }, { answered_at: null }), "まだ回答されていません"],
    ] as const;
    for (const [view, reason] of cases) {
      const html = render(<DecisionDetail view={view} history={[]} />);
      expect(html).toContain('data-testid="decision-revise-unavailable"');
      expect(html).toContain(reason);
      expect(html).not.toContain("decision-revise-form");
      expect(html).not.toContain("答えを変える…");
    }
    const withdrawn = render(<DecisionDetail view={cases[0][0]} history={[]} />);
    expect(withdrawn).toContain("不要になった");
  });

  it("decision_panel_lists_task_decisions_and_hides_when_empty", () => {
    const html = render(<TaskDecisionsPanel taskId="T1" />, (client) =>
      client.setQueryData(decisionKeys("T1"), {
        items: [decisionView(), decisionView({ id: "D2", question: "未回答の決定", status: "open", answer: null })],
      }),
    );
    expect(html).toContain('id="decision-D1"');
    expect(html).toContain('id="decision-D2"');
    expect(html).toContain("未回答の決定");
    expect(html).toContain("回答済み");
    expect(html).toContain("未回答");
    expect(html).toContain("→ (i) explicit-account");

    const empty = render(<TaskDecisionsPanel taskId="T1" />, (client) =>
      client.setQueryData(decisionKeys("T1"), { items: [] }),
    );
    expect(empty).toContain("決定はありません。");
  });
});
