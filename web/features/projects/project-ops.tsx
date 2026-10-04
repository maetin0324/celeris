import { useState } from "react";
import type { ProjectDetail, ProjectRepo, WorkspaceSpec } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";
import {
  type ActionResult,
  ActionResultView,
  type ActionTarget,
  useActionResult,
} from "../../components/actions/use-action-result";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { StatusBadge } from "../../components/ui/status-badge";
import { MilestoneBadge, ProjectSection } from "./project-detail-view";
import { inputClass } from "./project-list-screen";

// R10 /projects/:id の操作（P4-03〜P4-05）。どれも celeris の応答をそのまま出し、GUI 側で検証しない。
// 計画・途中目標の intent は web ADR-W2 の後継 API に写す（410 の入口は呼ばない）。

const enc = encodeURIComponent;

function useProjectActions(projectId: string) {
  return useActionResult(projectKeys.detail(projectId));
}

function Results({ results, ids }: { results: Record<string, ActionResult>; ids: readonly string[] }) {
  return (
    <>
      {ids.map((id) => (
        <ActionResultView key={id} result={results[id]} />
      ))}
    </>
  );
}

/** 破壊的操作の確認（ConfirmDialog）。対象・結果・戻し方・確認先を書いてから送る。 */
function ConfirmButton({
  label,
  title,
  target,
  consequence,
  reversibility,
  followUp,
  confirmLabel,
  disabled,
  onConfirm,
}: {
  label: string;
  title: string;
  target: string;
  consequence: string;
  reversibility: string;
  followUp: string;
  confirmLabel: string;
  disabled: boolean;
  onConfirm: () => Promise<unknown>;
}) {
  return (
    <ConfirmDialog
      trigger={
        <Button variant="secondary" disabled={disabled}>
          {label}
        </Button>
      }
      title={title}
      target={target}
      consequence={consequence}
      reversibility={reversibility}
      followUp={followUp}
      confirmLabel={confirmLabel}
      cancelLabel="やめる"
      onConfirm={async () => {
        await onConfirm();
      }}
    />
  );
}

function readWorkspace(kind: string, path: string, cluster: string): WorkspaceSpec {
  return kind === "remote" ? { kind: "remote", cluster, path: path || undefined } : { kind: "local", path };
}

// P4-03: 案件と作業場所、task_create。
export function ProjectOps({ detail }: { detail: ProjectDetail }) {
  const { project } = detail;
  const id = project.id;
  const base = `/api/projects/${enc(id)}`;
  const { run, pending, results } = useProjectActions(id);
  const [title, setTitle] = useState(project.title);
  const [request, setRequest] = useState(project.request);
  const [status, setStatus] = useState<string>(project.status);
  const [wsKind, setWsKind] = useState<string>(project.workspace?.kind ?? "local");
  const [wsPath, setWsPath] = useState(project.workspace?.path ?? "");
  const [wsCluster, setWsCluster] = useState(project.workspace?.kind === "remote" ? project.workspace.cluster : "");
  const [taskTitle, setTaskTitle] = useState("");
  const [taskObjective, setTaskObjective] = useState("");
  const act = (target: ActionTarget) => void run([target]);
  const lifecycle = (name: string) => act({ id: `project_${name}`, path: `${base}/${name}`, body: {} });
  const confirmLifecycle = (name: string) => run([{ id: `project_${name}`, path: `${base}/${name}`, body: {} }]);
  return (
    <ProjectSection title="案件の操作" testId="project-ops">
      <form
        className="space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          act({ id: "project_edit", method: "PATCH", path: base, body: { title, request } });
        }}
      >
        <label className="block text-label font-medium">
          案件名
          <input className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="block text-label font-medium">
          依頼文
          <textarea className={inputClass} value={request} onChange={(e) => setRequest(e.target.value)} />
        </label>
        <Button type="submit" variant="primary" disabled={pending}>
          名前と依頼を保存
        </Button>
      </form>
      <div className="flex flex-wrap items-end gap-2">
        <label className="text-label font-medium">
          状態
          <select className={inputClass} value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value="proposed">提案</option>
            <option value="active">進行中</option>
            <option value="done">完了</option>
          </select>
        </label>
        <Button
          variant="secondary"
          disabled={pending}
          onClick={() => act({ id: "project_status", method: "PATCH", path: base, body: { status } })}
        >
          状態を変える
        </Button>
      </div>
      <div className="flex flex-wrap gap-2">
        {project.status === "paused" ? (
          <Button variant="secondary" disabled={pending} onClick={() => lifecycle("resume")}>
            再開
          </Button>
        ) : (
          <Button variant="secondary" disabled={pending} onClick={() => lifecycle("pause")}>
            一時停止
          </Button>
        )}
        <ConfirmButton
          label="中止"
          title="案件を中止しますか"
          target={project.title}
          consequence="案件を中止し、動いている仕事と途中目標も止めます。"
          reversibility="中止した仕事は戻りません。続けるには新しい仕事を足します。"
          followUp="この画面の状態と仕事の木"
          confirmLabel="案件を中止する"
          disabled={pending}
          onConfirm={() => confirmLifecycle("cancel")}
        />
        {project.archived_at ? (
          <Button variant="secondary" disabled={pending} onClick={() => lifecycle("unarchive")}>
            アーカイブから戻す
          </Button>
        ) : (
          <ConfirmButton
            label="アーカイブ"
            title="案件をアーカイブしますか"
            target={project.title}
            consequence="案件の一覧の既定の表示から外します。"
            reversibility="この画面の「アーカイブから戻す」で戻せます。"
            followUp="案件の一覧の「アーカイブした案件も出す」"
            confirmLabel="案件をアーカイブする"
            disabled={pending}
            onConfirm={() => confirmLifecycle("archive")}
          />
        )}
      </div>
      <fieldset className="space-y-2">
        <legend className="text-label font-medium">作業場所</legend>
        <label className="block text-label font-medium">
          種類
          <select className={inputClass} value={wsKind} onChange={(e) => setWsKind(e.target.value)}>
            <option value="local">手元</option>
            <option value="remote">クラスタ</option>
          </select>
        </label>
        {wsKind === "remote" && (
          <label className="block text-label font-medium">
            クラスタ
            <input className={inputClass} value={wsCluster} onChange={(e) => setWsCluster(e.target.value)} />
          </label>
        )}
        <label className="block text-label font-medium">
          作業場所の path
          <input className={inputClass} value={wsPath} onChange={(e) => setWsPath(e.target.value)} />
        </label>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            disabled={pending}
            onClick={() =>
              act({
                id: "project_workspace_save",
                method: "PATCH",
                path: base,
                body: { workspace: readWorkspace(wsKind, wsPath, wsCluster) },
              })
            }
          >
            作業場所を保存
          </Button>
          <Button
            variant="secondary"
            disabled={pending}
            onClick={() =>
              act({ id: "project_workspace_clear", method: "PATCH", path: base, body: { workspace: null } })
            }
          >
            作業場所を消す
          </Button>
        </div>
      </fieldset>
      <form
        className="space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          act({
            id: "task_create",
            path: "/api/tasks",
            body: { title: taskTitle, objective: taskObjective || taskTitle, acceptance: [], project_id: id },
          });
        }}
      >
        <label className="block text-label font-medium">
          仕事の題
          <input className={inputClass} value={taskTitle} onChange={(e) => setTaskTitle(e.target.value)} />
        </label>
        <label className="block text-label font-medium">
          仕事の目的
          <textarea className={inputClass} value={taskObjective} onChange={(e) => setTaskObjective(e.target.value)} />
        </label>
        <Button type="submit" variant="primary" disabled={pending}>
          仕事を足す
        </Button>
      </form>
      <Results
        results={results}
        ids={[
          "project_edit",
          "project_status",
          "project_pause",
          "project_resume",
          "project_cancel",
          "project_archive",
          "project_unarchive",
          "project_workspace_save",
          "project_workspace_clear",
          "task_create",
        ]}
      />
    </ProjectSection>
  );
}

// P4-04: 計画と途中目標（web ADR-W2）。
export function PlanOps({ detail }: { detail: ProjectDetail }) {
  const id = detail.project.id;
  const { run, pending, results } = useProjectActions(id);
  const [goal, setGoal] = useState("");
  const [stages, setStages] = useState("");
  const [milestone, setMilestone] = useState("");
  const roots = detail.tasks.filter((task) => task.is_root_task || !task.parent_id);
  const busy = pending;
  const act = (target: ActionTarget) => void run([target]);
  const gate = (taskId: string, kind: "plan-gate" | "phase-gate", action: string, intent: string) =>
    act({ id: `${intent}:${taskId}`, path: `/api/tasks/${enc(taskId)}/execution/${kind}`, body: { action } });
  const ids = ["project_plan", "milestone_create"];
  return (
    <ProjectSection title="計画と途中目標" testId="project-plan-ops">
      <form
        className="space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          const hints = stages
            .split("\n")
            .map((line) => line.trim())
            .filter(Boolean)
            .map((line) => ({ title: line }));
          act({
            id: "project_plan",
            path: "/api/tasks",
            body: { title: goal, objective: goal, acceptance: [], project_id: id, stages_hint: hints },
          });
        }}
      >
        <label className="block text-label font-medium">
          計画の目標
          <input className={inputClass} value={goal} onChange={(e) => setGoal(e.target.value)} />
        </label>
        <label className="block text-label font-medium">
          段階（1 行に 1 つ、任意）
          <textarea className={inputClass} value={stages} onChange={(e) => setStages(e.target.value)} />
        </label>
        <Button type="submit" variant="primary" disabled={busy}>
          計画を立てる
        </Button>
      </form>
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          act({
            id: "milestone_create",
            path: "/api/tasks",
            body: {
              title: milestone,
              objective: milestone,
              acceptance: [],
              project_id: id,
              stages_hint: [{ title: milestone }],
            },
          });
        }}
      >
        <label className="min-w-0 flex-1 text-label font-medium">
          途中目標
          <input className={inputClass} value={milestone} onChange={(e) => setMilestone(e.target.value)} />
        </label>
        <Button type="submit" variant="primary" disabled={busy}>
          途中目標を足す
        </Button>
      </form>
      <Results results={results} ids={ids} />
      {roots.length > 0 && (
        <ul className="space-y-2" data-testid="project-root-ops">
          {roots.map((task) => {
            const rowIds = [
              "project_plan_decide",
              "milestone_decide",
              "milestone_status",
              "milestone_pause",
              "milestone_resume",
              "milestone_cancel",
            ].map((intent) => `${intent}:${task.id}`);
            return (
              <li
                key={task.id}
                className="min-w-0 space-y-2 rounded-md border border-border p-3"
                data-root-task={task.id}
              >
                <p className="break-words">
                  {task.title} <StatusBadge status={task.status} />
                </p>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="secondary"
                    disabled={busy}
                    onClick={() => gate(task.id, "plan-gate", "approve", "project_plan_decide")}
                  >
                    計画を承認
                  </Button>
                  <Button
                    variant="secondary"
                    disabled={busy}
                    onClick={() => gate(task.id, "phase-gate", "continue", "milestone_decide")}
                  >
                    段階を通す
                  </Button>
                  <ConfirmButton
                    label="段階を取り下げる"
                    title="段階を取り下げますか"
                    target={task.title}
                    consequence="この途中目標の段階を取り下げ、仕事を止めます。"
                    reversibility="取り下げは戻りません。もう一度計画を立て直します。"
                    followUp="この画面の仕事の木と受信箱"
                    confirmLabel="段階を取り下げる"
                    disabled={busy}
                    onConfirm={() =>
                      run([
                        {
                          id: `milestone_status:${task.id}`,
                          path: `/api/tasks/${enc(task.id)}/execution/phase-gate`,
                          body: { action: "withdraw" },
                        },
                      ])
                    }
                  />
                  {(["pause", "resume"] as const).map((k) => (
                    <Button
                      key={k}
                      variant="secondary"
                      disabled={busy}
                      onClick={() =>
                        act({ id: `milestone_${k}:${task.id}`, path: `/api/tasks/${enc(task.id)}/${k}`, body: {} })
                      }
                    >
                      {k === "pause" ? "止める" : "再開"}
                    </Button>
                  ))}
                  <ConfirmButton
                    label="取り消す"
                    title="途中目標の仕事を取り消しますか"
                    target={task.title}
                    consequence="この仕事と子の仕事を取り消します。"
                    reversibility="取り消しは戻りません。続けるには新しい仕事を足します。"
                    followUp="この画面の仕事の木"
                    confirmLabel="仕事を取り消す"
                    disabled={busy}
                    onConfirm={() =>
                      run([{ id: `milestone_cancel:${task.id}`, path: `/api/tasks/${enc(task.id)}/cancel`, body: {} }])
                    }
                  />
                </div>
                <Results results={results} ids={rowIds} />
              </li>
            );
          })}
        </ul>
      )}
      {(detail.milestones_frozen ?? 0) > 0 && (
        <details>
          <summary className="min-h-11 py-2">以前の途中目標 {detail.milestones_frozen} 件（読み取り専用）</summary>
          <ul className="space-y-1">
            {detail.milestones.map((m) => (
              <li key={m.id} className="break-words">
                {m.seq}. {m.title} <MilestoneBadge status={m.status} />
              </li>
            ))}
          </ul>
        </details>
      )}
    </ProjectSection>
  );
}

function RepoRow({ repo, projectId }: { repo: ProjectRepo; projectId: string }) {
  const { run, pending, results } = useProjectActions(projectId);
  const [name, setName] = useState(repo.name);
  const [branch, setBranch] = useState(repo.default_branch ?? "");
  const path = `/api/repos/${enc(repo.id)}`;
  return (
    <li className="min-w-0 space-y-2 rounded-md border border-border p-3" data-repo={repo.id}>
      <p className="break-words">
        {repo.name}
        {repo.is_primary ? "（主）" : ""}{" "}
        <span className="text-label text-muted-foreground">{repo.location.path ?? ""}</span>
      </p>
      <div className="flex flex-wrap items-end gap-2">
        <label className="min-w-0 text-label font-medium">
          名前 {repo.id}
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="min-w-0 text-label font-medium">
          既定ブランチ {repo.id}
          <input className={inputClass} value={branch} onChange={(e) => setBranch(e.target.value)} />
        </label>
        <Button
          variant="secondary"
          disabled={pending}
          onClick={() =>
            void run([
              {
                id: "repo_patch",
                method: "PATCH",
                path,
                body: { name, default_branch: branch || null },
              },
            ])
          }
        >
          保存
        </Button>
        {!repo.is_primary && (
          <Button
            variant="secondary"
            disabled={pending}
            onClick={() => void run([{ id: "repo_primary", method: "PATCH", path, body: { is_primary: true } }])}
          >
            主にする
          </Button>
        )}
        <ConfirmButton
          label="削除"
          title="リポジトリを案件から外しますか"
          target={repo.name}
          consequence="このリポジトリを案件から外します。リポジトリの中身は消しません。"
          reversibility="「リポジトリを足す」で同じ path を足し直せます。"
          followUp="この画面のリポジトリの一覧"
          confirmLabel={`${repo.name} を外す`}
          disabled={pending}
          onConfirm={() => run([{ id: "repo_delete", method: "DELETE", path }])}
        />
      </div>
      <Results results={results} ids={["repo_patch", "repo_primary", "repo_delete"]} />
    </li>
  );
}

// P4-05: リポジトリ。
export function RepoOps({ detail }: { detail: ProjectDetail }) {
  const id = detail.project.id;
  const { run, pending, results } = useProjectActions(id);
  const [name, setName] = useState("");
  const [location, setLocation] = useState("");
  const repos = detail.repos ?? [];
  return (
    <ProjectSection title="リポジトリ" testId="project-repos">
      {repos.length === 0 ? (
        <p>リポジトリはまだありません。</p>
      ) : (
        <ul className="space-y-2">
          {repos.map((repo) => (
            <RepoRow key={repo.id} repo={repo} projectId={id} />
          ))}
        </ul>
      )}
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void run([
            {
              id: "repo_create",
              path: `/api/projects/${enc(id)}/repos`,
              body: { name: name || null, location: { kind: "local", path: location } },
            },
          ]);
        }}
      >
        <label className="min-w-0 text-label font-medium">
          リポジトリ名
          <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="min-w-0 text-label font-medium">
          リポジトリの path
          <input className={inputClass} value={location} onChange={(e) => setLocation(e.target.value)} />
        </label>
        <Button type="submit" variant="primary" disabled={pending}>
          リポジトリを足す
        </Button>
      </form>
      <Results results={results} ids={["repo_create"]} />
    </ProjectSection>
  );
}
