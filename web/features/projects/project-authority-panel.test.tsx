import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  AUTHORITY_DISMISS_ACTION_ID,
  AUTHORITY_GRANT_ACTION_ID,
  type AuthorityProposalView,
  type AuthorityView,
  dismissAction,
  grantFromProposalAction,
  ProjectAuthorityPanel,
  revokeAction,
} from "./project-authority-panel";

const proposal: AuthorityProposalView = {
  id: "p1",
  project_id: "01ABC",
  question_key: "promote-this-week",
  scope: { kind: "project", ref: "01ABC" },
  action_class: "production",
  source: { kind: "human_message", ref: "m1" },
  expires_at: "2026-11-11T00:00:00Z",
  budget: 3,
  note: "今週 3 回まで promote する",
  proposed_by: "cos:t1",
  state: "open",
};

const view: AuthorityView = {
  grants: [
    {
      id: "g1",
      project_id: "01ABC",
      scope: { kind: "project", ref: "01ABC" },
      action_class: "launch_root",
      source: { kind: "human_operation", ref: "project.controller_put:01ABC:2" },
      granted_by: "human:web",
      note: "pilot",
      expires_at: "2026-12-01T00:00:00Z",
      budget: 10,
      consumed: 4,
      state: "live",
    },
    {
      id: "g2",
      project_id: "01ABC",
      scope: { kind: "project", ref: "01ABC" },
      action_class: "destructive",
      source: { kind: "human_message", ref: "m2" },
      granted_by: "human:web",
      note: "x",
      expires_at: "2026-11-01T00:00:00Z",
      budget: 1,
      consumed: 1,
      state: "grant_budget_exhausted",
      source_excerpt: "repo を消してよい",
    },
  ],
  proposals: [proposal, { ...proposal, id: "p2", state: "dismissed" }],
  audit: [
    {
      id: "a1",
      action_class: "opaque_external_effect",
      allow: true,
      reason: "allowed",
      suggestion: {},
      actual_actor: "cos:t1",
      outcome_state: "unknown",
      created_at: "2026-11-04T00:00:00Z",
    },
  ],
};

describe("許可の panel", () => {
  it("提案の許可は提案の範囲・根拠と人が決めた期限・回数を送る", () => {
    const action = grantFromProposalAction(proposal, "2026-11-10T00:00:00Z", 2);
    expect(action.id).toBe(AUTHORITY_GRANT_ACTION_ID);
    expect(action.method).toBe("POST");
    expect(action.path).toBe("/api/projects/01ABC/authority/grants");
    expect(action.body).toEqual({
      scope: proposal.scope,
      action_class: "production",
      source: { kind: "human_message", ref: "m1" },
      expires_at: "2026-11-10T00:00:00Z",
      budget: 2,
      note: proposal.note,
      proposal_id: "p1",
    });
    expect(dismissAction({ ...proposal, project_id: "a/b" })).toEqual({
      id: AUTHORITY_DISMISS_ACTION_ID,
      method: "POST",
      path: "/api/projects/a%2Fb/authority/proposals/p1/dismiss",
      body: {},
    });
    expect(revokeAction(view.grants[0]).path).toBe("/api/projects/01ABC/authority/grants/g1/revoke");
  });

  // D4: 案件の応答に並ぶ全案件の台帳（"*"）の行は、案件の path では 404 になる。行の台帳（global）へ送る。
  it("全案件に効く行の許可・断る・取り消しは global の台帳へ送り、全案件に効くと示す", () => {
    const global = { ...proposal, id: "p9", project_id: "*", scope: { kind: "global" as const, ref: "*" } };
    expect(grantFromProposalAction(global, "2036-11-10T00:00:00Z", 1000).path).toBe(
      "/api/projects/global/authority/grants",
    );
    expect(dismissAction(global).path).toBe("/api/projects/global/authority/proposals/p9/dismiss");
    const grant = { ...view.grants[0], id: "g9", project_id: "*", action_class: "job_wait_extend" };
    expect(revokeAction(grant).path).toBe("/api/projects/global/authority/grants/g9/revoke");
    const html = renderToStaticMarkup(
      <ProjectAuthorityPanel
        view={{ grants: [grant], proposals: [global], audit: [] }}
        pending={false}
        onAccept={() => {}}
        onDismiss={() => {}}
        onRevoke={() => {}}
      />,
    );
    expect(html).toContain("全案件に効く・job 待ちの上限の延長・有効");
    expect(html).toContain("CoS の提案: 全案件に効く・");
  });

  it("開いた提案・状態・根拠・状態不明の警告を出し、有効な許可だけ取り消せる", () => {
    const html = renderToStaticMarkup(
      <ProjectAuthorityPanel
        view={view}
        pending={false}
        onAccept={() => {}}
        onDismiss={() => {}}
        onRevoke={() => {}}
      />,
    );
    expect(html).toContain("本番の変更（3 回まで）");
    expect(html.match(/許可する/g)?.length).toBe(1);
    expect(html).toContain("残り 6");
    expect(html).toContain("回数を使い切った");
    expect(html).toContain("「repo を消してよい」");
    expect(html.match(/取り消す/g)?.length).toBe(1);
    expect(html).toContain("結果を確かめられていない操作が 1 件");
    const empty = renderToStaticMarkup(
      <ProjectAuthorityPanel
        view={{ grants: [], proposals: [], audit: [] }}
        pending
        onAccept={() => {}}
        onDismiss={() => {}}
        onRevoke={() => {}}
      />,
    );
    expect(empty).toContain("許可はありません");
  });
});
