import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useRouter } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { ApiError, apiGet } from "../../api/client";
import type { DocPage, DocsTree } from "../../api/generated/types";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

const input = "box-border min-h-11 w-full max-w-full rounded-md border border-input bg-surface p-2 text-foreground";
const navLink = "inline-flex min-h-11 min-w-11 items-center text-primary underline";
const docsBase = (id: string) => `/api/projects/${encodeURIComponent(id)}/docs`;
const docsUrl = (id: string, path?: string, q?: string, edit?: boolean) => {
  const params = new URLSearchParams();
  if (path) params.set("path", path);
  if (q) params.set("q", q);
  if (edit) params.set("edit", "1");
  return `/projects/${encodeURIComponent(id)}/docs${params.size ? `?${params}` : ""}`;
};

function Editor({ projectId, path, page }: { projectId: string; path: string; page?: DocPage }) {
  const sender = useActionResult(projectKeys.docs(projectId, path));
  const client = useQueryClient();
  const router = useRouter();
  const [draft, setDraft] = useState<{ path: string; body: string }>({ path, body: page?.raw ?? "" });
  // The editor is keyed by path; query refreshes do not replace the draft.
  const result = sender.results.save;
  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const nextPath = String(form.get("path") ?? "");
    const body = { path: nextPath, body: draft.body, ...(page?.etag ? { etag: page.etag } : {}) };
    const [outcome] = await sender.run([{ id: "save", method: "PUT", path: `${docsBase(projectId)}/page`, body }]);
    if (outcome?.ok) {
      await client.invalidateQueries({ queryKey: projectKeys.all });
      if (nextPath !== path) router.history.push(docsUrl(projectId, nextPath), {});
    }
  }
  return (
    <form onSubmit={save} className="min-w-0 space-y-3" data-testid="docs-editor">
      <label className="block">
        パス
        <input name="path" className={input} defaultValue={path} />
      </label>
      <label className="block min-w-0">
        Markdown
        <textarea
          name="body"
          className={`${input} min-w-0 resize-y`}
          rows={16}
          value={draft.body}
          onChange={(e) => setDraft({ path, body: e.target.value })}
        />
      </label>
      <Button type="submit" disabled={sender.pending}>
        保存
      </Button>
      <ActionResultView result={result} fieldId="docs-save-result" />
    </form>
  );
}

export function ProjectDocsScreen({
  projectId,
  path,
  q,
  edit,
}: {
  projectId: string;
  path?: string;
  q?: string;
  edit?: "1";
}) {
  const router = useRouter();
  const client = useQueryClient();
  const tree = useQuery({
    queryKey: projectKeys.docs(projectId, `tree:${q ?? ""}`),
    queryFn: async ({ signal }) => {
      try {
        return await apiGet<DocsTree>(`${docsBase(projectId)}${q ? `?q=${encodeURIComponent(q)}` : ""}`, signal);
      } catch (error) {
        if (error instanceof ApiError && error.status === 409) return null;
        throw error;
      }
    },
  });
  const page = useQuery({
    queryKey: projectKeys.docs(projectId, path ?? ""),
    enabled: !!path && tree.data !== null,
    queryFn: ({ signal }) =>
      apiGet<DocPage>(`${docsBase(projectId)}/page?path=${encodeURIComponent(path ?? "")}`, signal),
  });
  const sender = useActionResult(projectKeys.all);
  const result = sender.results.init ?? sender.results.delete;
  async function init() {
    await sender.run([{ id: "init", path: `${docsBase(projectId)}/init` }]);
  }
  async function remove() {
    if (!path || !window.confirm(`文書 ${path} を削除しますか？`)) return;
    const params = new URLSearchParams({ path });
    if (page.data?.etag) params.set("etag", page.data.etag);
    const [outcome] = await sender.run([
      { id: "delete", method: "DELETE", path: `${docsBase(projectId)}/page?${params}` },
    ]);
    if (outcome?.ok) {
      await client.removeQueries({ queryKey: projectKeys.docs(projectId, path) });
      router.history.push(docsUrl(projectId), {});
    }
  }
  function search(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = String(new FormData(event.currentTarget).get("q") ?? "").trim();
    router.history.push(docsUrl(projectId, undefined, text), {});
  }
  return (
    <ScreenFrame title={`案件の文書 ${projectId}`} route="/projects/:id/docs">
      <nav aria-label="文書の頁" className="flex flex-wrap gap-3">
        <Link className={navLink} to="/projects/$id" params={{ id: projectId }}>
          案件詳細
        </Link>
        <Link className={navLink} to="/projects/$id/docs/maintenance" params={{ id: projectId }}>
          文書の保守
        </Link>
      </nav>
      <ActionResultView result={result} />
      <FetchFrame query={tree} subject="案件の文書の一覧">
        {tree.data === null ? (
          <section className="space-y-2 rounded-lg border border-border bg-surface p-4">
            <p>文書リポジトリがありません。用意すると、ここで文書を読み書きできます。</p>
            <Button onClick={init} disabled={sender.pending}>
              文書を用意する
            </Button>
          </section>
        ) : tree.data ? (
          <div className="grid min-w-0 gap-4 lg:grid-cols-3" data-testid="project-docs">
            <aside className="min-w-0 rounded-lg border border-border bg-surface p-3">
              <form onSubmit={search} className="space-y-2">
                <label>
                  文書を検索
                  <input name="q" className={input} defaultValue={q} />
                </label>
                <Button type="submit">検索</Button>
              </form>
              {tree.data.items.length === 0 ? (
                <p className="mt-3 text-muted-foreground">
                  {q ? `「${q}」に一致する文書はありません。` : "文書はまだありません。"}
                </p>
              ) : null}
              {tree.data.truncated ? (
                <p className="mt-3 text-label text-muted-foreground">
                  件数が多いため一部だけ表示しています。検索で絞ってください。
                </p>
              ) : null}
              <ul className="mt-3 space-y-1">
                {tree.data.items.map((item) => (
                  <li key={item.path}>
                    <Link
                      className="block min-h-11 py-2 break-words text-primary underline"
                      to="/projects/$id/docs"
                      params={{ id: projectId }}
                      search={{ path: item.path, q }}
                      aria-current={item.path === path ? "page" : undefined}
                    >
                      <span className={item.path === path ? "font-semibold" : undefined}>
                        {item.title || item.path}
                      </span>
                      {item.title ? (
                        <span className="block text-label text-muted-foreground break-all">{item.path}</span>
                      ) : null}
                    </Link>
                  </li>
                ))}
              </ul>
              <Link
                className="inline-flex min-h-11 items-center underline"
                to="/projects/$id/docs"
                params={{ id: projectId }}
                search={{ path: `${tree.data.root}/new-page.md`, q, edit: "1" }}
              >
                新しいページ
              </Link>
            </aside>
            <section className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-3 lg:col-span-2">
              {!path ? (
                <p>ページを選んでください。</p>
              ) : edit && (page.data || page.isError) ? (
                <Editor key={path} projectId={projectId} path={path} page={page.data} />
              ) : (
                <FetchFrame query={page} subject={`文書 ${path}`}>
                  {page.data ? (
                    <>
                      <h2 className="text-section font-semibold break-words">{page.data.title}</h2>
                      <Markdown source={page.data.raw} />
                      <div className="flex flex-wrap gap-3">
                        <Link
                          className={navLink}
                          to="/projects/$id/docs"
                          params={{ id: projectId }}
                          search={{ path, q, edit: "1" }}
                        >
                          編集
                        </Link>
                        <Button variant="destructive" onClick={remove} disabled={sender.pending}>
                          削除
                        </Button>
                      </div>
                    </>
                  ) : null}
                </FetchFrame>
              )}
            </section>
          </div>
        ) : null}
      </FetchFrame>
    </ScreenFrame>
  );
}
