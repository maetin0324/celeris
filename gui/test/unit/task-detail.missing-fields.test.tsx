// gui-gate-regression/overview-500: main b742ec75 で mobile-audit が
// GET /tasks/01BOARDTASK00000000000001?tab=overview -> 500 を出した回帰の固定。
// 偽 celeris（と ADR-0130 より前の celeris）の詳細には actual_*_write_sets・behind_target が無く、
// WriteSetSection が `items.length` で投げて SSR が 500 になっていた。省略されうる欄が欠けても
// loader は投げず、overview の節は描画できることを確かめる。
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
import { loadTaskDetail } from "~/celeris/task-detail.server";
import type { ExecutionView, IntegrationRepairView, TaskDetail } from "~/celeris/types";
import { IntegrationRepairPanel, integrationRepairLines } from "~/components/IntegrationRepairPanel";
import { TaskExecutionRoute } from "~/components/task-detail/TaskExecutionRoute";
import { WriteSetSection } from "~/components/task-detail/WriteSetSection";
import { type MockCeleris, sendJson, startMockCeleris } from "../mock-celeris/server";

/** ADR-0130 で増えた欄（actual_*_write_sets・behind_target）を持たない、古い形の詳細。 */
const legacyDetail = {
  task: {
    id: "T1",
    kind: "execute",
    status: "running",
    title: "do the thing",
    objective: "do it",
    priority: 5,
    attempts: 1,
    created_at: "2026-09-15T00:00:00Z",
    updated_at: "2026-09-15T00:00:01Z",
    acceptance: [],
    depends_on: [],
    inputs: [],
    worker_hint: { tier: "standard" },
    budget: { max_retries: 3, max_turns: 10, max_wall_secs: 600 },
    workspace: { kind: "local", path: "." },
  },
  priority_label: "P3",
  workspace_dir: "/tmp/ws/T1",
  timers: { now: "2026-09-15T00:00:02Z", consecutive_requeues: 0, consecutive_reviewer_requeues: 0, max_requeues: 3 },
  criteria: [],
  runs: [],
  prior_review: [],
  answers: [],
  latest_question: null,
  approvals: [],
  dependencies: [],
  dependents: [],
  children: [],
  actions: ["cancel"],
  worker_run_hint: null,
  delegated: [],
} as unknown as TaskDetail;

describe("overview: 省略された欄に耐える", () => {
  it("actual_*_write_sets・behind_target が無くても WriteSetSection は描画できる", () => {
    const html = renderToStaticMarkup(<WriteSetSection detail={legacyDetail} />);
    expect(html).toContain('data-testid="write-set-section"');
    expect(html).toContain("指定なし");
    expect(html).toContain("計測されていません");
    expect(html.match(/ありません。/g)).toHaveLength(2);
  });

  it("null の欄・repos 欠落の behind_target でも描画できる", () => {
    const detail = {
      ...legacyDetail,
      actual_run_write_sets: null,
      actual_work_unit_write_sets: null,
      behind_target: null,
      expected_write_paths: null,
    } as unknown as TaskDetail;
    expect(renderToStaticMarkup(<WriteSetSection detail={detail} />)).toContain("計測されていません");
    const withCommits = { ...legacyDetail, behind_target: { behind_target_commits: 2 } } as unknown as TaskDetail;
    expect(renderToStaticMarkup(<WriteSetSection detail={withCommits} />)).toContain("target から遅れ: 2 commits");
  });

  it("write set の paths が欠けていても描画できる", () => {
    const detail = {
      ...legacyDetail,
      actual_run_write_sets: [{ repo_id: "code", owner_id: "r1", status: "complete", recorded_at: "t" }],
    } as unknown as TaskDetail;
    expect(renderToStaticMarkup(<WriteSetSection detail={detail} />)).toContain("actual-write-set-item");
  });

  it("execution.route の reasons が欠けていても TaskExecutionRoute は描画できる", () => {
    const execution = { route: { route: "direct" } } as unknown as ExecutionView;
    expect(renderToStaticMarkup(<TaskExecutionRoute execution={execution} />)).toContain("直行");
    expect(renderToStaticMarkup(<TaskExecutionRoute execution={undefined} />)).toBe("");
  });

  it("integration_repair が欠落・null なら何も出さず、target_sha 欠落でも投げない", () => {
    expect(renderToStaticMarkup(<IntegrationRepairPanel repair={legacyDetail.integration_repair} />)).toBe("");
    expect(renderToStaticMarkup(<IntegrationRepairPanel repair={null} />)).toBe("");
    const partial = { state: "scheduled", attempt: 1, max_attempts: 2 } as unknown as IntegrationRepairView;
    expect(integrationRepairLines(partial)[0]).toContain("1/2");
  });
});

describe("loadTaskDetail: 省略された欄", () => {
  let mock: MockCeleris;
  let client: CelerisClient;

  beforeEach(async () => {
    mock = await startMockCeleris();
    client = new CelerisClient({ baseUrl: mock.baseUrl });
  });

  afterEach(async () => {
    await mock.close();
  });

  it("新しい欄の無い詳細でも投げずに返す", async () => {
    mock.on("GET", "/api/v1/tasks/T1", (_req, res) => sendJson(res, 200, legacyDetail));
    mock.on("GET", "/api/v1/tasks/T1/events", (_req, res) => sendJson(res, 200, { has_more: false, items: [] }));
    mock.on("GET", "/api/v1/tasks/T1/artifacts", (_req, res) => sendJson(res, 200, { items: [] }));
    mock.on("GET", "/api/v1/tasks/T1/timeline", (_req, res) => sendJson(res, 200, { task_id: "T1", items: [] }));
    mock.on("GET", "/api/v1/tasks/T1/comments", (_req, res) => sendJson(res, 200, { items: [] }));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] }));

    const result = await loadTaskDetail(client, "T1", new Request("http://gui.invalid/tasks/T1?tab=overview"));
    expect(result.detail.task.id).toBe("T1");
    expect(renderToStaticMarkup(<WriteSetSection detail={result.detail} />)).toContain("write-set-section");
  });
});
