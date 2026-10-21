import type { ActionTarget } from "../../components/actions/use-action-result";

// ADR 2026-10-16-project-auto-integrate: レビュー合格した root task を既定ブランチへ自動で取り込む（案件の属性）。
// 切替は PATCH /api/v1/projects/{id} {auto_integrate}。応答をそのまま出し、GUI 側で検証しない。

export const AUTO_INTEGRATE_ACTION_ID = "project_auto_integrate";

/** 切替の PATCH。`enabled` は切り替えた後の値。 */
export function autoIntegrateAction(projectId: string, enabled: boolean): ActionTarget {
  return {
    id: AUTO_INTEGRATE_ACTION_ID,
    method: "PATCH",
    path: `/api/projects/${encodeURIComponent(projectId)}`,
    body: { auto_integrate: enabled },
  };
}

export function AutoIntegrateToggle({
  enabled,
  pending,
  onToggle,
}: {
  enabled: boolean;
  pending: boolean;
  onToggle: (next: boolean) => void;
}) {
  return (
    <fieldset className="space-y-1" data-testid="project-auto-integrate">
      <legend className="text-label font-medium">自動取り込み</legend>
      <label className="flex min-h-11 items-center gap-2 text-label">
        <input
          type="checkbox"
          className="size-5"
          checked={enabled}
          disabled={pending}
          onChange={(event) => onToggle(event.target.checked)}
        />
        レビュー合格した task を既定ブランチへ自動で取り込む
      </label>
      <p className="text-label text-muted-foreground">
        案件のリポジトリの既定ブランチへ fast-forward で取り込むだけで、release・本番への反映・push（remote
        があるときを除く）は行いません。
      </p>
    </fieldset>
  );
}
