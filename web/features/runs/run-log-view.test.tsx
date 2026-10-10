import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EventsPage, RunSummary, TaskDetail } from "../../api/generated/types";
import { taskDetailQueryKey } from "../tasks/task-detail-query";
import { RunLogScreen } from "./run-log-view";

const { useRunLog } = vi.hoisted(() => ({ useRunLog: vi.fn() }));
vi.mock("./use-run-log", () => ({ useRunLog }));
vi.mock("@tanstack/react-router", () => ({
  Link: ({ to, children, ...props }: { to: string; children: React.ReactNode }) => (
    <a href={to} {...props}>{children}</a>
  ),
}));

const run = (stdout: boolean, extra: Partial<RunSummary> = {}): RunSummary => ({
  run_id: "R1", adapter: "codex", model: "test-model", started_at: "2026-10-10T00:00:00Z",
  progress: 1, artifacts: 0, reviewer_deferrals: 0, verdicts: 0, role: "worker", finished_at: "2026-10-10T00:00:02Z",
  files: { stdout: stdout, result: true }, ...extra,
});
const detail = (item: RunSummary) => ({
  task: { id: "T1", status: "done" }, runs: [item], actions: [], dependencies: [], dependents: [], criteria: [],
}) as unknown as TaskDetail;

function render(item: RunSummary, data: { events?: EventsPage; result?: { summary?: string; question?: string } } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(taskDetailQueryKey("T1"), detail(item));
  client.setQueryData(["tasks", "run-events-view", "T1", "R1"], data.events);
  client.setQueryData(["tasks", "run-result-view", "T1", "R1"], data.result);
  const html = renderToStaticMarkup(
    <QueryClientProvider client={client}><RunLogScreen taskId="T1" runId="R1" /></QueryClientProvider>,
  );
  client.clear();
  return html;
}

beforeEach(() => {
  useRunLog.mockReturnValue({
    buffer: { lines: ['{"type":"assistant","message":{"content":[{"type":"text","text":"stdout body"}]}}'], dropped: 0, offset: 20 },
    status: "ready", following: false, capped: false, retry: () => {},
  });
});
afterEach(() => { useRunLog.mockReset(); });

describe("RunLogScreen stdout availability", () => {
  it("does not enable or fetch raw stdout when files.stdout: false, and shows protected progress and result", () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const progress = {
      items: [{ id: 1, seq: 1, task_id: "T1", ts: "2026-10-10T00:00:01Z", event: {
        type: "worker_progress", kind: "tool", tool: "browser", summary: "ページを確認しました",
      } }], has_more: false,
    } as unknown as EventsPage;
    const html = render(run(false, { files: { stdout: false, result: true } }), {
      events: progress,
      result: { summary: "処理が完了しました", question: "次に進めますか？" },
    });

    expect(useRunLog).toHaveBeenCalledWith("T1", "R1", false, false);
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(html).toContain("生ログは保存されていません");
    expect(html).toContain("機密保護のため生ログ（harness の stdout/stderr）を保存しません");
    expect(html).toContain("ページを確認しました");
    expect(html).toContain("処理が完了しました");
    expect(html).toContain("次に進めますか？");
    expect(html).not.toContain('data-testid="run-log"');
    fetchSpy.mockRestore();
  });

  it("keeps the stdout log path and rendered output when files.stdout: true", () => {
    const html = render(run(true, { files: { stdout: true, result: true } }));
    expect(useRunLog).toHaveBeenCalledWith("T1", "R1", false, true);
    expect(html).toContain('data-testid="run-log"');
    expect(html).toContain("stdout body");
    expect(html).not.toContain("生ログは保存されていません");
  });
});
