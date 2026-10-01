import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ClusterJobWaitView } from "~/celeris/types";
import { ClusterJobWaitBanner, clusterJobWaitLine } from "~/components/ClusterJobWaitBanner";

// celeris ADR-0090 D5（Phase R7-1）: task のページの 1 行「クラスタ job を待っています: 42634 (R) 42635 (Q)」。
const wait: ClusterJobWaitView = {
  wait_id: "01W",
  run_id: "01R",
  cluster: "sirius",
  scheduler: "pbs",
  jobs: [
    { job_id: "42634", state: "running", raw_state: "R" },
    { job_id: "42635", state: "queued", raw_state: "Q" },
  ],
  status_line: "42634 (R) 42635 (Q)",
  poll_secs: 300,
  created_at: "2026-09-30T05:00:00Z",
  deadline: "2026-10-01T05:00:00Z",
  last_polled_at: "2026-09-30T05:10:00Z",
  next_poll_at: "2026-09-30T05:15:00Z",
};

describe("ClusterJobWaitBanner", () => {
  it("shows one line with the job states", () => {
    expect(clusterJobWaitLine(wait)).toBe("クラスタ job を待っています: 42634 (R) 42635 (Q)");
    const html = renderToStaticMarkup(<ClusterJobWaitBanner wait={wait} />);
    expect(html).toContain('data-testid="cluster-job-wait-banner"');
    expect(html).toContain("クラスタ job を待っています: 42634 (R) 42635 (Q)");
    expect(html).toContain("sirius（pbs）");
    expect(html).toContain("次の確認 2026-09-30T05:15:00Z");
  });

  it("says the next poll is soon before the first poll", () => {
    const html = renderToStaticMarkup(<ClusterJobWaitBanner wait={{ ...wait, next_poll_at: undefined }} />);
    expect(html).toContain("次の確認はまもなく");
  });
});
