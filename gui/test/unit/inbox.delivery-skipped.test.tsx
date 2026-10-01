import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { describe, expect, it } from "vitest";
import type { Inbox } from "~/celeris/types";
import InboxPage from "~/routes/inbox";

function render(el: ReactElement): string {
  const router = createMemoryRouter([{ path: "/", element: el }], { initialEntries: ["/"] });
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

describe("delivery-skipped inbox attention", () => {
  it("renders the task link, summary reason, detail, reason code, and head", () => {
    const inbox: Inbox = {
      approvals: [],
      questions: [],
      drafts: [],
      attention: [
        {
          type: "delivery_skipped",
          at: "2026-10-01T00:00:00Z",
          task: {
            id: "01M3DELIVERYSKIPPED0000001",
            title: "root delivery task",
            kind: "execute",
            status: "done",
            actions: [],
          },
          summary: "完了したが main への取り込みを開始できません: 部署を決められません",
          detail: "担当、計画者、実行担当から部署を特定できませんでした。",
          reason: "department_unresolved",
          head: "0123456789abcdef",
        },
      ],
      browser_waits: [],
      decisions: [],
      counts: { approvals: 0, questions: 0, drafts: 0, attention: 1, browser_waits: 0, decisions: 0, by_status: {} },
    };
    const html = render(
      createElement(InboxPage, { loaderData: { inbox, fetchedAt: "2026-10-01T00:00:00Z" } } as never),
    );

    expect(html).toContain('data-attention-type="delivery_skipped"');
    expect(html).toContain("完了したが main への取り込みを開始できません");
    expect(html).toContain("担当、計画者、実行担当から部署を特定できませんでした。");
    expect(html).toContain("department_unresolved");
    expect(html).toContain("0123456789abcdef");
    expect(html).toContain('href="/tasks/01M3DELIVERYSKIPPED0000001"');
  });
});
