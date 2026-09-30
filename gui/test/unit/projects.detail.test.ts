import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { CelerisClient } from "~/celeris/client.server";
// celeris ADR-0079 D13（Phase R5a）: 案件計画と途中目標の書き込みの中継（`createMilestone` / `patchMilestoneStatus` /
// `decideMilestone` / `startProjectPlan`）は外した（celeris が 410）。
import { patchProjectStatus, patchProjectText } from "~/celeris/projects-admin.server";
import type {
  ArtifactList,
  MilestoneView,
  OrgList,
  Project,
  ProjectDetail,
  Report,
  ReportList,
  TaskDetail,
} from "~/celeris/types";
import {
  FROZEN_MILESTONES_PARAM,
  frozenMilestonesOpenCount,
  loadProjectDetail,
  wantsFrozenMilestones,
} from "~/routes/projects.$id";
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => {
  await mock.close();
});

const project = (over: Partial<Project> = {}): Project => ({
  id: "p1",
  title: "Pluvio",
  request: "Pluvio を基盤に用いた新たな研究テーマの模索、検証",
  status: "active",
  created_at: "2026-09-17T00:00:00Z",
  updated_at: "2026-09-17T00:00:00Z",
  ...over,
});

describe("loadProjectDetail", () => {
  it("案件・途中目標・仕事の木（tasks）と組織を束ねて返す", async () => {
    const detail: ProjectDetail = {
      project: project(),
      milestones: [
        {
          id: "m1",
          project_id: "p1",
          seq: 1,
          title: "調査",
          description: "",
          status: "approved",
          created_at: "…",
          updated_at: "…",
        },
      ],
      tasks: [
        {
          id: "t1",
          title: "survey",
          status: "running",
          parent_id: null,
          depends_on: [],
          assignee: "research-survey",
          milestone_id: "m1",
          conversation: false,
        },
      ],
    };
    const org: OrgList = {
      items: [
        {
          id: "research-survey",
          parent_id: "research",
          name: "関連研究調査課",
          kind: "section",
          position: 0,
          created_at: "…",
          updated_at: "…",
        },
      ],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, org));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.detail).toEqual(detail);
    expect(result.org).toEqual(org);
  });

  it("ADR-0038（Phase 41 / G13j）: milestones[] の review / proposal をそのまま通す", async () => {
    const milestone: MilestoneView = {
      id: "m1",
      project_id: "p1",
      seq: 1,
      title: "隣接領域の動向調査",
      description: "",
      status: "in_progress",
      created_at: "…",
      updated_at: "…",
      review: { message_id: "msg1", text: "候補を 3 本に絞りました。次は…", at: "2026-09-18T00:00:00Z" },
      proposal: {
        id: "m2",
        project_id: "p1",
        seq: 2,
        title: "候補の比較実験",
        description: "3 本を比較する",
        status: "proposed",
        created_at: "…",
        updated_at: "…",
      },
    };
    const detail: ProjectDetail = { project: project(), milestones: [milestone], tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.detail.milestones[0].review).toEqual(milestone.review);
    expect(result.detail.milestones[0].proposal).toEqual(milestone.proposal);
  });

  it("celeris ADR-0079 D14（Phase R4b）: root task だけ木（task-tree）を引き、失敗した root は null", async () => {
    const detail: ProjectDetail = {
      project: project(),
      milestones: [],
      tasks: [
        { id: "r1", title: "root 1", status: "ready", depends_on: [], conversation: false },
        { id: "r2", title: "root 2", status: "done", depends_on: [], conversation: false },
        { id: "c1", title: "child", status: "ready", depends_on: [], conversation: false, parent_id: "r1" },
        { id: "talk", title: "対話", status: "running", depends_on: [], conversation: true },
      ],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    const tree = { root_id: "r1", subtree_root: "r1", tree_enabled: true, nodes: [], totals: {} };
    mock.on("GET", "/api/v1/tasks/r1/task-tree", (_req, res) => sendJson(res, 200, tree));
    mock.on("GET", "/api/v1/tasks/r2/task-tree", (_req, res) =>
      sendProblem(res, { status: 404, code: "task_not_found", detail: "gone" }),
    );

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.rootTrees).toEqual({ r1: tree, r2: null });
    const treeCalls = mock.requests.filter((r) => r.url.includes("/task-tree")).map((r) => r.url);
    expect(treeCalls.sort()).toEqual(["/api/v1/tasks/r1/task-tree", "/api/v1/tasks/r2/task-tree"]);
  });

  it("GET /org が失敗しても案件の詳細は返す（組織は空扱い）", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [], tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendProblem(res, { status: 500, code: "internal", detail: "boom" }));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.detail).toEqual(detail);
    expect(result.org).toEqual({ items: [] });
  });

  it("GET /reports?project=<id> を呼び、この案件のすべての段の報告を返す（level を付けない。SPEC §4「報告の流れ」タブ）", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [], tasks: [] };
    const reportsResponse: ReportList = {
      items: [
        {
          id: "r1",
          kind: "proposal",
          level: 1,
          node_id: "research-survey",
          headline: "この framing で論文が書けそう",
          created_at: "…",
        } satisfies Report,
      ],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    mock.on("GET", "/api/v1/reports", (_req, res) => sendJson(res, 200, reportsResponse));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.reports).toEqual(reportsResponse);
    expect(typeof result.fetchedAt).toBe("string");
    const req = mock.requests.find((r) => r.url.startsWith("/api/v1/reports"));
    const url = new URL(req?.url ?? "", "http://mock-celeris.invalid");
    expect(url.searchParams.get("project")).toBe("p1");
    expect(url.searchParams.has("level")).toBe(false);
    expect(url.searchParams.has("unread")).toBe(false);
  });

  it("GET /reports が失敗しても案件の詳細は返す（報告は空扱い）", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [], tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    mock.on("GET", "/api/v1/reports", (_req, res) =>
      sendProblem(res, { status: 500, code: "internal", detail: "boom" }),
    );

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));
    expect(result.reports).toEqual({ items: [] });
  });

  it("GET /projects/{id} の tasks ぶん GET /tasks/{id} と GET /tasks/{id}/artifacts を束ね、成果物一覧（artifactRows）を組む（Phase G13c）", async () => {
    const detail: ProjectDetail = {
      project: project(),
      milestones: [],
      tasks: [
        {
          id: "t1",
          title: "survey",
          status: "done",
          parent_id: null,
          depends_on: [],
          assignee: "research-survey",
          conversation: false,
        },
      ],
    };
    const org: OrgList = {
      items: [
        {
          id: "research-survey",
          parent_id: "research",
          name: "関連研究調査課",
          kind: "section",
          position: 0,
          created_at: "…",
          updated_at: "…",
        },
      ],
    };
    const taskDetail: TaskDetail = {
      task: {
        id: "t1",
        kind: "execute",
        status: "done",
        title: "survey",
        objective: "survey",
        priority: 0,
        attempts: 1,
        created_at: "…",
        updated_at: "…",
        acceptance: [],
        depends_on: [],
        inputs: [],
        worker_hint: { tier: "standard" },
        budget: { max_retries: 3, max_turns: 10, max_wall_secs: 600 },
        workspace: { kind: "local", path: "lab/pluvio-survey" },
        assignee: "research-survey",
      },
      // ADR-0044 D3（Phase 53）で `TaskDetail` に増えた必須項目。
      priority_label: "P3",
      workspace_dir: "/home/user/workspace/lab/pluvio-survey",
      timers: { now: "…", consecutive_requeues: 0, consecutive_reviewer_requeues: 0, max_requeues: 3 },
      criteria: [],
      runs: [],
      prior_review: [],
      answers: [],
      latest_question: null,
      approvals: [],
      dependencies: [],
      dependents: [],
      children: [],
      actions: [],
      worker_run_hint: null,
      delegated: [],
    };
    const artifacts: ArtifactList = {
      items: [
        {
          idx: 0,
          run_id: "run1",
          ts: "2026-09-17T00:00:00Z",
          artifact: { name: "report.md", path: "artifacts/report.md", sha256: "abc", kind: "markdown" },
          exists: true,
          forbidden: false,
          size: 10,
          sha256_current: "abc",
          sha256_matches: true,
        },
      ],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, org));
    mock.on("GET", "/api/v1/reports", (_req, res) => sendJson(res, 200, { items: [] } satisfies ReportList));
    mock.on("GET", "/api/v1/tasks/t1", (_req, res) => sendJson(res, 200, taskDetail));
    mock.on("GET", "/api/v1/tasks/t1/artifacts", (_req, res) => sendJson(res, 200, artifacts));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.artifactRows).toHaveLength(1);
    expect(result.artifactRows[0]).toMatchObject({
      taskId: "t1",
      taskTitle: "survey",
      assigneeName: "関連研究調査課",
      workspace: {
        text: "/home/user/workspace/lab/pluvio-survey",
        vscodeHref: "vscode://file/home/user/workspace/lab/pluvio-survey",
      },
    });
    expect(result.artifactRows[0].artifact).toEqual(artifacts.items[0]);
  });

  // ADR-0039 D1（Phase G13k）: 案件の作業場所は loader を素通りする（GUI 側で新しい判断はしない）。
  it("project.workspace をそのまま通し、GET /clusters を編集フォームの選択肢として添える", async () => {
    const detail: ProjectDetail = {
      project: project({ workspace: { kind: "remote", cluster: "pegasus", path: "/work/NBB/rmaeda/benchfs" } }),
      milestones: [],
      tasks: [],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    mock.on("GET", "/api/v1/reports", (_req, res) => sendJson(res, 200, { items: [] } satisfies ReportList));
    mock.on("GET", "/api/v1/clusters", (_req, res) =>
      sendJson(res, 200, {
        items: [
          {
            id: "pegasus",
            host: "pegasus",
            concurrency: 1,
            delete_on_push: false,
            env_keys: [],
            has_setup: false,
            rsync_excludes: [],
            sync: "rsync",
          },
        ],
      }),
    );

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.detail.project.workspace).toEqual({
      kind: "remote",
      cluster: "pegasus",
      path: "/work/NBB/rmaeda/benchfs",
    });
    expect(result.clusters).toHaveLength(1);
    expect(result.clusters[0].id).toBe("pegasus");
  });

  it("GET /clusters が落ちても案件の詳細は返す（選択肢は空扱い）", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [], tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    mock.on("GET", "/api/v1/reports", (_req, res) => sendJson(res, 200, { items: [] } satisfies ReportList));
    mock.on("GET", "/api/v1/clusters", (_req, res) =>
      sendProblem(res, { status: 500, code: "internal", detail: "boom" }),
    );

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));
    expect(result.clusters).toEqual([]);
  });

  /**
   * 中止・一時停止・アーカイブ（ADR-0044 D6、Phase 55 / G19）。loader は素通りさせるだけ
   * （`archived_at` / `paused_from` / `paused` / `cancelled` を GUI 側で計算し直さない）。
   * `GET /projects/{id}`（個別）はアーカイブ済みでもそのまま見えるので、隠す判断もしない。
   */
  it("archived_at / paused_from と paused / cancelled の途中目標をそのまま通す", async () => {
    const detail: ProjectDetail = {
      project: project({ status: "paused", paused_from: "active", archived_at: "2026-09-19T12:00:00Z" }),
      milestones: [
        {
          id: "m1",
          project_id: "p1",
          seq: 1,
          title: "調査",
          description: "",
          status: "paused",
          paused_from: "in_progress",
          created_at: "…",
          updated_at: "…",
        },
        {
          id: "m2",
          project_id: "p1",
          seq: 2,
          title: "統合",
          description: "",
          status: "cancelled",
          created_at: "…",
          updated_at: "…",
        },
      ],
      tasks: [],
    };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));

    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));

    expect(result.detail.project.status).toBe("paused");
    expect(result.detail.project.paused_from).toBe("active");
    expect(result.detail.project.archived_at).toBe("2026-09-19T12:00:00Z");
    expect(result.detail.milestones.map((m) => m.status)).toEqual(["paused", "cancelled"]);
    expect(result.detail.milestones[0].paused_from).toBe("in_progress");
  });

  it("404 project_not_found は例外として投げる（loader が Response に変換する）", async () => {
    mock.on("GET", "/api/v1/projects/missing", (_req, res) =>
      sendProblem(res, { status: 404, code: "project_not_found", detail: "no such project" }),
    );
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));

    await expect(
      loadProjectDetail(client, "missing", new Request("http://gui.invalid/projects/missing")),
    ).rejects.toBeTruthy();
  });
});

describe("patchProjectStatus (PATCH /projects/{id})", () => {
  it("success", async () => {
    const updated = project({ status: "done" });
    mock.on("PATCH", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, updated));

    const result = await patchProjectStatus(client, "p1", "done");

    expect(result).toEqual({ ok: true, op: "project_status", project: updated });
  });

  it("400 bad_request（知らない値）はそのまま ActionError にする", async () => {
    mock.on("PATCH", "/api/v1/projects/p1", (_req, res) =>
      sendProblem(res, { status: 400, code: "bad_request", detail: "invalid status" }),
    );
    const result = await patchProjectStatus(client, "p1", "done");
    expect(result.ok).toBe(false);
  });
});

/**
 * celeris ADR-0072「Phase F6 実装時の決定」: 案件の名前・説明（依頼文）・slug の編集（`PATCH /projects/{id}`）と、
 * 案件計画を持たない既存の案件から「案件計画を提案させる」（`mode = "milestones"`）。GUI は検証しない。
 */
describe("patchProjectText (PATCH /projects/{id} の title / request / slug)", () => {
  it("名前と説明をそのまま送り、slug は今の値と違うときだけ送る", async () => {
    const bodies: unknown[] = [];
    mock.on("PATCH", "/api/v1/projects/p1", (_req, res, body) => {
      bodies.push(JSON.parse(body));
      sendJson(res, 200, project({ title: "BenchFS paper", request: "新しい説明" }));
    });
    const form = new FormData();
    form.set("title", "BenchFS paper");
    form.set("request", "新しい説明");
    form.set("slug", "benchfs");
    form.set("slug_current", "benchfs");
    const result = await patchProjectText(client, "p1", form);
    expect(result).toMatchObject({ ok: true, op: "project_edit", project: { title: "BenchFS paper" } });

    const renamed = new FormData();
    renamed.set("title", "BenchFS paper");
    renamed.set("request", "新しい説明");
    renamed.set("slug", " benchfs-paper ");
    renamed.set("slug_current", "benchfs");
    await patchProjectText(client, "p1", renamed);
    expect(bodies).toEqual([
      { title: "BenchFS paper", request: "新しい説明" },
      { title: "BenchFS paper", request: "新しい説明", slug: "benchfs-paper" },
    ]);
  });

  it("422 validation（空の名前）は ActionError にして返す", async () => {
    mock.on("PATCH", "/api/v1/projects/p1", (_req, res) =>
      sendProblem(res, {
        status: 422,
        code: "validation",
        detail: "title must not be blank",
      }),
    );
    const form = new FormData();
    form.set("title", " ");
    form.set("request", "r");
    const result = await patchProjectText(client, "p1", form);
    expect(result).toMatchObject({ ok: false, op: "project_edit", error: { status: 422, code: "validation" } });
  });
});

describe("以前の途中目標（読み取り専用）は開いたときだけ include_frozen=true で読む（celeris ADR-0079 R5a / R5b-prep）", () => {
  const frozen: MilestoneView = {
    id: "m1",
    project_id: "p1",
    seq: 1,
    title: "調査",
    description: "",
    status: "approved",
    created_at: "…",
    updated_at: "…",
  };

  it("既定（閉じている）は include_frozen を付けず、件数 milestones_frozen だけを受け取る", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [], milestones_frozen: 3, tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    const result = await loadProjectDetail(client, "p1", new Request("http://gui.invalid/projects/p1"));
    expect(result.detail.milestones_frozen).toBe(3);
    expect(result.detail.milestones).toEqual([]);
    const calls = mock.requests.filter(
      (r) => r.url.startsWith("/api/v1/projects/p1") && !r.url.includes("/integrations"),
    );
    expect(calls.map((r) => r.url)).toEqual(["/api/v1/projects/p1"]);
    expect(wantsFrozenMilestones(new Request("http://gui.invalid/projects/p1"))).toBe(false);
  });

  it("人が開く（?frozen=1）と GET /projects/{id}?include_frozen=true で凍結した行を読む", async () => {
    const detail: ProjectDetail = { project: project(), milestones: [frozen], milestones_frozen: 1, tasks: [] };
    mock.on("GET", "/api/v1/projects/p1", (_req, res) => sendJson(res, 200, detail));
    mock.on("GET", "/api/v1/org", (_req, res) => sendJson(res, 200, { items: [] } satisfies OrgList));
    const request = new Request(`http://gui.invalid/projects/p1?${FROZEN_MILESTONES_PARAM}=1`);
    expect(wantsFrozenMilestones(request)).toBe(true);
    const result = await loadProjectDetail(client, "p1", request);
    expect(result.detail.milestones).toEqual([frozen]);
    const calls = mock.requests.filter(
      (r) => r.url.startsWith("/api/v1/projects/p1") && !r.url.includes("/integrations"),
    );
    expect(calls.map((r) => r.url)).toEqual(["/api/v1/projects/p1?include_frozen=true"]);
    // `frozen` 以外の値では開かない。
    expect(wantsFrozenMilestones(new Request("http://gui.invalid/projects/p1?frozen=0"))).toBe(false);
  });

  it("celeris ADR-0079 R6-4: 終わらないまま凍結した件数は milestones_frozen_open、欄が無ければ読めた行から数える", () => {
    expect(frozenMilestonesOpenCount({ milestones: [], milestones_frozen_open: 7 })).toBe(7);
    expect(
      frozenMilestonesOpenCount({
        milestones: [frozen, { ...frozen, id: "m2", status: "reached" }, { ...frozen, id: "m3", status: "paused" }],
      }),
    ).toBe(2);
    expect(frozenMilestonesOpenCount({ milestones: [] })).toBe(0);
  });
});
