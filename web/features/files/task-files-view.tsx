import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { isApiError } from "../../api/client";
import { EmptyState, FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { CodeBlock } from "../../components/ui/code-block";
import { Icon } from "../../components/ui/icon";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { type FilesSearch, formatBytes, parentPath, taskTreeFileQuery, taskTreeQuery } from "./task-files-query";

// /tasks/:id/files（P3-11、R24）。作業ツリーの閲覧。repo・path・file は URL の search param。
// 一覧は Table、本文は CodeBlock（横は枠の内側でだけ scroll）。長い path は折り返し、全文は title 属性に置く。
// 不正な path（403 path_forbidden など）は枠の中で error として出し、作業ツリーの先頭へ戻る道を残す。
export function TaskFilesScreen({ taskId, search }: { taskId: string; search: FilesSearch }) {
  return (
    <ScreenFrame title={`作業ツリーと成果物 ${taskId}`} route="/tasks/:id/files">
      <p className="text-label">
        <Link
          to="/tasks/$id"
          params={{ id: taskId }}
          className="inline-flex min-h-11 items-center gap-1 text-foreground underline"
        >
          <Icon name="chevron-left" size="sm" />
          タスクの詳細へ
        </Link>
      </p>
      <TaskFilesPanel taskId={taskId} search={search} />
    </ScreenFrame>
  );
}

/** 作業ツリーの一覧と本文。/tasks/:id?tab=files（P3-13）も同じ部品を置く。広い幅では一覧と本文を横に並べる。 */
export function TaskFilesPanel({ taskId, search }: { taskId: string; search: FilesSearch }) {
  if (!search.file) return <TreePane taskId={taskId} search={search} />;
  return (
    <div className="grid min-w-0 gap-4 lg:grid-cols-3">
      <div className="min-w-0 lg:col-span-1">
        <TreePane taskId={taskId} search={search} />
      </div>
      <div className="min-w-0 lg:col-span-2">
        <FilePane taskId={taskId} search={search} file={search.file} />
      </div>
    </div>
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

/** 取得できなかった path を示す枠。path は折り返して全文を出し、先頭へ戻る link を添える。 */
function PathError({ error, path, action }: { error: unknown; path: string; action: ReactNode }) {
  return (
    <div
      role="alert"
      data-testid="files-error"
      className="flex min-w-0 flex-col gap-2 border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
    >
      <p className="flex min-w-0 items-start gap-2">
        <Icon name="alert" />
        <span className="min-w-0">{describeError(error)}</span>
      </p>
      <p className="min-w-0 break-all font-mono text-label" title={path}>
        {path}
      </p>
      <div>{action}</div>
    </div>
  );
}

const linkClass = "inline-flex min-h-11 items-center text-foreground underline";

function TreePane({ taskId, search }: { taskId: string; search: FilesSearch }) {
  const tree = useQuery(taskTreeQuery(taskId, search.repo, search.path));
  if (tree.isError && tree.data === undefined)
    return (
      <PathError
        error={tree.error}
        path={search.path ?? "/"}
        action={
          <Link to="/tasks/$id/files" params={{ id: taskId }} search={{ repo: search.repo }} className={linkClass}>
            作業ツリーの先頭へ
          </Link>
        }
      />
    );
  const up = parentPath(search.path);
  const data = tree.data;
  const location = data ? `${data.repo}:/${data.path}` : "";
  return (
    <FetchFrame query={tree} subject="作業ツリー">
      {data ? (
        <section aria-label="作業ツリー" data-testid="files-tree" className="flex min-w-0 flex-col gap-2">
          {data.repos.length > 1 ? (
            <nav aria-label="リポジトリ">
              <ul className="flex flex-wrap gap-2 text-label">
                {data.repos.map((repo) => (
                  <li key={repo.name}>
                    <Link
                      to="/tasks/$id/files"
                      params={{ id: taskId }}
                      search={{ repo: repo.name }}
                      aria-current={repo.name === data.repo ? "page" : undefined}
                      className={`${linkClass} rounded-sm px-2 ${repo.name === data.repo ? "bg-accent font-semibold" : ""}`}
                    >
                      {repo.name}
                    </Link>
                  </li>
                ))}
              </ul>
            </nav>
          ) : null}
          <p
            className="min-w-0 break-all font-mono text-label text-muted-foreground"
            data-testid="files-path"
            title={location}
          >
            {location}
          </p>
          {data.entries.length === 0 && !search.path ? (
            <EmptyState message="このディレクトリは空です。" />
          ) : (
            <Table aria-label={`一覧 ${location}`} className="table-fixed">
              <TableHeader>
                <TableRow>
                  <TableHead>名前</TableHead>
                  <TableHead className="w-24 text-right">大きさ</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {search.path ? (
                  <TableRow>
                    <TableCell className="py-0">
                      <Link
                        to="/tasks/$id/files"
                        params={{ id: taskId }}
                        search={{ repo: search.repo, path: up }}
                        aria-label="一つ上のディレクトリへ"
                        className={`${linkClass} gap-1 font-mono`}
                      >
                        <Icon name="chevron-up" size="sm" />
                        ..
                      </Link>
                    </TableCell>
                    <TableCell />
                  </TableRow>
                ) : null}
                {data.entries.map((entry) => {
                  const dir = entry.kind === "dir";
                  const current = entry.path === search.file;
                  return (
                    <TableRow key={entry.path} data-state={current ? "checked" : undefined}>
                      <TableCell className="min-w-0 py-0">
                        <Link
                          to="/tasks/$id/files"
                          params={{ id: taskId }}
                          search={
                            dir
                              ? { repo: search.repo, path: entry.path }
                              : { repo: search.repo, path: search.path, file: entry.path }
                          }
                          data-entry={entry.path}
                          title={entry.path}
                          aria-current={current ? "page" : undefined}
                          className={`${linkClass} min-w-0 gap-1 break-all py-1 font-mono ${current ? "font-semibold" : ""}`}
                        >
                          {dir ? <Icon name="chevron-right" size="sm" /> : null}
                          <span className="min-w-0">{dir ? `${entry.name}/` : entry.name}</span>
                        </Link>
                      </TableCell>
                      <TableCell
                        className="text-right whitespace-nowrap text-muted-foreground tabular-nums"
                        title={entry.size != null ? `${entry.size} B` : undefined}
                      >
                        {entry.size != null ? formatBytes(entry.size) : dir ? "—" : ""}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
          {data.entries.length === 0 && search.path ? <EmptyState message="このディレクトリは空です。" /> : null}
        </section>
      ) : null}
    </FetchFrame>
  );
}

function FilePane({ taskId, search, file }: { taskId: string; search: FilesSearch; file: string }) {
  const body = useQuery(taskTreeFileQuery(taskId, file, search.repo));
  if (body.isError && body.data === undefined)
    return (
      <PathError
        error={body.error}
        path={file}
        action={
          <Link
            to="/tasks/$id/files"
            params={{ id: taskId }}
            search={{ repo: search.repo, path: search.path }}
            className={linkClass}
          >
            本文を閉じる
          </Link>
        }
      />
    );
  const data = body.data;
  return (
    <FetchFrame query={body} subject="file の本文">
      {data ? (
        <section aria-label="file の本文" data-testid="files-body" className="flex min-w-0 flex-col gap-2">
          <div className="flex min-w-0 flex-wrap items-baseline gap-2">
            <h2 className="min-w-0 break-all font-mono text-body font-semibold text-foreground" title={data.path}>
              {data.path}
            </h2>
            <span className="text-label text-muted-foreground tabular-nums" title={`${data.size} B`}>
              {formatBytes(data.size)}
            </span>
          </div>
          {data.too_large ? (
            <FileNotice testId="files-too-large" label="大きすぎる">
              大きすぎるため本文を表示しません（{formatBytes(data.size)}）。
            </FileNotice>
          ) : data.binary ? (
            <FileNotice testId="files-binary" label="バイナリ">
              バイナリのため本文を表示しません（{formatBytes(data.size)}）。
            </FileNotice>
          ) : data.text ? (
            <CodeBlock label={`本文 ${data.path}`}>{data.text}</CodeBlock>
          ) : (
            <EmptyState message="この file は空です。" />
          )}
        </section>
      ) : null}
    </FetchFrame>
  );
}

function FileNotice({ testId, label, children }: { testId: string; label: string; children: ReactNode }) {
  return (
    <p
      role="status"
      data-testid={testId}
      className="flex min-w-0 flex-wrap items-center gap-2 border-t border-border py-3 text-body text-muted-foreground"
    >
      <Badge tone="neutral">{label}</Badge>
      <span className="min-w-0">{children}</span>
    </p>
  );
}
