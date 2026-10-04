import { apiGet } from "../../api/client";
import type { TreeFileView, TreeView } from "../../api/generated/types";
import { taskKeys } from "../../api/queries/keys";

// /tasks/:id/files の query（P3-11、R24）。path の検査は daemon（403 path_forbidden）が行う。
// 本文は daemon の `tree/file` が大きさを見て `too_large` / `binary` を返すので、巨大な file を全部は読まない。

export type FilesSearch = { repo?: string; path?: string; file?: string };

function withQuery(base: string, params: Record<string, string | undefined>): string {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) if (value !== undefined) query.set(key, value);
  const text = query.toString();
  return text ? `${base}?${text}` : base;
}

export function taskTreePath(taskId: string, repo?: string, path?: string): string {
  return withQuery(`/api/tasks/${encodeURIComponent(taskId)}/tree`, { repo, path });
}

export function taskTreeFilePath(taskId: string, file: string, repo?: string): string {
  return withQuery(`/api/tasks/${encodeURIComponent(taskId)}/tree/file`, { repo, path: file });
}

export function taskTreeQuery(taskId: string, repo?: string, path?: string) {
  return {
    queryKey: [...taskKeys.files(taskId), "tree", repo ?? "", path ?? ""] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<TreeView>(taskTreePath(taskId, repo, path), signal),
  };
}

export function taskTreeFileQuery(taskId: string, file: string, repo?: string) {
  return {
    queryKey: [...taskKeys.files(taskId), "file", repo ?? "", file] as const,
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<TreeFileView>(taskTreeFilePath(taskId, file, repo), signal),
  };
}

/** 親ディレクトリの path。root は undefined。 */
export function parentPath(path: string | undefined): string | undefined {
  if (!path) return undefined;
  const parts = path.split("/").filter(Boolean);
  parts.pop();
  return parts.length ? parts.join("/") : undefined;
}

/** byte 数を読める単位にする。原数は title 属性で示す。 */
export function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = size / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
