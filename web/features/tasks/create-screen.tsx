import { useNavigate } from "@tanstack/react-router";
import { type FormEvent, useRef, useState } from "react";
import type { CriterionSpec, NewTaskSpec, Task } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

const control = "min-h-11 w-full rounded border border-neutral-400 bg-white p-2";
type CriterionType = "command" | "artifact_exists" | "reviewer" | "human";
type Row = { id: number; type: CriterionType; value: string };
const criterionTypes: { type: CriterionType; label: string }[] = [
  { type: "command", label: "command" },
  { type: "artifact_exists", label: "artifact_exists" },
  { type: "reviewer", label: "reviewer" },
  { type: "human", label: "human" },
];

export function buildCriteria(rows: readonly Row[]): CriterionSpec[] {
  return rows.flatMap(({ type, value }): CriterionSpec[] => {
    const text = value.trim();
    if (!text) return [];
    if (type === "command") return [{ type, cmd: text, expect_exit: 0 }];
    if (type === "artifact_exists") return [{ type, name: text }];
    return [{ type, text }];
  });
}

function createdId(response: unknown): string | null {
  if (!response || typeof response !== "object") return null;
  const id = (response as Partial<Task>).id;
  return typeof id === "string" && id !== "" ? id : null;
}

export function TaskCreateScreen() {
  const navigate = useNavigate();
  const sender = useActionResult(taskKeys.all);
  const nextRow = useRef(1);
  const [title, setTitle] = useState("");
  const [objective, setObjective] = useState("");
  const [rows, setRows] = useState<Row[]>([{ id: 0, type: "human", value: "" }]);
  const result = sender.results["create-task"];
  const errorId = "task-create-error";
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const id = "create-task";
    const body: NewTaskSpec = { title, objective, acceptance: buildCriteria(rows) };
    const [outcome] = await sender.run([{ id, path: "/api/tasks", body }]);
    if (outcome?.ok) {
      const taskId = createdId(outcome.response);
      if (taskId) void navigate({ to: "/tasks/$id", params: { id: taskId } });
    }
  }
  return (
    <ScreenFrame title="タスクの作成" route="/tasks/new">
      <form onSubmit={submit} className="max-w-3xl space-y-5" data-testid="new-task-form">
        <div>
          <label htmlFor="task-title" className="block font-medium">
            名前
          </label>
          <input
            id="task-title"
            className={control}
            value={title}
            onChange={(event) => setTitle(event.target.value)}
            aria-describedby={result?.status === 422 ? errorId : undefined}
          />
          {result?.status === 422 && <ActionResultView result={result} fieldId={errorId} />}
        </div>
        <div>
          <label htmlFor="task-objective" className="block font-medium">
            目的
          </label>
          <textarea
            id="task-objective"
            aria-label="目的"
            className={control}
            rows={4}
            value={objective}
            onChange={(event) => setObjective(event.target.value)}
            aria-describedby={result?.status === 422 ? errorId : undefined}
          />
        </div>
        <fieldset className="space-y-3">
          <legend className="font-medium">受け入れ条件</legend>
          {rows.map((row, index) => (
            <div key={row.id} className="space-y-2 rounded border p-3" data-testid="criterion-row">
              <label htmlFor={`criterion-type-${row.id}`} className="block">
                条件 {index + 1} の種類
              </label>
              <select
                id={`criterion-type-${row.id}`}
                className={control}
                value={row.type}
                onChange={(event) =>
                  setRows((current) =>
                    current.map((item) =>
                      item.id === row.id ? { ...item, type: event.target.value as CriterionType } : item,
                    ),
                  )
                }
              >
                {criterionTypes.map((item) => (
                  <option key={item.type} value={item.type}>
                    {item.label}
                  </option>
                ))}
              </select>
              <label htmlFor={`criterion-value-${row.id}`} className="block">
                条件 {index + 1} の内容
              </label>
              <input
                id={`criterion-value-${row.id}`}
                className={control}
                value={row.value}
                onChange={(event) =>
                  setRows((current) =>
                    current.map((item) => (item.id === row.id ? { ...item, value: event.target.value } : item)),
                  )
                }
              />
              <Button
                data-testid="remove-criterion"
                onClick={() => setRows((current) => current.filter((item) => item.id !== row.id))}
              >
                条件を削除
              </Button>
            </div>
          ))}
          <Button
            data-testid="add-criterion"
            onClick={() => setRows((current) => [...current, { id: nextRow.current++, type: "human", value: "" }])}
          >
            条件を追加
          </Button>
        </fieldset>
        <Button type="submit" disabled={sender.pending}>
          タスクを作成
        </Button>
        {result?.status !== 422 && <ActionResultView result={result} />}
      </form>
    </ScreenFrame>
  );
}

export function PlanCreateScreen() {
  const navigate = useNavigate();
  const sender = useActionResult(taskKeys.all);
  const [goal, setGoal] = useState("");
  const result = sender.results["create-plan"];
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const id = "create-plan";
    // ADR-0079 U-R6: POST /plans は 410。計画（root task）は POST /tasks で作り、分解は
    // Complexity Gate と planner に任せる（ここでは stages_hint を付けない）。goal をそのまま
    // objective と reviewer 条件にし、title は先頭 80 文字。
    const title = goal.trim().slice(0, 80) || goal;
    const body: NewTaskSpec = {
      title,
      objective: goal,
      acceptance: [{ type: "reviewer", text: goal }],
    };
    const [outcome] = await sender.run([{ id, path: "/api/tasks", body }]);
    if (outcome?.ok) {
      const taskId = createdId(outcome.response);
      if (taskId) void navigate({ to: "/tasks/$id", params: { id: taskId } });
    }
  }
  return (
    <ScreenFrame title="計画の作成" route="/plans/new">
      <form onSubmit={submit} className="max-w-3xl space-y-5" data-testid="new-plan-form">
        <div>
          <label htmlFor="plan-goal" className="block font-medium">
            目標
          </label>
          <textarea
            id="plan-goal"
            aria-label="目標"
            className={control}
            rows={5}
            value={goal}
            onChange={(event) => setGoal(event.target.value)}
            aria-describedby={result?.status === 422 ? "plan-create-error" : undefined}
          />
          {result?.status === 422 && <ActionResultView result={result} fieldId="plan-create-error" />}
        </div>
        <Button type="submit" disabled={sender.pending}>
          計画を作成
        </Button>
        {result?.status !== 422 && <ActionResultView result={result} />}
      </form>
    </ScreenFrame>
  );
}
