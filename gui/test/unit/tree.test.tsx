import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { describe, expect, it } from "vitest";
import type { ExecutionView, ProjectTaskView, TaskTreeNode, TaskTreeView, TimelineItem } from "~/celeris/types";
import { ProjectRootTasks } from "~/components/ProjectRootTasks";
import { TaskHoldBanner } from "~/components/TaskHoldBanner";
import { TaskTreeTab } from "~/components/TaskTreeTab";
import {
  byStatusText,
  currentStallFromTimeline,
  decisionBreadcrumb,
  decisionChoices,
  integrationTargetText,
  limitUsageRows,
  neededBeforeText,
  planApprovalReasonLines,
  planGateActions,
  progressText,
  projectRootTasks,
  rollupCostText,
  runsByRoleText,
  STALL_HEADLINE,
  stallText,
  taskHold,
  treeNodeHold,
  treeRows,
  unitsByStage,
} from "~/lib/tree";
import treeFixture from "../fixtures/api/task-tree.json";

/**
 * celeris ADR-0079 D14 / §7 R4b (a): `~/lib/tree.ts` の表示の判定（止まっている理由の文言・パンくず・押せるボタンの
 * 出し分け）と、「木」タブ・止まっている理由の帯・案件の root task 一覧の描画。fixture は R4a の API の形
 * （`test/fixtures/api/task-tree.json`: root → 子 2〈決定待ち・理由なしの停止〉→ 孫 1）。
 */

const view = treeFixture as unknown as TaskTreeView;
const ROOT = view.root_id;
const [rootNode, heldChild, grandchild, stalledChild] = view.nodes;

function render(el: ReactElement): string {
  const router = createMemoryRouter([{ path: "/", element: el }], { initialEntries: ["/"] });
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

function node(overrides: Partial<TaskTreeNode>): TaskTreeNode {
  return { ...rootNode, ...overrides } as TaskTreeNode;
}

describe("止まっている理由（D10）", () => {
  it("stall は最優先で「理由なく止まっています」と分類の語を出す", () => {
    expect(stallText({ reason: "child_missing", detail: "x" })).toBe(
      "理由なく止まっています（待っている子 task が見つからない）",
    );
    expect(stallText({ reason: "", detail: "x" })).toBe(STALL_HEADLINE);
    expect(stallText({ reason: "new_reason", detail: "x" })).toBe("理由なく止まっています（new_reason）");
    const hold = treeNodeHold(stalledChild);
    expect(hold?.kind).toBe("stall");
    expect(hold && hold.kind === "stall" ? hold.detail : "").toContain("子が見つからない");
  });

  it("名指しの待ち（決定・基盤・承認）を出し、子待ち・実行中・終端は出さない", () => {
    expect(treeNodeHold(heldChild)?.kind).toBe("decision");
    expect(treeNodeHold(node({ phase: "blocked_infra" }))?.kind).toBe("infra");
    expect(treeNodeHold(node({ phase: "awaiting_plan_approval" }))?.kind).toBe("plan_approval");
    expect(treeNodeHold(rootNode)).toBeNull();
    expect(treeNodeHold(grandchild)).toBeNull();
    expect(treeNodeHold({ ...stalledChild, status: "cancelled" })).toBeNull();
  });

  it("タスク詳細では最後の event が stall_detected のときだけ stall（何か起きれば消える）", () => {
    const stall: TimelineItem = {
      kind: "event",
      at: "2026-09-29T02:10:00Z",
      seq: 9,
      event: { type: "stall_detected", task_id: ROOT, detail: "d", reason: "nothing_runnable", since: "s" },
    };
    const later: TimelineItem = {
      kind: "event",
      at: "2026-09-29T02:20:00Z",
      seq: 10,
      event: { type: "answered", question: "q", answer: "a" },
    };
    expect(currentStallFromTimeline([stall], "ready")).toEqual({ reason: "nothing_runnable", since: "s", detail: "d" });
    expect(currentStallFromTimeline([stall, later], "ready")).toBeNull();
    expect(currentStallFromTimeline([stall], "done")).toBeNull();
    expect(taskHold("ready", null, { reason: "", detail: "d" })?.text).toBe(STALL_HEADLINE);
  });

  it("タスク詳細の名指しの待ちは承認 → 基盤 → 決定の順（celeris の node_phase と同じ優先）", () => {
    const exec = (units: { status: string; blocked_reason?: string }[], planApproval = false) =>
      ({
        metrics: {},
        plan_approval: planApproval ? { plan_id: "p", reasons: [], summary: "s" } : null,
        plan: { work_units: units },
      }) as unknown as ExecutionView;
    expect(taskHold("blocked", exec([], true), null)?.kind).toBe("plan_approval");
    expect(
      taskHold(
        "ready",
        exec([
          { status: "blocked", blocked_reason: "decision" },
          { status: "blocked", blocked_reason: "infra" },
        ]),
        null,
      )?.kind,
    ).toBe("infra");
    expect(taskHold("ready", exec([{ status: "blocked", blocked_reason: "decision" }]), null)?.kind).toBe("decision");
    expect(taskHold("ready", exec([{ status: "running" }]), null)).toBeNull();
    expect(taskHold("failed", exec([{ status: "blocked", blocked_reason: "infra" }]), null)).toBeNull();
  });
});

describe("決定のパンくず・選択肢（D7 / D14）", () => {
  it("パンくずは root の題名から段階・子・unit を › でつなぐ（同じ語は 1 つ）", () => {
    expect(
      decisionBreadcrumb([
        { task_id: "r", title: "browser capability", stage: "Phase 2" },
        { task_id: "c", title: "broker / provider 1 種", unit: "P2-B" },
      ]),
    ).toBe("browser capability › Phase 2 › broker / provider 1 種 › P2-B");
    expect(
      decisionBreadcrumb([
        { task_id: "r", title: "root", stage: "s1" },
        { task_id: "r", title: "root", unit: "u1" },
      ]),
    ).toBe("root › s1 › root › u1");
    expect(decisionBreadcrumb([])).toBe("");
  });

  it("推奨に印を付け、自由記述は kind = choice だけ（daemon の決定は option 必須）", () => {
    const base = {
      options: [
        { key: "a", label: "A" },
        { key: "b", label: "B", consequence: "後で困る" },
      ],
      recommended: "b",
    };
    const choice = decisionChoices({ ...base, kind: "choice" });
    expect(choice.map((c) => [c.key, c.recommended])).toEqual([
      ["a", false],
      ["b", true],
      ["", false],
    ]);
    expect(choice[1].consequence).toBe("後で困る");
    expect(decisionChoices({ ...base, kind: "limit" }).map((c) => c.key)).toEqual(["a", "b"]);
    expect(decisionChoices({ ...base, kind: "plan_invalid" }).some((c) => c.key === "")).toBe(false);
  });

  it("待っているもの", () => {
    expect(neededBeforeText(["self", "p2-b", "stage:phase-2"])).toBe("この task 自身、unit p2-b、段階 phase-2");
    expect(neededBeforeText([])).toContain("待たずに進みます");
  });
});

describe("計画の承認（D8）", () => {
  it("3 つのボタンは celeris が actions に plan_gate を入れたときだけ", () => {
    expect(planGateActions(["cancel", "plan_gate"])).toEqual(["approve", "replan", "withdraw"]);
    expect(planGateActions(["cancel", "answer"])).toEqual([]);
  });

  it("承認を求めた理由を人の文にする（知らない形はそのまま）", () => {
    expect(
      planApprovalReasonLines(["decisions:h1,h2", "review_human:phase-2", "near_limit:max_tree_leaves:40/48", "x"]),
    ).toEqual([
      "人の決定を含む（h1,h2）",
      "人の確認を挟む段階がある（phase-2）",
      "上限に近い（max_tree_leaves:40/48）",
      "x",
    ]);
  });
});

describe("roll-up と上限の使用（D3 / D11）", () => {
  it("role ごとの run（reviewer を含む）・定価（不完全の明示）・進み", () => {
    expect(runsByRoleText(view.totals)).toBe("planner 2 / worker 3 / reviewer 1");
    expect(runsByRoleText({ runs_by_role: {} })).toBe("run なし");
    expect(rollupCostText(view.totals)).toBe("$1.00 以上（単価不明のモデルあり）");
    expect(rollupCostText(rootNode.own)).toBe("$0.50");
    expect(progressText(view.totals)).toBe("leaf 2/4・子 task 0/3");
    expect(progressText(grandchild.own)).toBe("leaf 1/1");
  });

  it("上限の使用は 0.8 以上に印", () => {
    const rows = limitUsageRows(view.limits ?? ({} as never));
    expect(rows.find((r) => r.key === "leaves")).toMatchObject({ used: 4, max: 5, near: true });
    expect(rows.find((r) => r.key === "runs")).toMatchObject({ used: 6, max: 40, near: false });
    expect(rows.find((r) => r.key === "tokens")).toMatchObject({ max: null, near: false });
  });

  it("前順のまま字下げの段数を付け、unit は段階ごとに統合を除いてまとめる", () => {
    expect(treeRows(view).map((r) => r.indent)).toEqual([0, 1, 2, 1]);
    expect(unitsByStage(rootNode).map((g) => [g.stage, g.units.map((u) => u.key)])).toEqual([
      ["phase-1", ["p1-a"]],
      ["phase-2", ["p2-b", "p2-c"]],
    ]);
  });
});

describe("成果の取り込み（D6 の語）", () => {
  it("root は main へ、子は親の段階へ、木の無い task は出さない", () => {
    expect(integrationTargetText({ depth: 1 }, null)).toBe("成果の取り込み（main へ）");
    expect(integrationTargetText({ depth: 2, parent_unit: { stage: "phase-2" } }, "browser capability")).toBe(
      "成果の取り込み（親『browser capability』の段階『phase-2』へ）",
    );
    expect(integrationTargetText(null, null)).toBeNull();
  });
});

describe("TaskTreeTab（「木」タブ）", () => {
  it("節点を深さで字下げし、状態・導出値・決定・止まっている理由・roll-up・子へのリンクを出す", () => {
    const html = render(
      createElement(TaskTreeTab, {
        view,
        error: null,
        currentId: heldChild.id,
        tree: {
          depth: 2,
          root_id: ROOT,
          parent_unit: { plan_id: "p", stage: "phase-2", task_id: ROOT, unit_key: "p2-b" },
        },
      }),
    );
    expect(html.match(/data-testid="task-tree-node"/g)).toHaveLength(4);
    expect(html).toContain('data-depth="3"');
    expect(html).toContain('data-current="true"');
    expect(html).toContain("broker / provider 1 種（この task）");
    expect(html).toContain("子 task の完了待ち");
    expect(html).toContain("人の決定待ち");
    expect(html).toContain("未回答の決定 1");
    expect(html).toContain('data-hold-kind="decision"');
    expect(html).toContain('data-hold-kind="stall"');
    expect(html).toContain("理由なく止まっています（待っている子 task が見つからない）");
    expect(html).toContain(`href="/tasks/${ROOT}?tab=tree"`);
    expect(html).toContain(`href="/tasks/${grandchild.id}?tab=tree"`);
    expect(html).toContain("planner 2 / worker 3 / reviewer 1");
    expect(html).toContain("成果の取り込み（親『browser capability』の段階『phase-2』へ）");
    // root を含む view なので木の上限の使用を出す（leaf 4/5 は 8 割以上）。
    expect(html).toContain('data-testid="task-tree-limits"');
    expect(html).toContain("leaf: 4 / 5（上限の 8 割以上）");
    // 統合 unit は折りたたみの一覧に出さない。
    expect(html).not.toContain("integrate-phase-2");
  });

  it("1 節点の木（木の無い task）と、取得に失敗したとき", () => {
    const single: TaskTreeView = {
      ...view,
      tree_enabled: false,
      limits: null,
      nodes: [{ ...grandchild, parent_id: null }],
      totals: grandchild.own,
    };
    const html = render(
      createElement(TaskTreeTab, { view: single, error: null, currentId: grandchild.id, tree: null }),
    );
    expect(html).toContain("1 節点の木");
    expect(html).toContain('data-testid="task-tree-disabled"');
    expect(html).not.toContain("成果の取り込み");
    const failed = render(
      createElement(TaskTreeTab, {
        view: null,
        error: {
          status: 404,
          code: "task_not_found",
          detail: "no such task",
          conflict: false,
          fields: {},
          messages: [],
        },
        currentId: "x",
        tree: null,
      }),
    );
    expect(failed).toContain('data-testid="task-tree-unavailable"');
    expect(failed).toContain("no such task");
  });
});

describe("TaskHoldBanner（タスク詳細の止まっている理由）", () => {
  it("stall の帯と木へのリンク。止まっていなければ何も出さない", () => {
    const items: TimelineItem[] = [
      {
        kind: "event",
        at: "t",
        seq: 1,
        event: {
          type: "stall_detected",
          task_id: "T",
          detail: "計画 run の後に何も起きない",
          reason: "nothing_runnable",
        },
      },
    ];
    const html = render(
      createElement(TaskHoldBanner, { taskId: "T", status: "ready", execution: null, timeline: items }),
    );
    expect(html).toContain('data-hold-kind="stall"');
    expect(html).toContain("理由なく止まっています（走れる unit も名指しの待ちも無い）");
    expect(html).toContain("計画 run の後に何も起きない");
    expect(html).toContain('href="/tasks/T?tab=tree"');
    expect(render(createElement(TaskHoldBanner, { taskId: "T", status: "ready", execution: null, timeline: [] }))).toBe(
      "",
    );
  });

  it("決定待ちは受信箱へ、承認待ちは「実行の形」へ", () => {
    const decision = {
      metrics: {},
      plan: { work_units: [{ status: "blocked", blocked_reason: "decision" }] },
    } as unknown as ExecutionView;
    const html = render(
      createElement(TaskHoldBanner, { taskId: "T", status: "ready", execution: decision, timeline: [] }),
    );
    expect(html).toContain('data-testid="task-hold-inbox-link"');
    const approval = {
      metrics: {},
      plan_approval: { plan_id: "p", reasons: [], summary: "s" },
    } as unknown as ExecutionView;
    const html2 = render(
      createElement(TaskHoldBanner, { taskId: "T", status: "blocked", execution: approval, timeline: [] }),
    );
    expect(html2).toContain('href="/tasks/T?tab=overview#execution-mode"');
  });
});

describe("案件ページの root task 一覧（D13 / D14）", () => {
  const tasks: ProjectTaskView[] = [
    { id: ROOT, title: "browser capability", status: "ready", depends_on: [], conversation: false },
    { id: heldChild.id, title: "child", status: "ready", depends_on: [], conversation: false, parent_id: ROOT },
    { id: "conv", title: "対話", status: "running", depends_on: [], conversation: true },
    { id: "sup", title: "報告のまとめ", status: "done", depends_on: [], conversation: false, support: "report" },
    { id: "plain", title: "旧いタスク", status: "done", depends_on: [], conversation: false },
  ];

  it("root task = 親の無い・対話でも裏方でもない task", () => {
    expect(projectRootTasks(tasks).map((t) => t.id)).toEqual([ROOT, "plain"]);
    expect(byStatusText({ ready: 1, done: 2, failed: 0 })).toBe("ready 1・done 2");
    expect(byStatusText(null)).toBe("—");
  });

  it("状態・導出値・未回答の決定・subtree の roll-up と案件の合計を出す", () => {
    const html = render(
      createElement(ProjectRootTasks, {
        roots: projectRootTasks(tasks),
        trees: { [ROOT]: view, plain: null },
        totals: { root_tasks: 2, by_status: { ready: 1, done: 1 }, totals: view.totals },
      }),
    );
    expect(html.match(/data-testid="root-task-row"/g)).toHaveLength(2);
    expect(html).toContain("2 件（ready 1・done 1）");
    expect(html).toContain("子 task の完了待ち");
    expect(html).toContain("未回答の決定 1");
    expect(html).toContain("子 task を含む 4 節点");
    expect(html).toContain(`href="/tasks/${ROOT}?tab=tree"`);
    expect(html).toContain("leaf 2/4・子 task 0/3");
    expect(html).toContain("roll-up は木のタブで見られます。");
  });
});
