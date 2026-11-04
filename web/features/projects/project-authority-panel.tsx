import type { ActionTarget } from "../../components/actions/use-action-result";

// ADR 2026-11-04-project-controller A7: 人の許可範囲（authority ledger）の表示と人の操作。
// GET /api/v1/projects/{id}/authority の応答をそのまま出す。判定は API（Rust）が行い、画面は検証しない。
// 案件の詳細画面への結線は A9 が行う（この file は panel と単体試験まで）。

export type AuthorityScope = { kind: "project" | "outcome" | "task"; ref: string };
export type AuthoritySource = {
  kind: "human_message" | "decision" | "approval" | "standing_rule" | "human_operation";
  ref: string;
};

export type AuthorityGrantView = {
  id: string;
  scope: AuthorityScope;
  action_class: string;
  source: AuthoritySource;
  granted_by: string;
  question_key?: string;
  note: string;
  expires_at: string;
  budget: number;
  consumed: number;
  revoked_at?: string;
  state: string;
  source_excerpt?: string;
};

export type AuthorityProposalView = {
  id: string;
  question_key: string;
  scope: AuthorityScope;
  action_class: string;
  source: AuthoritySource;
  expires_at: string;
  budget: number;
  note: string;
  proposed_by: string;
  state: "open" | "accepted" | "dismissed";
};

export type AuthorityAuditView = {
  id: string;
  action_class: string;
  allow: boolean;
  reason: string;
  suggestion: Record<string, unknown>;
  actual_actor: string;
  human_decision_ref?: string;
  outcome_state: "unknown" | "applied" | "failed" | "refused";
  created_at: string;
};

export type AuthorityView = {
  grants: AuthorityGrantView[];
  proposals: AuthorityProposalView[];
  audit: AuthorityAuditView[];
};

export const AUTHORITY_GRANT_ACTION_ID = "project_authority_grant";
export const AUTHORITY_REVOKE_ACTION_ID = "project_authority_revoke";
export const AUTHORITY_DISMISS_ACTION_ID = "project_authority_dismiss";

const CLASS_LABEL: Record<string, string> = {
  destructive: "取り消せない操作",
  production: "本番の変更",
  api_authorization: "外部 API の認可",
  external_publish: "外部への公開",
  opaque_external_effect: "結果を確かめられない外部の操作",
  launch_root: "続きの root の起票",
};

export function authorityClassLabel(actionClass: string): string {
  return CLASS_LABEL[actionClass] ?? actionClass;
}

function base(projectId: string): string {
  return `/api/projects/${encodeURIComponent(projectId)}/authority`;
}

/** 提案を受けて許可する POST。期限・回数は人が決める（無期限・無制限の許可は API が断る）。 */
export function grantFromProposalAction(
  projectId: string,
  proposal: AuthorityProposalView,
  expiresAt: string,
  budget: number,
): ActionTarget {
  return {
    id: AUTHORITY_GRANT_ACTION_ID,
    method: "POST",
    path: `${base(projectId)}/grants`,
    body: {
      scope: proposal.scope,
      action_class: proposal.action_class,
      source: proposal.source,
      expires_at: expiresAt,
      budget,
      note: proposal.note,
      proposal_id: proposal.id,
    },
  };
}

export function revokeAction(projectId: string, grantId: string): ActionTarget {
  return {
    id: AUTHORITY_REVOKE_ACTION_ID,
    method: "POST",
    path: `${base(projectId)}/grants/${encodeURIComponent(grantId)}/revoke`,
    body: { reason: "画面から取り消した" },
  };
}

export function dismissAction(projectId: string, proposalId: string): ActionTarget {
  return {
    id: AUTHORITY_DISMISS_ACTION_ID,
    method: "POST",
    path: `${base(projectId)}/proposals/${encodeURIComponent(proposalId)}/dismiss`,
    body: {},
  };
}

const STATE_LABEL: Record<string, string> = {
  live: "有効",
  grant_revoked: "取り消し済み",
  grant_expired: "期限切れ",
  grant_budget_exhausted: "回数を使い切った",
  grant_expiry_unreadable: "期限が読めない（無効）",
};

export function ProjectAuthorityPanel({
  view,
  pending,
  onAccept,
  onDismiss,
  onRevoke,
}: {
  view: AuthorityView;
  pending: boolean;
  onAccept: (proposal: AuthorityProposalView) => void;
  onDismiss: (proposal: AuthorityProposalView) => void;
  onRevoke: (grant: AuthorityGrantView) => void;
}) {
  const open = view.proposals.filter((p) => p.state === "open");
  const unknown = view.audit.filter((a) => a.outcome_state === "unknown");
  return (
    <section className="space-y-3" data-testid="project-authority">
      <h3 className="text-label font-medium">許可</h3>
      <p className="text-label text-muted-foreground">
        CoS と controller がこの案件で人に聞かずに行える範囲です。許可には期限と回数があり、人の発言などの根拠が付きます。
      </p>
      {unknown.length > 0 && (
        <p role="alert" className="text-label" data-testid="authority-unknown">
          結果を確かめられていない操作が {unknown.length} 件あります。確かめるまで、同じ種類の操作は止まります。
        </p>
      )}
      {open.length > 0 && (
        <ul className="space-y-2" data-testid="authority-proposals">
          {open.map((p) => (
            <li key={p.id} className="space-y-1">
              <p className="text-label">
                CoS の提案: {authorityClassLabel(p.action_class)}（{p.budget} 回まで）。{p.note}
              </p>
              <div className="flex gap-2">
                <button type="button" className="min-h-11 px-3" disabled={pending} onClick={() => onAccept(p)}>
                  許可する
                </button>
                <button type="button" className="min-h-11 px-3" disabled={pending} onClick={() => onDismiss(p)}>
                  断る
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {view.grants.length === 0 ? (
        <p className="text-label" data-testid="authority-none">
          許可はありません。controller は続きの root を起票せず、人に聞きます。
        </p>
      ) : (
        <ul className="space-y-2" data-testid="authority-grants">
          {view.grants.map((g) => (
            <li key={g.id} className="space-y-1">
              <p className="text-label">
                {authorityClassLabel(g.action_class)}・{STATE_LABEL[g.state] ?? g.state}・残り {g.budget - g.consumed}{" "}
                回・期限 {g.expires_at}
              </p>
              <p className="text-label text-muted-foreground">
                根拠: {g.source.kind} {g.source.ref}
                {g.source_excerpt ? `「${g.source_excerpt}」` : ""}
              </p>
              {g.state === "live" && (
                <button type="button" className="min-h-11 px-3" disabled={pending} onClick={() => onRevoke(g)}>
                  取り消す
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
