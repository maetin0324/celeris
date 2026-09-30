import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { isApiError } from "../../api/client";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { type FilesSearch, parentPath, taskTreeFileQuery, taskTreeQuery } from "./task-files-query";

// /tasks/:id/files（P3-11、R24）。作業ツリーの閲覧。repo・path・file は URL の search param。
// 不正な path（403 path_forbidden など）は枠の中で error として出す。長い path・長い行は枠の中で折り返す・scroll する。
export function TaskFilesScreen({ taskId, search }: { taskId: string; search: FilesSearch }) {
  return (
    <ScreenFrame title={`作業ツリーと成果物 ${taskId}`} route="/tasks/:id/files">
      <p className="text-sm">
        <Link to="/tasks/$id" params={{ id: taskId }} className="inline-flex min-h-11 items-center underline">
          タスクの詳細へ
        </Link>
      </p>
      <TreePane taskId={taskId} search={search} />
      {search.file ? <FilePane taskId={taskId} search={search} file={search.file} /> : null}
    </ScreenFrame>
  );
}

function describeError(error: unknown): string {
  if (isApiError(error)) {
    if (error.kind === "forbidden") return "この path は作業ツリーの外か、読めない場所です。";
    if (error.kind === "not_found") return "作業ツリーまたは file が見つかりません。";
    if (error.kind === "validation") return "path が正しくありません。";
  }
  return "取得できませんでした。";
}

function TreePane({ taskId, search }: { taskId: string; search: FilesSearch }) {
  const tree = useQuery(taskTreeQuery(taskId, search.repo, search.path));
  if (tree.isError && tree.data === undefined)
    return (
      <p role="alert" data-testid="files-error" className="min-w-0 break-all">
        {describeError(tree.error)}（{search.path ?? "/"}）
      </p>
    );
  const up = parentPath(search.path);
  return (
    <FetchFrame query={tree}>
      {tree.data ? (
        <section aria-label="作業ツリー" data-testid="files-tree" className="min-w-0">
          {tree.data.repos.length > 1 ? (
            <ul className="flex flex-wrap gap-2 text-sm">
              {tree.data.repos.map((repo) => (
                <li key={repo.name}>
                  <Link
                    to="/tasks/$id/files"
                    params={{ id: taskId }}
                    search={{ repo: repo.name }}
                    aria-current={repo.name === tree.data.repo ? "page" : undefined}
                    className="inline-flex min-h-11 items-center underline"
                  >
                    {repo.name}
                  </Link>
                </li>
              ))}
            </ul>
          ) : null}
          <p className="min-w-0 break-all font-mono text-sm" data-testid="files-path">
            {tree.data.repo}:/{tree.data.path}
          </p>
          <ul className="min-w-0">
            {search.path ? (
              <li>
                <Link
                  to="/tasks/$id/files"
                  params={{ id: taskId }}
                  search={{ repo: search.repo, path: up }}
                  className="inline-flex min-h-11 items-center underline"
                >
                  ..
                </Link>
              </li>
            ) : null}
            {tree.data.entries.map((entry) => (
              <li key={entry.path} className="min-w-0 break-all">
                <Link
                  to="/tasks/$id/files"
                  params={{ id: taskId }}
                  search={
                    entry.kind === "dir"
                      ? { repo: search.repo, path: entry.path }
                      : { repo: search.repo, path: search.path, file: entry.path }
                  }
                  data-entry={entry.path}
                  aria-current={entry.path === search.file ? "page" : undefined}
                  className="inline-flex min-h-11 items-center font-mono text-sm underline"
                >
                  {entry.kind === "dir" ? `${entry.name}/` : entry.name}
                </Link>
                {entry.size != null ? <span className="ml-2 text-xs text-neutral-600">{entry.size} B</span> : null}
              </li>
            ))}
          </ul>
          {tree.data.entries.length === 0 ? <p>このディレクトリは空です。</p> : null}
        </section>
      ) : null}
    </FetchFrame>
  );
}

function FilePane({ taskId, search, file }: { taskId: string; search: FilesSearch; file: string }) {
  const body = useQuery(taskTreeFileQuery(taskId, file, search.repo));
  if (body.isError && body.data === undefined)
    return (
      <p role="alert" data-testid="files-error" className="min-w-0 break-all">
        {describeError(body.error)}（{file}）
      </p>
    );
  return (
    <FetchFrame query={body}>
      {body.data ? (
        <section aria-label="file の本文" data-testid="files-body" className="min-w-0">
          <h2 className="min-w-0 break-all font-mono text-sm font-semibold">{body.data.path}</h2>
          {body.data.too_large ? (
            <p data-testid="files-too-large">大きすぎるため表示しません（{body.data.size} B）。</p>
          ) : body.data.binary ? (
            <p data-testid="files-binary">バイナリのため表示しません（{body.data.size} B）。</p>
          ) : (
            <pre className="max-h-[70vh] min-w-0 max-w-full overflow-auto whitespace-pre-wrap break-all rounded border p-3 text-xs">
              {body.data.text ?? ""}
            </pre>
          )}
        </section>
      ) : null}
    </FetchFrame>
  );
}
