import type { ProjectRepo } from "../../api/generated/types";
import type { ActionTarget } from "../../components/actions/use-action-result";

// ADR 2026-11-14-repo-auto-integrate: 自動取り込みはリポジトリの欄。切替は PATCH /api/v1/repos/{id} {auto_integrate}。
// 有効にできる条件の正本はサーバー（remote・dir の行は 422）。GUI は無効表示と理由を出すだけで、応答をそのまま出す。

export const REPO_AUTO_INTEGRATE_ACTION_ID = "repo_auto_integrate";

export function repoAutoIntegrateAction(repoId: string, enabled: boolean): ActionTarget {
  return {
    id: REPO_AUTO_INTEGRATE_ACTION_ID,
    method: "PATCH",
    path: `/api/repos/${encodeURIComponent(repoId)}`,
    body: { auto_integrate: enabled },
  };
}

/** 切り替えられない理由。切り替えられるなら null。すでに有効な行は戻せるよう無効にしない。 */
export function autoIntegrateDisabledReason(repo: ProjectRepo): string | null {
  if (repo.auto_integrate) return null;
  if (repo.location.kind === "remote") return "remote のリポジトリは自動取り込みにできません。";
  if (repo.kind === "dir") return "git でないディレクトリは自動取り込みにできません。";
  return null;
}

export function RepoAutoIntegrate({
  repo,
  pending,
  onToggle,
}: {
  repo: ProjectRepo;
  pending: boolean;
  onToggle: (next: boolean) => void;
}) {
  const reason = autoIntegrateDisabledReason(repo);
  return (
    <fieldset className="space-y-1" data-testid="repo-auto-integrate">
      <legend className="text-label font-medium">自動取り込み</legend>
      <label className="flex min-h-11 items-center gap-2 text-label">
        <input
          type="checkbox"
          className="size-5"
          checked={repo.auto_integrate ?? false}
          disabled={pending || reason !== null}
          onChange={(event) => onToggle(event.target.checked)}
        />
        このリポジトリだけに書くレビュー合格の task を既定ブランチへ自動で取り込む
      </label>
      {reason && (
        <p className="text-label text-muted-foreground" data-testid="repo-auto-integrate-reason">
          {reason}
        </p>
      )}
    </fieldset>
  );
}
