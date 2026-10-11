import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { TaskDetail, WorkUnitView } from "../../api/generated/types";
import { OverviewView, taskWaitsFor } from "./overview-view";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: React.ReactNode }) => (
    <a href={to} {...props}>
      {children}
    </a>
  ),
}));

function render(stdout: boolean) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const detail = {
    task: {
      id: "T1",
      title: "Test task",
      status: "done",
      kind: "task",
      attempts: 1,
      budget: { max_retries: 3 },
      created_at: "2026-10-10",
      updated_at: "2026-10-10",
    },
    runs: [
      {
        run_id: "RUN-123456789",
        adapter: "codex",
        model: "test-model",
        started_at: "2026-10-10T00:00:00Z",
        progress: 1,
        artifacts: 0,
        reviewer_deferrals: 0,
        verdicts: 0,
        role: "worker",
        files: { stdout },
      },
    ],
    actions: [],
    dependencies: [],
    dependents: [],
    criteria: [],
    children: [],
  } as unknown as TaskDetail;
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <OverviewView detail={detail} />
    </QueryClientProvider>,
  );
  client.clear();
  return html;
}

describe("task overview run link", () => {
  it("labels a run without stdout as progress", () => {
    const html = render(false);
    expect(html).toContain("進捗を開く");
    expect(html).toContain('href="/tasks/$id/runs/$runId"');
    expect(html).not.toContain("RUN-123456789");
  });

  it("keeps the run id link when stdout is available", () => {
    const html = render(true);
    expect(html).toContain("RUN-123456789");
    expect(html).not.toContain("進捗を開く");
  });
});

describe("task tree waits", () => {
  it("uses server supplied unfinished keys and previous stage", () => {
    const unit = { key: "integrate-edges" } as WorkUnitView;
    expect(
      taskWaitsFor(
        [
          {
            kind: "work_units",
            units: [
              { key: "integrate-edges", stage: "close", waiting_on: ["edges", "remove"], waiting_stage: "edges" },
            ],
          },
        ],
        unit,
      ),
    ).toEqual(["edges", "remove", "段 edges"]);
  });
});
