import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  ProjectRoadmapPanel,
  pendingProposals,
  proposalDiff,
  ROADMAP_DECIDE_ACTION_ID,
  type Roadmap,
  type RoadmapHistory,
  type RoadmapVersion,
  roadmapDecideAction,
} from "./project-roadmap-panel";

const roadmap: Roadmap = {
  project_id: "01P",
  version: 1,
  status: "approved",
  request_sha256: "h",
  change_reason: "最初の方針",
  proposed_by: "cos:t",
  decided_by: "human:web",
  decided_at: "2026-11-04T00:00:00Z",
  outcomes: [
    {
      id: "o1",
      title: "計測が再現する",
      acceptance: ["3 回の反復で同じ値"],
      evidence: ["task:R2"],
      priority: 1,
      depends_on: [],
      status: "reached",
      root_task_ids: ["R1", "R2"],
    },
    {
      id: "o2",
      title: "論文の表",
      acceptance: ["表 2 を出す"],
      evidence: [],
      priority: 2,
      depends_on: ["o1"],
      status: "open",
      root_task_ids: [],
    },
  ],
};

const proposal: RoadmapVersion = {
  project_id: "01P",
  version: 2,
  status: "proposed",
  base_version: 1,
  request_sha256: "h",
  change_reason: "o2 をやめて o3 にする",
  outcomes: [
    {
      id: "o1",
      title: "計測が再現する",
      acceptance: ["3 回の反復で同じ値"],
      priority: 1,
      depends_on: [],
      dropped: false,
    },
    { id: "o2", title: "論文の表", acceptance: ["表 2 を出す"], priority: 2, depends_on: ["o1"], dropped: true },
    { id: "o3", title: "図", acceptance: ["図 3"], priority: 2, depends_on: ["o1"], dropped: false },
  ],
  proposed_by: "cos:t",
  proposed_at: "2026-11-04T00:00:01Z",
  decided_by: null,
  decided_at: null,
  decision_note: null,
  superseded_at: null,
};

const history: RoadmapHistory = {
  versions: [{ ...proposal, version: 1, status: "approved", outcomes: proposal.outcomes.slice(0, 2) }, proposal],
  outcome_events: [],
};

describe("案件の Roadmap", () => {
  it("決定は人の POST …/roadmap/proposals/{version}/decide だけで、milestones を使わない", () => {
    const a = roadmapDecideAction("01P", 2, "approve");
    expect(a).toEqual({
      id: ROADMAP_DECIDE_ACTION_ID,
      method: "POST",
      path: "/api/projects/01P/roadmap/proposals/2/decide",
      body: { decision: "approve", note: "" },
    });
    expect(a.path).not.toContain("milestone");
    expect(roadmapDecideAction("a/b", 3, "reject").path).toBe("/api/projects/a%2Fb/roadmap/proposals/3/decide");
  });

  it("未決定の提案だけを拾い、今の版との差を出す", () => {
    expect(pendingProposals(history).map((v) => v.version)).toEqual([2]);
    expect(pendingProposals(null)).toEqual([]);
    expect(proposalDiff(roadmap, proposal)).toEqual({ added: ["o3"], dropped: ["o2"], changed: [] });
  });

  it("Outcome の状態と担った root を出し、提案には承認・却下を置く", () => {
    const html = renderToStaticMarkup(
      <ProjectRoadmapPanel roadmap={roadmap} history={history} pending={false} onDecide={() => {}} />,
    );
    expect(html).toContain("第 1 版");
    expect(html).toContain('data-outcome="o1" data-status="reached"');
    expect(html).toContain("R1, R2");
    expect(html).toContain('data-proposal="2"');
    expect(html).toContain("承認して版を進める");
    expect(html).toContain("追加 o3");
    expect(html).not.toContain("disabled");
  });

  it("承認済みの版が無ければ block を読むと伝え、送信中は押せない", () => {
    const html = renderToStaticMarkup(
      <ProjectRoadmapPanel roadmap={null} history={history} pending onDecide={() => {}} />,
    );
    expect(html).toContain("承認済みの Roadmap はありません");
    expect(html).toContain("disabled");
  });
});
