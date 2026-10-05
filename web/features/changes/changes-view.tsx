import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { ChangedFile, IntegrateBody, IntegrateResult, RepoChangesView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button, buttonVariants } from "../../components/ui/button";
import { CodeBlock } from "../../components/ui/code-block";
import { DataList } from "../../components/ui/data-list";
import { Icon } from "../../components/ui/icon";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { taskChangeDiffQuery, taskChangesQuery, taskRepoChangesPath } from "./changes-query";
import { type DiffLineKind, diffLines, fileStatusView, integrationLabel, integrationTone } from "./diff-lines";

// /tasks/:id/changes（P3-13、R25）。変更の一覧・差分と取り込み（integrate、pr_merge）。
// 同じ部品を /tasks/:id?tab=changes に置く。衝突・git の失敗は 200 で返るので integration.state で表示を分ける。
// 差分は CodeBlock の内側でだけ横に scroll し、画面は横に溢れさせない。path は折り返し、全文を title に持つ。
export function TaskChangesScreen({ taskId }: { taskId: string }) {
  return (
    <ScreenFrame title={`変更 ${taskId}`} route="/tasks/:id/changes">
      <p className="text-label">
        <Link
          to="/tasks/$id"
          params={{ id: taskId }}
          className="inline-flex min-h-11 items-center gap-1 text-primary underline underline-offset-2"
        >
          <Icon name="chevron-left" size="sm" />
          タスクの詳細へ
        </Link>
      </p>
      <TaskChangesPanel taskId={taskId} />
    </ScreenFrame>
  );
}

export function TaskChangesPanel({ taskId }: { taskId: string }) {
  const changes = useQuery(taskChangesQuery(taskId));
  return (
    <FetchFrame query={changes} subject="変更">
      {changes.data ? (
        <div data-testid="task-changes" className="min-w-0 space-y-4">
          {changes.data.repos.length === 0 ? (
            <div data-testid="changes-empty">
              <EmptyState message="git のリポジトリの変更はありません。" />
            </div>
          ) : (
            changes.data.repos.map((repo) => (
              <RepoChanges key={repo.repo} taskId={taskId} repo={repo} mergeMethod={changes.data.merge_method} />
            ))
          )}
        </div>
      ) : null}
    </FetchFrame>
  );
}

function RepoChanges({ taskId, repo, mergeMethod }: { taskId: string; repo: RepoChangesView; mergeMethod: string }) {
  const [selected, setSelected] = useState<string | null>(null);
  const integration = repo.integration;
  const changedLines = repo.stat.additions + repo.stat.deletions;
  return (
    <Section
      title={<span className="break-all">{repo.repo}</span>}
      aria-label={`リポジトリ ${repo.repo}`}
      data-repo={repo.repo}
      className="rounded-lg border border-border bg-surface p-3 md:p-4"
    >
      <div className="mb-3 flex min-w-0 flex-wrap items-center gap-2 text-label" role="status">
        <Badge tone={repo.missing ? "danger" : repo.dirty ? "warning" : "neutral"}>{repo.files.length} ファイル</Badge>
        <span className="font-mono tabular-nums text-success-foreground">+{repo.stat.additions} 行</span>
        <span className="font-mono tabular-nums text-danger-foreground">−{repo.stat.deletions} 行</span>
        {changedLines > 0 ? <span className="text-muted-foreground">計 {changedLines} 行の変更</span> : null}
      </div>
      <DataList
        items={[
          {
            label: "ブランチ",
            value: (
              <span className="break-all font-mono">
                {repo.branch} → {repo.default_branch}
              </span>
            ),
          },
          {
            label: "先行",
            value: (
              <span className="flex flex-wrap items-center gap-2">
                <span className="tabular-nums">{repo.ahead} commit</span>
                {repo.dirty ? <Badge tone="warning">未コミットあり</Badge> : null}
                {repo.missing ? <Badge tone="danger">作業ツリーなし</Badge> : null}
              </span>
            ),
          },
          ...(integration
            ? [
                {
                  label: "取り込み",
                  value: (
                    <span data-testid="integration-state" className="flex min-w-0 flex-wrap items-center gap-2">
                      <Badge tone={integrationTone(integration.state)}>{integrationLabel(integration.state)}</Badge>
                      <span className="text-label text-muted-foreground">
                        {integration.method} / {integration.state}
                      </span>
                      {integration.pr_url ? (
                        <a
                          href={integration.pr_url}
                          className="inline-flex min-h-11 min-w-0 items-center gap-1 break-all text-primary underline underline-offset-2"
                        >
                          {integration.pr_url}
                          <Icon name="external" size="sm" />
                        </a>
                      ) : null}
                      {integration.detail ? <span className="min-w-0 break-words">{integration.detail}</span> : null}
                    </span>
                  ),
                },
              ]
            : []),
        ]}
      />
      <h3 className="mt-4 text-body font-semibold text-foreground">変わったファイル（{repo.files.length}）</h3>
      {repo.files.length === 0 ? (
        <EmptyState message="変わったファイルはありません。" />
      ) : (
        <ChangedFiles
          files={repo.files}
          selected={selected}
          onSelect={(path) => setSelected(selected === path ? null : path)}
        />
      )}
      {selected ? <DiffPane taskId={taskId} repo={repo.repo} path={selected} /> : null}
      {repo.missing ? null : <IntegrateForm taskId={taskId} repo={repo} mergeMethod={mergeMethod} />}
    </Section>
  );
}

// 変更ファイルの表。path は折り返して列を広げず、全文は title に持つ。行の選択は aria-pressed の button で伝える。
function ChangedFiles({
  files,
  selected,
  onSelect,
}: {
  files: ChangedFile[];
  selected: string | null;
  onSelect: (path: string) => void;
}) {
  return (
    <Table data-testid="changed-files">
      <TableHeader>
        <TableRow>
          <TableHead className="w-px">状態</TableHead>
          <TableHead>ファイル</TableHead>
          <TableHead className="w-px text-right">行</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {files.map((file) => {
          const status = fileStatusView(file.status);
          const pressed = selected === file.path;
          return (
            <TableRow key={file.path} data-state={pressed ? "checked" : undefined}>
              <TableCell className="whitespace-nowrap">
                <Badge tone={status.tone} title={`status ${file.status}`} className="break-normal whitespace-nowrap">
                  {status.label}
                </Badge>
              </TableCell>
              <TableCell className="min-w-0">
                <button
                  type="button"
                  data-file={file.path}
                  title={file.path}
                  aria-pressed={pressed}
                  onClick={() => onSelect(file.path)}
                  className="inline-flex min-h-11 min-w-0 items-center break-all text-left font-mono text-label text-primary underline underline-offset-2"
                >
                  {file.path}
                </button>
              </TableCell>
              <TableCell className="whitespace-nowrap text-right font-mono tabular-nums">
                {file.binary ? (
                  <span className="text-muted-foreground">binary</span>
                ) : (
                  <span className="flex flex-col items-end">
                    <span className="text-success-foreground">
                      <span className="sr-only">追加 </span>+{file.additions}
                    </span>
                    <span className="text-danger-foreground">
                      <span className="sr-only">削除 </span>−{file.deletions}
                    </span>
                  </span>
                )}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

const lineClass: Record<DiffLineKind, string> = {
  add: "bg-success text-success-foreground",
  remove: "bg-danger text-danger-foreground",
  hunk: "bg-info text-info-foreground",
  meta: "text-muted-foreground",
  context: "",
};

function DiffPane({ taskId, repo, path }: { taskId: string; repo: string; path: string }) {
  const diff = useQuery(taskChangeDiffQuery(taskId, repo, path));
  return (
    <div data-testid="change-diff" className="mt-4 min-w-0 space-y-2">
      <h3 className="min-w-0 break-all text-body font-semibold text-foreground" title={path}>
        差分 <span className="font-mono">{path}</span>
      </h3>
      <FetchFrame query={diff} subject="差分">
        {diff.data ? (
          <div className="min-w-0 space-y-2">
            {diff.data.truncated ? (
              <p
                role="status"
                className="border-l-2 border-warning-foreground bg-warning px-3 py-2 text-label text-warning-foreground"
              >
                200 KiB を超えたので途中で切っています。
              </p>
            ) : null}
            {diff.data.diff === "" ? (
              <EmptyState message="差分はありません。" />
            ) : (
              <>
                <p className="text-label text-muted-foreground">
                  <span className="text-success-foreground">+ 追加</span> /{" "}
                  <span className="text-danger-foreground">− 削除</span>
                </p>
                <CodeBlock label={`差分 ${path}`} data-testid="change-diff-body" className="max-h-96 overflow-y-auto">
                  {/* 行の背景を最長の行の幅まで伸ばすため、行を w-max の grid に並べる。 */}
                  <span className="grid w-max min-w-full">
                    {diffLines(diff.data.diff).map((line, index) => (
                      <span
                        // 行は位置で一意。差分の原文は並べ替わらない。
                        // biome-ignore lint/suspicious/noArrayIndexKey: 同じ文字列の行が繰り返し現れる。
                        key={index}
                        data-line={line.kind}
                        className={`block px-1 ${lineClass[line.kind]}`}
                      >
                        {line.text === "" ? " " : line.text}
                        {"\n"}
                      </span>
                    ))}
                  </span>
                </CodeBlock>
              </>
            )}
          </div>
        ) : null}
      </FetchFrame>
    </div>
  );
}

const fieldClass = "min-h-11 w-full rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground";

function IntegrateForm({ taskId, repo, mergeMethod }: { taskId: string; repo: RepoChangesView; mergeMethod: string }) {
  const [method, setMethod] = useState<IntegrateBody["method"]>("merge");
  const [note, setNote] = useState("");
  const [confirm, setConfirm] = useState(false);
  const [lastAction, setLastAction] = useState<"integrate" | "pr_merge" | null>(null);
  const sender = useActionResult(taskKeys.changes(taskId));
  const base = taskRepoChangesPath(taskId, repo.repo);
  const integration = repo.integration;
  const prOpen = integration?.method === "pr" && integration.state === "open" && integration.pr_number != null;
  const latest = lastAction ? sender.results[lastAction] : undefined;
  const outcome = latest?.ok ? (latest.response as IntegrateResult | undefined) : undefined;

  function integrate() {
    const body: IntegrateBody = { method };
    if (note.trim()) body.note = note.trim();
    if (method === "discard" && confirm) body.confirm = true;
    setLastAction("integrate");
    void sender.run([{ id: "integrate", path: `${base}/integrate`, body }]);
  }

  return (
    <fieldset className="mt-4 min-w-0 space-y-3">
      <legend className="w-full border-t border-border pt-4 text-body font-semibold text-foreground">取り込み</legend>
      <p className="text-label text-muted-foreground">
        取り込む前に、上のファイルと対象ブランチを確認してください。範囲外の変更かどうかは、この一覧では判定できません。
      </p>
      <label className="flex flex-col gap-1 text-label font-medium text-foreground">
        取り込みの方法
        <select
          className={fieldClass}
          value={method}
          onChange={(event) => setMethod(event.target.value as IntegrateBody["method"])}
        >
          <option value="merge">merge（default_branch へ）</option>
          <option value="pr">PR を作る</option>
          <option value="discard">破棄</option>
        </select>
      </label>
      <label className="flex flex-col gap-1 text-label font-medium text-foreground">
        取り込みの note（任意）
        <textarea
          aria-label="取り込みの note（任意）"
          className={fieldClass}
          rows={2}
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </label>
      {method === "discard" ? (
        <label className="flex min-h-11 items-center gap-2 text-label text-danger-foreground">
          <input
            type="checkbox"
            className="size-5 shrink-0"
            checked={confirm}
            onChange={(event) => setConfirm(event.target.checked)}
          />
          取り返しがつかないことを確認した
        </label>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button
          variant={method === "discard" ? "destructive" : "primary"}
          disabled={sender.pending}
          onClick={integrate}
        >
          取り込む
        </Button>
        {prOpen ? (
          <Button
            disabled={sender.pending}
            onClick={() => {
              setLastAction("pr_merge");
              void sender.run([{ id: "pr_merge", path: `${base}/pr/merge`, body: {} }]);
            }}
          >
            Celeris で merge（{mergeMethod}）
          </Button>
        ) : null}
      </div>
      <ActionResultView result={latest?.ok ? undefined : latest} />
      {outcome?.integration ? (
        <p
          role="status"
          data-testid="integrate-result"
          className="flex min-w-0 flex-wrap items-center gap-2 break-all text-label"
        >
          結果:
          <Badge tone={integrationTone(outcome.integration.state)}>{integrationLabel(outcome.integration.state)}</Badge>
          <span className="text-muted-foreground">{outcome.integration.state}</span>
          {outcome.integration.detail ? <span>{outcome.integration.detail}</span> : null}
          {outcome.child_task_id ? (
            <span>
              衝突の解消:{" "}
              <Link
                to="/tasks/$id"
                params={{ id: outcome.child_task_id }}
                className={buttonVariants({ variant: "ghost", size: "sm" })}
              >
                {outcome.child_task_id}
              </Link>
            </span>
          ) : null}
        </p>
      ) : null}
    </fieldset>
  );
}
