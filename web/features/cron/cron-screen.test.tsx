import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { configureApiClient } from "../../api/client";
import type { CronJobList, CronJobView } from "../../api/generated/types";
import { message } from "../chat/data/fixtures.test-support";
import { CronDetailScreen, CronScreen, ReviewThreadMessages } from "./cron-screen";
import { cronDetailQuery, cronListQuery, reviewThreadQuery } from "./queries";

vi.mock("@tanstack/react-router", () => ({
  Link: ({
    to,
    hash,
    children,
    params: _params,
    ...props
  }: {
    to: string;
    hash?: string;
    children: ReactNode;
    params?: unknown;
  }) => (
    <a href={hash ? `${to}#${hash}` : to} {...props}>
      {children}
    </a>
  ),
}));

const AT = "2026-10-28T00:00:00Z";
function job(over: Partial<CronJobView> = {}): CronJobView {
  return {
    id: "cos-review",
    name: "定期見直し",
    schedule: "0 * * * *",
    timezone: "Asia/Tokyo",
    enabled: true,
    catch_up: "skip",
    overlap: "skip",
    template: { title: "定期見直し" },
    created_at: AT,
    updated_at: AT,
    next_fire_at: null,
    last_run: null,
    review_thread_id: "rv",
    ...over,
  } as CronJobView;
}
const review = [
  message("m2", 2, { thread_id: "rv", role: "assistant", text: "変化なし" }),
  message("m1", 1, { thread_id: "rv", role: "user", text: "定期見直し: 変わった案件 0 件" }),
];

function render(client: QueryClient, node: ReactNode) {
  return renderToStaticMarkup(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

afterEach(() => configureApiClient({ fetcher: (input, init) => fetch(input, init) }));

describe("cron screen and the review thread (ADR 2026-10-28-cos-review-back-thread D6)", () => {
  it("cron_list_links_the_cos_review_job_to_its_review_thread_history", () => {
    const client = new QueryClient();
    const list: CronJobList = { items: [job(), job({ id: "daily", name: "日次整理", review_thread_id: null })] };
    client.setQueryData(cronListQuery.queryKey, list);
    const html = render(client, <CronScreen />);
    expect(html).toContain('href="/cron/$id#review-thread"');
    expect(html.match(/見直しの記録を読む/g)).toHaveLength(1);
  });

  it("cron_detail_shows_the_review_thread_read_only_without_a_composer", () => {
    const client = new QueryClient();
    client.setQueryData(cronDetailQuery("cos-review").queryKey, {
      job: job(),
      history: { job_id: "cos-review", items: [] },
    });
    client.setQueryData(reviewThreadQuery("rv").queryKey, review);
    const html = render(client, <CronDetailScreen id="cos-review" />);
    expect(html).toContain('id="review-thread"');
    expect(html).toContain("見直しの記録（読み取り専用）");
    expect(html).toContain("変化なし");
    expect(html).toContain("定期見直し: 変わった案件 0 件");
    expect(html.indexOf("変化なし")).toBeLessThan(html.indexOf("変わった案件 0 件"));
    for (const tag of ["<textarea", "<form", 'type="file"', "送信"]) expect(html).not.toContain(tag);
  });

  it("cron_detail_without_a_review_thread_has_no_history_section", () => {
    const client = new QueryClient();
    client.setQueryData(cronDetailQuery("daily").queryKey, {
      job: job({ id: "daily", review_thread_id: null }),
      history: { job_id: "daily", items: [] },
    });
    const html = render(client, <CronDetailScreen id="daily" />);
    expect(html).not.toContain('id="review-thread"');
    expect(html).not.toContain("見直しの記録");
  });

  it("review_thread_messages_say_when_empty", () => {
    expect(renderToStaticMarkup(<ReviewThreadMessages messages={[]} />)).toContain("見直しの記録はまだありません。");
  });

  it("review_thread_query_reads_the_messages_newest_first", async () => {
    const fetcher = vi.fn<typeof fetch>(
      async () => new Response(JSON.stringify({ items: [...review].reverse(), snapshot_event_id: "e" })),
    );
    configureApiClient({ fetcher });
    const items = await reviewThreadQuery("r v").queryFn({ signal: new AbortController().signal });
    expect(fetcher.mock.calls[0]?.[0]).toBe("/api/chat/threads/r%20v/messages?limit=50");
    expect(items.map((item) => item.id)).toEqual(["m2", "m1"]);
  });
});
