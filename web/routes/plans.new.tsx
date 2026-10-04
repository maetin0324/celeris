import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import type { NewTaskBody, Task } from "../api/generated/types";
import { taskKeys } from "../api/queries/keys";
import { ActionResultView, useActionResult } from "../components/actions/use-action-result";
import { ScreenFrame } from "../components/shell/screen-frame";
import { Button } from "../components/ui/button";

// R28 /plans/new。計画（root task）の作成。入力欄の枠は --color-input（3:1）、失敗は欄の下に出す。
export const Route = createFileRoute("/plans/new")({
  component: PlanCreateScreen,
});

function createdId(response: unknown): string | null {
  if (!response || typeof response !== "object") return null;
  const id = (response as Partial<Task>).id;
  return typeof id === "string" && id !== "" ? id : null;
}

function PlanCreateScreen() {
  const navigate = useNavigate();
  const sender = useActionResult(taskKeys.all);
  const [goal, setGoal] = useState("");
  const result = sender.results["create-plan"];
  const invalid = result?.status === 422;
  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    // ADR-0079 U-R6: POST /plans は 410。計画（root task）は POST /tasks で作り、分解は
    // Complexity Gate と planner に任せる（ここでは stages_hint を付けない）。goal をそのまま
    // objective と reviewer 条件にし、title は先頭 80 文字。
    const title = goal.trim().slice(0, 80) || goal;
    const body: NewTaskBody = { title, objective: goal, acceptance: [{ type: "reviewer", text: goal }] };
    const [outcome] = await sender.run([{ id: "create-plan", path: "/api/tasks", body }]);
    if (outcome?.ok) {
      const taskId = createdId(outcome.response);
      if (taskId) void navigate({ to: "/tasks/$id", params: { id: taskId } });
    }
  }
  return (
    <ScreenFrame title="計画の作成" route="/plans/new">
      <form onSubmit={submit} className="flex max-w-form flex-col gap-4" data-testid="new-plan-form">
        <div className="flex flex-col gap-1">
          <label htmlFor="plan-goal" className="text-label font-medium">
            目標
          </label>
          <p id="plan-goal-help" className="max-w-prose text-label text-muted-foreground">
            達成したいことを文で書きます。分け方は planner が決め、完了はこの目標を基準にレビュアーが判定します。
          </p>
          <textarea
            id="plan-goal"
            aria-label="目標"
            className="block min-h-11 w-full rounded-md border border-input bg-surface p-2 text-body text-foreground aria-invalid:border-danger-foreground"
            rows={5}
            value={goal}
            onChange={(event) => setGoal(event.target.value)}
            aria-invalid={invalid || undefined}
            aria-describedby={invalid ? "plan-goal-help plan-create-error" : "plan-goal-help"}
          />
          {invalid && <ActionResultView result={result} fieldId="plan-create-error" />}
        </div>
        <div className="flex flex-wrap gap-2">
          <Button type="submit" variant="primary" disabled={sender.pending}>
            計画を作成
          </Button>
        </div>
        {!invalid && <ActionResultView result={result} />}
      </form>
    </ScreenFrame>
  );
}
