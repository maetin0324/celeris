import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { IntegrateBody, IntegrateResult, RepoChangesView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { taskChangeDiffQuery, taskChangesQuery, taskRepoChangesPath } from "./changes-query";

// /tasks/:id/changes（P3-13、R25）。変更の一覧・差分と取り込み（integrate、pr_merge）。
// 同じ部品を /tasks/:id?tab=changes に置く。衝突・git の失敗は 200 で返るので integration.state をそのまま出す。
// 差分の長い行は <pre> の中で横に scroll し、枠（画面）は横に溢れさせない。
export function TaskChangesScreen({ taskId }: { taskId: string }) {
  return (
    <ScreenFrame title={`変更 ${taskId}`} route="/tasks/:id/changes">
      <p className="text-sm">
        <Link to="/tasks/$id" params={{ id: taskId }} className="inline-flex min-h-11 items-center underline">
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
    <FetchFrame query={changes}>
      {changes.data ? (
        <div data-testid="task-changes" className="min-w-0 space-y-4">
          {changes.data.repos.length === 0 ? (
            <p data-testid="changes-empty">git のリポジトリの変更はありません。</p>
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
    <section
      aria-label={`リポジトリ ${repo.repo}`}
      data-repo={repo.repo}
      className="min-w-0 space-y-2 rounded border p-3"
    >
      <h2 className="min-w-0 break-all text-base font-semibold">{repo.repo}</h2>
      <p className="min-w-0 break-all text-sm">
        {repo.branch} → {repo.default_branch}（{repo.ahead} commit{repo.dirty ? "、未コミットあり" : ""}
        {repo.missing ? "、作業ツリーなし" : ""}）
      </p>
      {integration ? (
        <p data-testid="integration-state" className="min-w-0 break-all text-sm">
          取り込み: {integration.method} / {integration.state}
          {integration.pr_url ? ` / ${integration.pr_url}` : ""}
          {integration.detail ? ` / ${integration.detail}` : ""}
        </p>
      ) : null}
      {repo.files.length === 0 ? (
        <p>変わったファイルはありません。</p>
      ) : (
        <ul className="min-w-0 space-y-1" data-testid="changed-files">
          {repo.files.map((file) => (
            <li key={file.path} className="min-w-0 break-all">
              <button
                type="button"
                data-file={file.path}
                aria-pressed={selected === file.path}
                onClick={() => setSelected(selected === file.path ? null : file.path)}
                className="inline-flex min-h-11 items-center text-left font-mono text-sm underline"
              >
                {file.status} {file.path}
              </button>
              <span className="ml-2 text-xs text-neutral-600">
                {file.binary ? "binary" : `+${file.additions} -${file.deletions}`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {selected ? <DiffPane taskId={taskId} repo={repo.repo} path={selected} /> : null}
      {repo.missing ? null : (
        <fieldset className="min-w-0 space-y-2">
          <legend className="font-semibold">取り込み</legend>
          <label className="flex flex-col text-sm">
            取り込みの方法
            <select
              className="min-h-11 rounded border px-2"
              value={method}
              onChange={(event) => setMethod(event.target.value as IntegrateBody["method"])}
            >
              <option value="merge">merge（default_branch へ）</option>
              <option value="pr">PR を作る</option>
              <option value="discard">破棄</option>
            </select>
          </label>
          <label className="block text-sm">
            取り込みの note（任意）
            <textarea
              className="block min-h-11 w-full rounded border p-2"
              value={note}
              onChange={(event) => setNote(event.target.value)}
            />
          </label>
          {method === "discard" ? (
            <label className="flex min-h-11 items-center gap-2 text-sm">
              <input
                type="checkbox"
                className="size-11"
                checked={confirm}
                onChange={(event) => setConfirm(event.target.checked)}
              />
              取り返しがつかないことを確認した
            </label>
          ) : null}
          <Button disabled={sender.pending} onClick={integrate}>
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
          <ActionResultView result={latest?.ok ? undefined : latest} />
          {outcome?.integration ? (
            <p role="status" data-testid="integrate-result" className="min-w-0 break-all text-sm">
              結果: {outcome.integration.state}
              {outcome.integration.detail ? `（${outcome.integration.detail}）` : ""}
              {outcome.child_task_id ? (
                <>
                  {" "}
                  衝突の解消:{" "}
                  <Link to="/tasks/$id" params={{ id: outcome.child_task_id }} className="underline">
                    {outcome.child_task_id}
                  </Link>
                </>
              ) : null}
            </p>
          ) : null}
        </fieldset>
      )}
    </section>
  );
}

function DiffPane({ taskId, repo, path }: { taskId: string; repo: string; path: string }) {
  const diff = useQuery(taskChangeDiffQuery(taskId, repo, path));
  return (
    <FetchFrame query={diff}>
      {diff.data ? (
        <section aria-label={`差分 ${path}`} data-testid="change-diff" className="min-w-0">
          {diff.data.truncated ? <p>200 KiB を超えたので途中で切っています。</p> : null}
          {diff.data.diff === "" ? (
            <p>差分はありません。</p>
          ) : (
            <pre
              data-testid="change-diff-body"
              className="max-h-[70vh] min-w-0 max-w-full overflow-auto whitespace-pre rounded border p-3 text-xs"
            >
              {diff.data.diff}
            </pre>
          )}
        </section>
      ) : null}
    </FetchFrame>
  );
}
