import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ProjectBrief, ProjectDetail } from "../../api/generated/types";
import {
  type ControllerDecision,
  type ControllerSummary,
  type ControllerView,
  controllerModeAction,
  decisionRule,
  ProjectControllerPanel,
  rootTasks,
} from "./project-controller-panel";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: ReactNode }) => <a href="#x">{children}</a>,
}));

const project = {
  id: "01P",
  title: "pilot: 計測",
  request: "BenchFS の計測を長期に回す",
  status: "active",
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
};
const tasks = [
  { id: "R1", title: "最初の計測", status: "done", is_root_task: true },
  { id: "R2", title: "続きの計測", status: "running", is_root_task: true },
  { id: "C1", title: "子の作業", status: "done", parent_id: "R1", is_root_task: false },
];
const detail = { project, tasks } as unknown as ProjectDetail;
const brief = {
  project_id: "01P",
  objective: "計測を再現できるようにする",
  state: "root 1 件完了",
  open_issues: [],
  next_actions: ["表 2 を作る"],
  waiting_on: [{ kind: "human", note: "account の login" }],
  evidence_refs: ["task:R1"],
  updated_at: "2026-11-02T00:00:00Z",
  updated_by: "cos:t1",
} as unknown as ProjectBrief;
const decision: ControllerDecision = {
  id: "D1",
  mode: "enabled",
  proposal_json: JSON.stringify({ explanation: { rule_id: "pc/outcome/next" } }),
  validation: "ok",
  effect_json: "{}",
  effect_state: "applied",
  effect_ref: "task:R2",
  created_at: "2026-11-02T00:00:00Z",
};
const view = (over: Partial<ControllerView["state"]> = {}): ControllerView => ({
  state: {
    project_id: "01P",
    mode: "enabled",
    phase: "waiting",
    revision: 7,
    next_wake_at: "2026-11-03T00:00:00Z",
    updated_at: "2026-11-02T00:00:00Z",
    updated_by: "human:web",
    ...over,
  },
  pending_wakes: [],
  last_decision: decision,
  budget_today: {
    day: "2026-11-02",
    cos_inputs: 1,
    roots: 2,
    tokens: 1500,
    usd: 12.5,
    usd_unknown_runs: 0,
    price_table_version: "2026-09-approx-1",
    chat_starts: 3,
  },
  summary: summary(),
});

const summary = (over: Partial<ControllerSummary> = {}): ControllerSummary => ({
  outcomes: [
    { id: "o1", title: "表 1", source: "request_block", recorded: true, evidence_root_ids: ["R1"], reached: true },
    { id: "o2", title: "表 2", source: "request_block", recorded: false, evidence_root_ids: [], reached: false },
  ],
  outcomes_invalid: false,
  waiting: {
    phase: "waiting",
    kind: "guard",
    rule_id: "pc/guard/budget",
    details: ['guard budget: {"kind":"usd"}', "escalate budget:usd:2026-11-02: 今日の費用の上限"],
    decision_id: "D1",
    decided_at: "2026-11-02T00:00:00Z",
  },
  authority: {
    launch_root_allowed: true,
    launch_root_reason: "allowed",
    grants: [
      {
        id: "G1",
        action_class: "launch_root",
        scope_kind: "project",
        scope_ref: "01P",
        expires_at: "2026-12-01T00:00:00Z",
        budget_remaining: 8,
      },
    ],
  },
  next_wake: { at: "2026-11-03T00:00:00Z", backoff_until: null, pending_kinds: ["external_released"] },
  budget_limits: {
    max_cos_inputs_per_day: 10,
    max_roots_per_day: 5,
    max_no_progress_decisions: 5,
    max_same_failure_repeats: 3,
    max_invalid_decisions: 3,
    max_usd_per_day: 100,
    ttl_secs: 259200,
  },
  retention: {
    wakes_total: 12,
    wakes_pending: 1,
    decisions_total: 30,
    decisions_effect_none: 25,
    decisions_pending: 0,
  },
  trace: {
    decision_id: "D1",
    rule_id: "pc/guard/budget",
    mode: "enabled",
    effect_state: "pending",
    effect_ref: null,
    based_on_revision: 6,
    snapshot_sha256: "abcdef0123456789",
    generation_id: null,
    decided_at: "2026-11-02T00:00:00Z",
  },
  ...over,
});

function show(
  state: Partial<ControllerView["state"]> = {},
  withBrief = true,
  summaryOver: Partial<ControllerSummary> = {},
) {
  return renderToStaticMarkup(
    <ProjectControllerPanel
      detail={detail}
      brief={withBrief ? brief : null}
      view={{ ...view(state), summary: summary(summaryOver) }}
      decisions={[decision, { ...decision, id: "D2", proposal_json: "壊れた JSON", validation: "" }]}
      pendingHumanItems={2}
      openAuthorityProposals={1}
      actionPending={false}
      onMode={() => {}}
    />,
  );
}

/** data-testid の欄の HTML（次の欄の始まりまで）。 */
function part(html: string, testId: string): string {
  const start = html.indexOf(`data-testid="${testId}"`);
  expect(start, testId).toBeGreaterThanOrEqual(0);
  const next = html.indexOf("<section", start);
  return next < 0 ? html.slice(start) : html.slice(start, next);
}

describe("ProjectControllerPanel", () => {
  it("6 つの欄の中身を出す", () => {
    const html = show();
    const policy = part(html, "controller-policy");
    expect(policy).toContain("計測を再現できるようにする");
    expect(policy).toContain("BenchFS の計測を長期に回す");
    const roots = part(html, "controller-roots");
    expect(roots).toContain("最初の計測");
    expect(roots).toContain("続きの計測");
    expect(roots).not.toContain("子の作業");
    const next = part(html, "controller-next");
    expect(next).toContain("表 2 を作る");
    expect(next).toContain("2026-11-03T00:00:00Z");
    expect(next).toContain("CoS の起動 3 回");
    expect(next).toContain("換算費用 12.50");
    const waiting = part(html, "controller-waiting");
    expect(waiting).toContain("account の login");
    expect(waiting).toContain("判断待ち 2 件");
    expect(waiting).toContain("許可の提案 1 件");
    expect(waiting).toContain("人の確認");
    const decisions = part(html, "controller-decisions");
    expect(decisions).toContain("pc/outcome/next");
    expect(decisions).toContain("task:R2");
    expect(decisions).toContain("規則の記録なし");
    expect(decisions).toContain("task:R1");
  });

  it("P2-J: 未達・達成・待機の理由・承認の範囲・次の wake・上限・最後の判断を出す", () => {
    const html = show();
    const outcomes = part(html, "controller-outcomes");
    expect(outcomes).toContain('data-outcome="o1" data-reached="true"');
    expect(outcomes).toContain('data-outcome="o2" data-reached="false"');
    expect(outcomes).toContain("達成");
    expect(outcomes).toContain("未達");
    expect(outcomes).toContain("根拠の root: R1");
    expect(outcomes).toContain("根拠の root なし");
    const waiting = part(html, "controller-waiting");
    expect(waiting).toContain('data-wait-kind="guard"');
    expect(waiting).toContain("規則 pc/guard/budget");
    expect(waiting).toContain("今日の費用の上限");
    expect(waiting).toContain("承認の範囲: 続きの root の起票は許可あり");
    expect(waiting).toContain("続きの root の起票（project 01P）: 残り 8 回、2026-12-01T00:00:00Z まで");
    const next = part(html, "controller-next");
    expect(next).toContain("2026-11-03T00:00:00Z");
    expect(next).toContain("external_released");
    expect(next).toContain("換算費用 12.50 / 100 USD 相当");
    expect(next).toContain("token 1500 は表示のみ");
    expect(next).toContain("起票 2 件 / 5");
    expect(next).toContain("同じ失敗 3 回");
    expect(next).toContain("259200 秒");
    // P4-D: 記録の行数。
    expect(next).toContain("wake 12 件（未処理 1 件）");
    expect(next).toContain("効果なし 25 件");
    const decisions = part(html, "controller-decisions");
    expect(decisions).toContain("最後の判断: pc/guard/budget");
    expect(decisions).toContain("版 6");
    expect(decisions).toContain("snapshot abcdef012345");
  });

  it("P2-J: 待機なし・許可なし・timer なし・block の誤りを出し分ける", () => {
    const html = show({ phase: "active", next_wake_at: null }, true, {
      waiting: null,
      outcomes: [],
      outcomes_invalid: true,
      authority: { launch_root_allowed: false, launch_root_reason: "no_grant", grants: [] },
      next_wake: { at: null, backoff_until: "2026-11-02T01:00:00Z", pending_kinds: [] },
      trace: null,
    });
    expect(html).not.toContain('data-testid="controller-wait-reason"');
    expect(html).toContain('data-testid="controller-outcomes-invalid"');
    expect(part(html, "controller-outcomes")).toContain("途中目標はまだありません");
    expect(part(html, "controller-waiting")).toContain("許可なし（no_grant）");
    expect(part(html, "controller-waiting")).toContain("生きている許可はありません");
    expect(part(html, "controller-next")).toContain("予定なし（きっかけの event を待つ）");
    expect(part(html, "controller-next")).toContain("再試行の待ち: 2026-11-02T01:00:00Z まで");
    expect(html).not.toContain('data-testid="controller-trace"');
  });

  it("blocked は人への案内を出す", () => {
    const html = show({ phase: "blocked" });
    expect(html).toContain('data-testid="controller-blocked"');
    expect(html).toContain('data-phase="blocked"');
  });

  it("off の案件は phase を隠し、要約が無くても壊れない", () => {
    const html = show({ mode: "off", phase: "active" }, false);
    expect(html).not.toContain('data-testid="controller-phase"');
    expect(html).toContain('data-mode="off"');
    expect(part(html, "controller-next")).toContain("次の作業はまだありません");
  });
});

describe("controller の部品", () => {
  it("mode の PUT は版と理由を送る", () => {
    expect(controllerModeAction("01P", "off", 7, "止める")).toEqual({
      id: "project_controller_mode",
      method: "PUT",
      path: "/api/projects/01P/controller",
      body: { mode: "off", expected_revision: 7, reason: "止める" },
    });
  });
  it("規則の id を読み、読めなければ検証の欄、それも無ければ null", () => {
    expect(decisionRule(decision)).toBe("pc/outcome/next");
    expect(decisionRule({ ...decision, proposal_json: JSON.stringify({ rule_id: "pc/old" }) })).toBe("pc/old");
    expect(decisionRule({ ...decision, proposal_json: "x", validation: "pc/script-unavailable" })).toBe(
      "pc/script-unavailable",
    );
    expect(decisionRule({ ...decision, proposal_json: "x", validation: "" })).toBeNull();
  });
  it("rootTasks は子を外す", () => {
    expect(rootTasks(detail.tasks).map((t) => String(t.id))).toEqual(["R1", "R2"]);
  });
});
