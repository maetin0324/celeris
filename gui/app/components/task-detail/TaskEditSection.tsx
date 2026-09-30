import { useState } from "react";
import { useFetcher } from "react-router";
import type { TaskEditOutcome } from "~/celeris/action-types";
import type { MilestoneView, OrgNode, TaskDetail } from "~/celeris/types";
import { TaskEditFlash } from "~/components/Flash";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { chipLabelClass, hintClass, inputClass, labelClass, selectClass, textareaClass } from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { isValidLabel, MAX_LABELS, PRIORITY_LABELS } from "~/lib/board";
import {
  harnessOptions,
  milestoneStatusLabel,
  priorityFullLabel,
  TASK_CATEGORIES,
  TASK_MODES,
  TIERS,
  taskCategoryLabel,
  taskModeLabel,
  tierLabel,
} from "~/lib/labels";
import { cn } from "~/lib/utils";

/** 空白・カンマ区切りの id 列を配列にする（`tasks.new.tsx` の `depends_on_extra` と同じ扱い）。 */
function splitIds(text: string): string[] {
  return Array.from(
    new Set(
      text
        .split(/[\s,]+/)
        .map((s) => s.trim())
        .filter((s) => s !== ""),
    ),
  );
}

/**
 * タスクの編集フォーム（ADR-0044 D1、Phase 53）。**GUI 側では検証しない**のが原則だが、ラベルの形
 * （小文字・`[a-z0-9-]`・最大 8 個）だけはチップを足すときの入力補助として弾く（正は celeris の 422）。
 * `running` / `reviewing` でも編集できる（走っている run は止まらず、次の run から効く）。
 */
export function TaskEditSection({
  detail,
  org,
  milestones,
  genres,
}: {
  detail: TaskDetail;
  org: OrgNode[];
  milestones: MilestoneView[];
  /** ADR-0046 D3（Phase 59）: ハーネス（`genre` 列）の選択肢。空なら自由記述にする。 */
  genres: string[];
}) {
  const { task } = detail;
  const harnesses = harnessOptions(genres);
  const fetcher = useFetcher<TaskEditOutcome>({ key: `task-edit-${task.id}` });
  const busy = fetcher.state !== "idle";
  const [labels, setLabels] = useState<string[]>(task.labels ?? []);
  const [labelDraft, setLabelDraft] = useState("");
  const [labelError, setLabelError] = useState<string | null>(null);
  const [dependsOnText, setDependsOnText] = useState(() => (task.depends_on ?? []).join(" "));
  const dependsOnIds = splitIds(dependsOnText);

  function addLabel() {
    const value = labelDraft.trim().toLowerCase();
    if (value === "") return;
    if (!isValidLabel(value)) {
      setLabelError("ラベルは小文字の英数字とハイフン（a-z 0-9 -）だけです。");
      return;
    }
    if (labels.includes(value)) {
      setLabelDraft("");
      return;
    }
    if (labels.length >= MAX_LABELS) {
      setLabelError(`ラベルは ${MAX_LABELS} 個までです。`);
      return;
    }
    setLabels([...labels, value]);
    setLabelDraft("");
    setLabelError(null);
  }

  return (
    <section aria-labelledby="edit-heading" data-testid="edit-section">
      <Card>
        <CardHeader
          icon="settings"
          tone="primary"
          title={
            <h2 id="edit-heading" className="text-[0.95rem] font-semibold text-fg">
              編集
            </h2>
          }
          description="書き換えた項目だけが変わります。作業中でも変えられますが、走っている run は止まりません（次の run から効きます）。"
        />
        <CardBody className="space-y-4">
          <TaskEditFlash
            outcome={fetcher.data}
            runningNote={task.status === "running" || task.status === "reviewing"}
          />
          <fetcher.Form method="post" data-testid="task-edit-form" className="space-y-4">
            <input type="hidden" name="intent" value="edit" />
            <input type="hidden" name="expected_status" value={task.status} />

            <div>
              <label htmlFor="edit-title" className={labelClass}>
                題名
              </label>
              <input
                id="edit-title"
                name="title"
                type="text"
                defaultValue={task.title}
                data-testid="edit-title"
                className={cn(inputClass, "mt-1.5")}
              />
            </div>

            <div>
              <label htmlFor="edit-objective" className={labelClass}>
                目的
              </label>
              <textarea
                id="edit-objective"
                name="objective"
                rows={4}
                defaultValue={task.objective}
                data-testid="edit-objective"
                className={cn(textareaClass, "mt-1.5")}
              />
            </div>

            <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
              <div>
                <label htmlFor="edit-tier" className={labelClass}>
                  担当エージェントのレベル
                </label>
                <select
                  id="edit-tier"
                  name="tier"
                  defaultValue={task.worker_hint.tier}
                  data-testid="edit-tier"
                  className={cn(selectClass, "mt-1.5")}
                >
                  {TIERS.map((t) => (
                    <option key={t} value={t}>
                      {tierLabel(t)}
                    </option>
                  ))}
                </select>
              </div>

              <div>
                <label htmlFor="edit-priority" className={labelClass}>
                  優先度
                </label>
                <select
                  id="edit-priority"
                  name="priority"
                  defaultValue={detail.priority_label}
                  data-testid="edit-priority"
                  className={cn(selectClass, "mt-1.5")}
                >
                  {PRIORITY_LABELS.map((p) => (
                    <option key={p} value={p}>
                      {priorityFullLabel(p)}
                    </option>
                  ))}
                </select>
              </div>

              <div>
                <label htmlFor="edit-category" className={labelClass}>
                  種類
                </label>
                <select
                  id="edit-category"
                  name="category"
                  defaultValue={task.category ?? "other"}
                  data-testid="edit-category"
                  className={cn(selectClass, "mt-1.5")}
                >
                  {TASK_CATEGORIES.map((c) => (
                    <option key={c} value={c}>
                      {taskCategoryLabel(c)}
                    </option>
                  ))}
                </select>
              </div>

              <div>
                <label htmlFor="edit-assignee" className={labelClass}>
                  担当
                </label>
                <select
                  id="edit-assignee"
                  name="assignee"
                  defaultValue={task.assignee ?? ""}
                  data-testid="edit-assignee"
                  className={cn(selectClass, "mt-1.5")}
                >
                  <option value="">（決めない）</option>
                  {org.map((node) => (
                    <option key={node.id} value={node.id}>
                      {node.name}
                    </option>
                  ))}
                </select>
                {org.length === 0 && <p className={cn(hintClass, "mt-1")}>組織を読めませんでした。</p>}
              </div>

              <div>
                <label htmlFor="edit-milestone" className={labelClass}>
                  途中目標
                </label>
                <select
                  id="edit-milestone"
                  name="milestone_id"
                  defaultValue={task.milestone_id ?? ""}
                  data-testid="edit-milestone"
                  className={cn(selectClass, "mt-1.5")}
                >
                  <option value="">（なし）</option>
                  {milestones
                    .slice()
                    .sort((a, b) => a.seq - b.seq)
                    .map((m) => (
                      <option key={m.id} value={m.id}>
                        #{m.seq} {m.title}（{milestoneStatusLabel(m.status)}）
                      </option>
                    ))}
                </select>
                {milestones.length === 0 && <p className={cn(hintClass, "mt-1")}>この案件には途中目標がありません。</p>}
              </div>

              {/* ADR-0046 D3（Phase 59）: ハーネス（`Task.genre` 列がそのまま harness id）。空の選択肢を
                  選べば `null`（外す）を送る（`NULLABLE_FIELDS`。§3.74）。 */}
              <div>
                <label htmlFor="edit-harness" className={labelClass}>
                  ハーネス
                </label>
                {harnesses.length > 0 ? (
                  <select
                    id="edit-harness"
                    name="harness"
                    defaultValue={task.genre ?? ""}
                    data-testid="edit-harness"
                    className={cn(selectClass, "mt-1.5")}
                  >
                    <option value="">（決めない）</option>
                    {harnesses.map((h) => (
                      <option key={h} value={h}>
                        {h}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    id="edit-harness"
                    name="harness"
                    type="text"
                    defaultValue={task.genre ?? ""}
                    data-testid="edit-harness"
                    className={cn(inputClass, "mt-1.5")}
                  />
                )}
                <p className={hintClass}>担当が無ければ、これと能力タグの重なりで決まります（ADR-0046 D5）。</p>
              </div>

              {/* ADR-0046 D2（Phase 59）: 必要な能力タグ（開いた語彙。空白/カンマ区切り）。 */}
              <div>
                <label htmlFor="edit-skills" className={labelClass}>
                  能力タグ
                </label>
                <input
                  id="edit-skills"
                  name="skills"
                  type="text"
                  defaultValue={(task.skills ?? []).join(", ")}
                  data-testid="edit-skills"
                  className={cn(inputClass, "mt-1.5")}
                />
                <p className={hintClass}>空白かカンマ区切り（例: rust, benchmark）。小文字の `[a-z0-9._-]` だけ。</p>
              </div>

              {/* ADR-0046 D4（Phase 59）: 進め方。前置きの規則とレビューの厳しさが変わる。 */}
              <div>
                <label htmlFor="edit-mode" className={labelClass}>
                  進め方
                </label>
                <select
                  id="edit-mode"
                  name="mode"
                  defaultValue={task.mode ?? "production"}
                  data-testid="edit-mode"
                  className={cn(selectClass, "mt-1.5")}
                >
                  {TASK_MODES.map((m) => (
                    <option key={m} value={m}>
                      {taskModeLabel(m)}
                    </option>
                  ))}
                </select>
              </div>
            </div>

            {/* ラベル（ADR-0044 D3）。送るのは hidden の `labels`（空の番兵で「差し替える意思」を示す）。 */}
            <fieldset data-testid="edit-labels">
              <legend className={labelClass}>ラベル</legend>
              <input type="hidden" name="labels" value="" />
              {labels.map((label) => (
                <input key={label} type="hidden" name="labels" value={label} />
              ))}
              <div className="mt-1.5 flex flex-wrap items-center gap-2">
                {labels.map((label) => (
                  <span key={label} className={cn(chipLabelClass, "cursor-default")} data-testid="edit-label-chip">
                    {label}
                    <button
                      type="button"
                      aria-label={`ラベル ${label} を外す`}
                      onClick={() => setLabels(labels.filter((l) => l !== label))}
                      className="text-fg-subtle hover:text-danger"
                    >
                      <Icon name="x" className="size-3.5" />
                    </button>
                  </span>
                ))}
                <input
                  type="text"
                  value={labelDraft}
                  aria-label="ラベルを足す"
                  placeholder="例: pluvio"
                  data-testid="edit-label-input"
                  onChange={(e) => {
                    setLabelDraft(e.target.value);
                    if (labelError) setLabelError(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      e.preventDefault();
                      addLabel();
                    }
                  }}
                  className={cn(inputClass, "w-40")}
                />
                <Button type="button" variant="ghost" size="xs" onClick={addLabel} data-testid="edit-label-add">
                  <Icon name="plus" />
                  足す
                </Button>
              </div>
              {labelError && (
                <p role="alert" className="mt-1 text-sm text-danger lg:text-xs" data-testid="edit-label-error">
                  {labelError}
                </p>
              )}
              <p className={cn(hintClass, "mt-1")}>小文字の英数字とハイフンだけ、最大 {MAX_LABELS} 個。</p>
            </fieldset>

            <div>
              <label htmlFor="edit-depends-on" className={labelClass}>
                依存（depends_on）
              </label>
              <input type="hidden" name="depends_on" value="" />
              {dependsOnIds.map((id) => (
                <input key={id} type="hidden" name="depends_on" value={id} />
              ))}
              <input
                id="edit-depends-on"
                type="text"
                value={dependsOnText}
                onChange={(e) => setDependsOnText(e.target.value)}
                data-testid="edit-depends-on"
                className={cn(inputClass, "mt-1.5 font-mono text-xs")}
              />
              <p className={cn(hintClass, "mt-1")}>
                タスク id を空白かカンマで区切って書きます（差し替え。空にすると依存が無くなります）。
              </p>
            </div>

            <Button type="submit" variant="primary" size="sm" disabled={busy} data-testid="task-edit-submit">
              <Icon name="check" />
              保存
            </Button>
          </fetcher.Form>
        </CardBody>
      </Card>
    </section>
  );
}
