import type { ActionTarget } from "../../components/actions/use-action-result";

// ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap（人が認可した途中目標の版）。
// 読むのは GET /api/v1/projects/{id}/roadmap と …/roadmap/history。版を進めるのは人の決定
// （POST …/roadmap/proposals/{version}/decide）だけで、この画面の承認・却下は人の画面（human:web）から送る。
// 旧 milestones（凍結）や task の phase は表示しない。brief（状態の要約）とは別の欄として出す。
// 案件画面への結線は A9 が行う（この file は panel と単体試験まで）。

export type OutcomeStatus = "open" | "in_progress" | "reached" | "dropped";
export type RoadmapStatus = "proposed" | "approved" | "rejected" | "superseded";

export interface RoadmapOutcome {
  id: string;
  title: string;
  acceptance: string[];
  evidence: string[];
  priority: number;
  depends_on: string[];
  status: OutcomeStatus;
  root_task_ids: string[];
}

export interface Roadmap {
  project_id: string;
  version: number;
  status: RoadmapStatus;
  request_sha256: string;
  change_reason: string;
  proposed_by: string;
  decided_by: string | null;
  decided_at: string | null;
  outcomes: RoadmapOutcome[];
}

export interface RoadmapOutcomeSpec {
  id: string;
  title: string;
  acceptance: string[];
  priority: number;
  depends_on: string[];
  dropped: boolean;
}

export interface RoadmapVersion {
  project_id: string;
  version: number;
  status: RoadmapStatus;
  base_version: number | null;
  request_sha256: string;
  change_reason: string;
  outcomes: RoadmapOutcomeSpec[];
  proposed_by: string;
  proposed_at: string;
  decided_by: string | null;
  decided_at: string | null;
  decision_note: string | null;
  superseded_at: string | null;
}

export interface RoadmapHistory {
  versions: RoadmapVersion[];
  outcome_events: {
    id: number;
    outcome_id: string;
    kind: "status" | "root_linked";
    status: OutcomeStatus | null;
    root_task_id: string | null;
    roadmap_version: number;
    evidence: string[];
    reason: string;
    actor: string;
    created_at: string;
  }[];
}

export const ROADMAP_DECIDE_ACTION_ID = "project_roadmap_decide";

/** 人の決定の POST。`decision` は approve か reject。 */
export function roadmapDecideAction(
  projectId: string,
  version: number,
  decision: "approve" | "reject",
  note = "",
): ActionTarget {
  return {
    id: ROADMAP_DECIDE_ACTION_ID,
    method: "POST",
    path: `/api/projects/${encodeURIComponent(projectId)}/roadmap/proposals/${version}/decide`,
    body: { decision, note },
  };
}

const STATUS_LABEL: Record<OutcomeStatus, string> = {
  open: "未着手",
  in_progress: "進行中",
  reached: "到達",
  dropped: "取り下げ",
};

const VERSION_LABEL: Record<RoadmapStatus, string> = {
  proposed: "提案（未決定）",
  approved: "承認済み",
  rejected: "却下",
  superseded: "置き換え済み",
};

/** 承認を待つ提案（版の番号の順）。 */
export function pendingProposals(history: RoadmapHistory | null): RoadmapVersion[] {
  return (history?.versions ?? []).filter((v) => v.status === "proposed").sort((a, b) => a.version - b.version);
}

/** 提案と今の版の差（足した id・取り下げた id・条件を変えた id）。 */
export function proposalDiff(current: Roadmap | null, proposal: RoadmapVersion) {
  const before = new Map((current?.outcomes ?? []).map((o) => [o.id, o]));
  const added: string[] = [];
  const dropped: string[] = [];
  const changed: string[] = [];
  for (const o of proposal.outcomes) {
    const prev = before.get(o.id);
    if (!prev) added.push(o.id);
    else if (o.dropped && prev.status !== "dropped") dropped.push(o.id);
    else if (prev.title !== o.title || JSON.stringify(prev.acceptance) !== JSON.stringify(o.acceptance))
      changed.push(o.id);
  }
  return { added, dropped, changed };
}

export function ProjectRoadmapPanel({
  roadmap,
  history,
  pending,
  onDecide,
}: {
  roadmap: Roadmap | null;
  history: RoadmapHistory | null;
  pending: boolean;
  onDecide: (version: number, decision: "approve" | "reject") => void;
}) {
  const proposals = pendingProposals(history);
  return (
    <section className="space-y-3" data-testid="project-roadmap" aria-labelledby="project-roadmap-title">
      <h3 id="project-roadmap-title" className="text-label font-medium">
        Roadmap（人が承認した途中目標）
      </h3>
      {roadmap ? (
        <div className="space-y-2">
          <p className="text-label text-muted-foreground">
            第 {roadmap.version} 版（{VERSION_LABEL[roadmap.status]}。承認: {roadmap.decided_by ?? "—"}）。理由:{" "}
            {roadmap.change_reason}
          </p>
          <div className="overflow-x-auto">
            <table className="w-full text-label">
              <thead>
                <tr>
                  <th className="text-left">id</th>
                  <th className="text-left">達成したい状態</th>
                  <th className="text-left">状態</th>
                  <th className="text-left">担った root</th>
                </tr>
              </thead>
              <tbody>
                {roadmap.outcomes.map((o) => (
                  <tr key={o.id} data-outcome={o.id} data-status={o.status}>
                    <td>{o.id}</td>
                    <td>
                      {o.title}
                      <ul className="list-disc pl-4 text-muted-foreground">
                        {o.acceptance.map((a) => (
                          <li key={a}>{a}</li>
                        ))}
                      </ul>
                    </td>
                    <td>{STATUS_LABEL[o.status]}</td>
                    <td>{o.root_task_ids.length === 0 ? "—" : o.root_task_ids.join(", ")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : (
        <p className="text-label text-muted-foreground">
          承認済みの Roadmap はありません。controller は依頼文の途中目標の block を読みます。
        </p>
      )}
      {proposals.map((p) => {
        const diff = proposalDiff(roadmap, p);
        return (
          <div key={p.version} className="space-y-1 rounded border p-2" data-proposal={p.version}>
            <p className="text-label">
              第 {p.version} 版の提案（{p.proposed_by}）: {p.change_reason}
            </p>
            <p className="text-label text-muted-foreground">
              追加 {diff.added.join(", ") || "なし"} / 取り下げ {diff.dropped.join(", ") || "なし"} / 条件の変更{" "}
              {diff.changed.join(", ") || "なし"}
            </p>
            <div className="flex gap-2">
              <button
                type="button"
                className="min-h-11 rounded border px-3"
                disabled={pending}
                onClick={() => onDecide(p.version, "approve")}
              >
                承認して版を進める
              </button>
              <button
                type="button"
                className="min-h-11 rounded border px-3"
                disabled={pending}
                onClick={() => onDecide(p.version, "reject")}
              >
                却下する
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}
