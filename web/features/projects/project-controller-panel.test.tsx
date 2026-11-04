import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ProjectBrief, ProjectDetail } from "../../api/generated/types";
import {
  type ControllerDecision,
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
});

function show(state: Partial<ControllerView["state"]> = {}, withBrief = true) {
  return renderToStaticMarkup(
    <ProjectControllerPanel
      detail={detail}
      brief={withBrief ? brief : null}
      view={view(state)}
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
