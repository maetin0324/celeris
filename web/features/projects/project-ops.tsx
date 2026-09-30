import { useRef, useState } from "react";
import type { ProjectDetail, WorkspaceSpec } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";
import {
  type ActionResult,
  ActionResultView,
  type ActionTarget,
  useActionResult,
} from "../../components/actions/use-action-result";
import { ProjectSection } from "./project-detail-view";

// R10 /projects/:id の操作（P4-03〜P4-05）。どれも celeris の応答をそのまま出し、GUI 側で検証しない。
// 計画・途中目標の intent は ADR-0083 の後継 API に写す（410 の入口は呼ばない）。

const inputClass = "min-h-11 w-full min-w-0 rounded border px-2";
const buttonClass = "min-h-11 rounded border px-3";

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

function ConfirmButton({
  label,
  question,
  disabled,
  onConfirm,
}: {
  label: string;
  question: string;
  disabled: boolean;
  onConfirm: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  return (
    <>
      <button type="button" className={buttonClass} disabled={disabled} onClick={() => ref.current?.showModal()}>
        {label}
      </button>
      <dialog ref={ref} aria-label={label} className="max-w-[90vw] rounded border p-4">
        <p>{question}</p>
        <div className="mt-3 flex flex-wrap gap-2">
          <button
            type="button"
            className={buttonClass}
            onClick={() => {
              ref.current?.close();
              onConfirm();
            }}
          >
            {label}する
          </button>
          <button type="button" className={buttonClass} onClick={() => ref.current?.close()}>
            やめる
          </button>
        </div>
      </dialog>
    </>
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
  return (
    <ProjectSection title="案件の操作" testId="project-ops">
      <form
        className="space-y-2"
        onSubmit={(event) => {
          event.preventDefault();
          act({ id: "project_edit", method: "PATCH", path: base, body: { title, request } });
        }}
      >
        <label className="block">
          案件名
          <input className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="block">
          依頼文
          <textarea className={inputClass} value={request} onChange={(e) => setRequest(e.target.value)} />
        </label>
        <button type="submit" className={buttonClass} disabled={pending}>
          名前と依頼を保存
        </button>
      </form>
      <div className="flex flex-wrap items-end gap-2">
        <label>
          状態
          <select className={inputClass} value={status} onChange={(e) => setStatus(e.target.value)}>
            <option value="proposed">提案</option>
            <option value="active">進行中</option>
            <option value="done">完了</option>
          </select>
        </label>
        <button
          type="button"
          className={buttonClass}
          disabled={pending}
          onClick={() => act({ id: "project_status", method: "PATCH", path: base, body: { status } })}
        >
          状態を変える
        </button>
      </div>
      <div className="flex flex-wrap gap-2">
        {project.status === "paused" ? (
          <button type="button" className={buttonClass} disabled={pending} onClick={() => lifecycle("resume")}>
            再開
          </button>
        ) : (
          <button type="button" className={buttonClass} disabled={pending} onClick={() => lifecycle("pause")}>
            一時停止
          </button>
        )}
        <ConfirmButton
          label="中止"
          question="この案件を中止します。動いている仕事も止まります。"
          disabled={pending}
          onConfirm={() => lifecycle("cancel")}
        />
        {project.archived_at ? (
          <button type="button" className={buttonClass} disabled={pending} onClick={() => lifecycle("unarchive")}>
            アーカイブから戻す
          </button>
        ) : (
          <ConfirmButton
            label="アーカイブ"
            question="この案件をアーカイブします。一覧の既定の表示から外れます。"
            disabled={pending}
            onConfirm={() => lifecycle("archive")}
          />
        )}
      </div>
      <fieldset className="space-y-2">
        <legend>作業場所</legend>
        <label className="block">
          種類
          <select className={inputClass} value={wsKind} onChange={(e) => setWsKind(e.target.value)}>
            <option value="local">手元</option>
            <option value="remote">クラスタ</option>
          </select>
        </label>
        {wsKind === "remote" && (
          <label className="block">
            クラスタ
            <input className={inputClass} value={wsCluster} onChange={(e) => setWsCluster(e.target.value)} />
          </label>
        )}
        <label className="block">
          作業場所の path
          <input className={inputClass} value={wsPath} onChange={(e) => setWsPath(e.target.value)} />
        </label>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className={buttonClass}
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
          </button>
          <button
            type="button"
            className={buttonClass}
            disabled={pending}
            onClick={() =>
              act({ id: "project_workspace_clear", method: "PATCH", path: base, body: { workspace: null } })
            }
          >
            作業場所を消す
          </button>
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
        <label className="block">
          仕事の題
          <input className={inputClass} value={taskTitle} onChange={(e) => setTaskTitle(e.target.value)} />
        </label>
        <label className="block">
          仕事の目的
          <textarea className={inputClass} value={taskObjective} onChange={(e) => setTaskObjective(e.target.value)} />
        </label>
        <button type="submit" className={buttonClass} disabled={pending}>
          仕事を足す
        </button>
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

// P4-04: 計画と途中目標（ADR-0083）。
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
        <label className="block">
          計画の目標
          <input className={inputClass} value={goal} onChange={(e) => setGoal(e.target.value)} />
        </label>
        <label className="block">
          段階（1 行に 1 つ、任意）
          <textarea className={inputClass} value={stages} onChange={(e) => setStages(e.target.value)} />
        </label>
        <button type="submit" className={buttonClass} disabled={busy}>
          計画を立てる
        </button>
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
        <label className="min-w-0 flex-1">
          途中目標
          <input className={inputClass} value={milestone} onChange={(e) => setMilestone(e.target.value)} />
        </label>
        <button type="submit" className={buttonClass} disabled={busy}>
          途中目標を足す
        </button>
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
              <li key={task.id} className="min-w-0 space-y-1 rounded border p-2" data-root-task={task.id}>
                <p className="break-words">
                  {task.title} <span className="text-sm">{task.status}</span>
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    className={buttonClass}
                    disabled={busy}
                    onClick={() => gate(task.id, "plan-gate", "approve", "project_plan_decide")}
                  >
                    計画を承認
                  </button>
                  <button
                    type="button"
                    className={buttonClass}
                    disabled={busy}
                    onClick={() => gate(task.id, "phase-gate", "continue", "milestone_decide")}
                  >
                    段階を通す
                  </button>
                  <button
                    type="button"
                    className={buttonClass}
                    disabled={busy}
                    onClick={() => gate(task.id, "phase-gate", "withdraw", "milestone_status")}
                  >
                    段階を取り下げる
                  </button>
                  {(["pause", "resume", "cancel"] as const).map((k) => (
                    <button
                      key={k}
                      type="button"
                      className={buttonClass}
                      disabled={busy}
                      onClick={() =>
                        act({ id: `milestone_${k}:${task.id}`, path: `/api/tasks/${enc(task.id)}/${k}`, body: {} })
                      }
                    >
                      {k === "pause" ? "止める" : k === "resume" ? "再開" : "取り消す"}
                    </button>
                  ))}
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
                {m.seq}. {m.title} <span className="text-sm">{m.status}</span>
              </li>
            ))}
          </ul>
        </details>
      )}
    </ProjectSection>
  );
}
