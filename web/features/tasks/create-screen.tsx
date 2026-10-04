import { useNavigate } from "@tanstack/react-router";
import { type FormEvent, useRef, useState } from "react";
import type { CriterionSpec, NewTaskBody, Task } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { Panel } from "../../components/ui/panel";

// 入力欄の枠は --color-input（区切りの border より濃い 3:1 以上の境界）。
const control =
  "min-h-11 w-full rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const fieldLabel = "block text-label font-medium text-foreground";
const fieldHint = "mt-1 text-label text-muted-foreground";
type CriterionType = "command" | "artifact_exists" | "reviewer" | "human";
type Row = { id: number; type: CriterionType; value: string };
// option の値と文字は API の型名のまま（parity e2e が selectOption で選ぶ）。説明は入力欄の下に出す。
const criterionTypes: { type: CriterionType; label: string; hint: string }[] = [
  { type: "command", label: "command", hint: "コマンド。終了コード 0 で合格（例: cargo test）" },
  { type: "artifact_exists", label: "artifact_exists", hint: "run が残すべき成果物の名前" },
  { type: "reviewer", label: "reviewer", hint: "レビュアーが確かめる内容" },
  { type: "human", label: "human", hint: "人が確かめる内容" },
];

function criterionHint(type: CriterionType): string {
  return criterionTypes.find((item) => item.type === type)?.hint ?? "";
}

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
    const body: NewTaskBody = { title, objective, acceptance: buildCriteria(rows) };
    const [outcome] = await sender.run([{ id, path: "/api/tasks", body }]);
    if (outcome?.ok) {
      const taskId = createdId(outcome.response);
      if (taskId) void navigate({ to: "/tasks/$id", params: { id: taskId } });
    }
  }
  return (
    <ScreenFrame title="タスクの作成" route="/tasks/new">
      <form onSubmit={submit} className="flex max-w-form min-w-0 flex-col gap-4" data-testid="new-task-form">
        <Panel title="内容">
          <div className="flex flex-col gap-4">
            <div>
              <label htmlFor="task-title" className={fieldLabel}>
                名前
              </label>
              <input
                id="task-title"
                className={`${control} mt-1`}
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                aria-describedby={result?.status === 422 ? errorId : undefined}
                aria-invalid={result?.status === 422 ? true : undefined}
              />
              {result?.status === 422 && (
                <div className="mt-1 text-label">
                  <ActionResultView result={result} fieldId={errorId} />
                </div>
              )}
            </div>
            <div>
              <label htmlFor="task-objective" className={fieldLabel}>
                目的
              </label>
              <textarea
                id="task-objective"
                aria-label="目的"
                className={`${control} mt-1 text-body`}
                rows={4}
                value={objective}
                onChange={(event) => setObjective(event.target.value)}
                aria-describedby={result?.status === 422 ? errorId : undefined}
              />
            </div>
          </div>
        </Panel>
        <Panel title="受け入れ条件">
          <fieldset className="flex min-w-0 flex-col gap-3">
            <legend className="sr-only">受け入れ条件</legend>
            <p className={fieldHint}>空の内容の条件は送りません。条件が 0 件でも作成できます。</p>
            {rows.map((row, index) => (
              <div
                key={row.id}
                className="min-w-0 rounded-md border border-border bg-surface p-3"
                data-testid="criterion-row"
              >
                <div className="flex min-w-0 flex-col gap-3 md:flex-row md:items-start">
                  <div className="md:w-48 md:shrink-0">
                    <label htmlFor={`criterion-type-${row.id}`} className={fieldLabel}>
                      条件 {index + 1} の種類
                    </label>
                    <select
                      id={`criterion-type-${row.id}`}
                      className={`${control} mt-1 font-mono`}
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
                  </div>
                  <div className="min-w-0 flex-1">
                    <label htmlFor={`criterion-value-${row.id}`} className={fieldLabel}>
                      条件 {index + 1} の内容
                    </label>
                    <input
                      id={`criterion-value-${row.id}`}
                      className={`${control} mt-1 ${row.type === "command" || row.type === "artifact_exists" ? "font-mono" : ""}`}
                      value={row.value}
                      aria-describedby={`criterion-hint-${row.id}`}
                      onChange={(event) =>
                        setRows((current) =>
                          current.map((item) => (item.id === row.id ? { ...item, value: event.target.value } : item)),
                        )
                      }
                    />
                    <p id={`criterion-hint-${row.id}`} className={fieldHint}>
                      {criterionHint(row.type)}
                    </p>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="self-end md:mt-6 md:self-start"
                    data-testid="remove-criterion"
                    onClick={() => setRows((current) => current.filter((item) => item.id !== row.id))}
                  >
                    条件を削除
                  </Button>
                </div>
              </div>
            ))}
            <div>
              <Button
                data-testid="add-criterion"
                onClick={() => setRows((current) => [...current, { id: nextRow.current++, type: "human", value: "" }])}
              >
                条件を追加
              </Button>
            </div>
          </fieldset>
        </Panel>
        <div className="flex flex-col gap-2">
          <div>
            <Button type="submit" variant="primary" disabled={sender.pending}>
              タスクを作成
            </Button>
          </div>
          {result?.status !== 422 && <ActionResultView result={result} />}
        </div>
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
    const body: NewTaskBody = {
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
      <form onSubmit={submit} className="flex max-w-form min-w-0 flex-col gap-4" data-testid="new-plan-form">
        <div>
          <label htmlFor="plan-goal" className={fieldLabel}>
            目標
          </label>
          <textarea
            id="plan-goal"
            aria-label="目標"
            className={`${control} mt-1`}
            rows={5}
            value={goal}
            onChange={(event) => setGoal(event.target.value)}
            aria-describedby={result?.status === 422 ? "plan-create-error" : undefined}
          />
          {result?.status === 422 && <ActionResultView result={result} fieldId="plan-create-error" />}
        </div>
        <div>
          <Button type="submit" variant="primary" disabled={sender.pending}>
            計画を作成
          </Button>
        </div>
        {result?.status !== 422 && <ActionResultView result={result} />}
      </form>
    </ScreenFrame>
  );
}
