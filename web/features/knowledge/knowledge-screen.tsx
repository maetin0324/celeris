import { useQuery } from "@tanstack/react-query";
import { Link, useRouter, useRouterState } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { apiGet } from "../../api/client";
import type { KnowledgeInbox, KnowledgePage, KnowledgeTree } from "../../api/generated/types";
import { knowledgeKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";

type Sender = ReturnType<typeof useActionResult>;

const field = "block w-full min-h-11 rounded border border-neutral-400 p-2 text-base";
const pathFor = (q: string, path?: string, edit = false) => {
  const params = new URLSearchParams();
  if (q) params.set("q", q);
  if (path) params.set("path", path);
  if (edit) params.set("edit", "1");
  return `/knowledge${params.size ? `?${params}` : ""}`;
};

function PageEditor({ page, sender }: { page: KnowledgePage; sender: Sender }) {
  const [body, setBody] = useState(page.raw);
  const [message, setMessage] = useState("");
  useEffect(() => {
    setBody(page.raw);
  }, [page.raw]);
  const result = sender.results[page.path];
  return (
    <form
      className="space-y-3"
      onSubmit={(event) => {
        event.preventDefault();
        void sender.run([
          {
            id: page.path,
            method: "PUT",
            path: "/api/knowledge/page",
            body: { path: page.path, body, etag: page.etag, message: message || null },
          },
        ]);
      }}
    >
      <label className="block">
        本文
        <textarea
          className={`${field} min-h-48 font-mono`}
          aria-label="本文"
          value={body}
          onChange={(event) => setBody(event.target.value)}
          aria-describedby={result?.status === 422 ? "knowledge-body-error" : undefined}
        />
      </label>
      {result?.status === 422 && <ActionResultView result={result} fieldId="knowledge-body-error" />}
      <label className="block">
        コミットメッセージ（任意）
        <input className={field} value={message} onChange={(event) => setMessage(event.target.value)} />
      </label>
      <Button disabled={sender.pending} type="submit">
        保存
      </Button>
      {result?.status !== 422 && <ActionResultView result={result} />}
    </form>
  );
}

export function KnowledgeScreen() {
  const searchStr = useRouterState({ select: (state) => state.location.searchStr });
  const router = useRouter();
  const params = new URLSearchParams(searchStr);
  const q = params.get("q") ?? "";
  const path = params.get("path") ?? "";
  const sender = useActionResult(knowledgeKeys.all);
  const [search, setSearch] = useState(q);
  useEffect(() => setSearch(q), [q]);
  const tree = useQuery({
    queryKey: [...knowledgeKeys.all, "tree", q],
    queryFn: ({ signal }) =>
      apiGet<KnowledgeTree>(`/api/knowledge/tree${q ? `?q=${encodeURIComponent(q)}` : ""}`, signal),
  });
  const page = useQuery({
    queryKey: [...knowledgeKeys.all, "page", path],
    queryFn: ({ signal }) => apiGet<KnowledgePage>(`/api/knowledge/page?path=${encodeURIComponent(path)}`, signal),
    enabled: !!path,
  });
  return (
    <ScreenFrame title="知識" route="/knowledge">
      <nav className="flex flex-wrap gap-3">
        <Link className="underline inline-flex min-w-11 min-h-11 items-center" to="/knowledge/inbox">
          候補
        </Link>
        <Link className="underline inline-flex min-w-11 min-h-11 items-center" to="/knowledge/skills">
          skills
        </Link>
      </nav>
      <form
        className="flex flex-wrap gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          router.history.push(pathFor(search.trim(), path), {});
        }}
      >
        <label className="flex-1 min-w-0">
          検索
          <input className={field} value={search} onChange={(event) => setSearch(event.target.value)} />
        </label>
        <Button type="submit">検索</Button>
      </form>
      <div className="grid min-w-0 gap-4 lg:grid-cols-[minmax(12rem,1fr)_minmax(0,2fr)]">
        <section className="min-w-0 rounded border p-3">
          <h2 className="font-semibold">検索結果</h2>
          <FetchFrame query={tree}>
            <ul className="space-y-2">
              {tree.data?.items.map((item) => (
                <li key={item.path}>
                  <Link
                    className="underline inline-flex min-w-11 min-h-11 items-center break-all"
                    to={pathFor(q, item.path)}
                  >
                    {item.title || item.path}
                  </Link>
                </li>
              ))}
            </ul>
            {tree.data?.items.length === 0 && <p>一致する知識はありません。</p>}
          </FetchFrame>
        </section>
        <section className="min-w-0 rounded border p-3 space-y-3">
          {path ? (
            <FetchFrame query={page}>
              {page.data && (
                <>
                  <h2 className="font-semibold break-all">{page.data.title || page.data.path}</h2>
                  <p className="text-sm break-all">{page.data.path}</p>
                  {params.get("edit") === "1" ? (
                    <PageEditor key={page.data.path} page={page.data} sender={sender} />
                  ) : (
                    <>
                      <Markdown source={page.data.raw} />
                      <Link
                        className="underline min-w-11 min-h-11 inline-flex items-center"
                        to={pathFor(q, path, true)}
                      >
                        編集
                      </Link>
                    </>
                  )}
                </>
              )}
            </FetchFrame>
          ) : (
            <p>知識を選択してください。</p>
          )}
        </section>
      </div>
    </ScreenFrame>
  );
}

function Candidate({ item, sender }: { item: KnowledgeInbox["items"][number]; sender: Sender }) {
  const [target, setTarget] = useState(item.target);
  const [overwrite, setOverwrite] = useState(false);
  const result = sender.results[item.id];
  return (
    <li className="min-w-0 rounded border p-3 space-y-2">
      <h2 className="font-semibold">{item.title}</h2>
      <p className="break-all text-sm">{item.path}</p>
      <Markdown source={item.body} />
      <label className="block">
        取り込み先
        <input
          className={field}
          value={target}
          onChange={(event) => setTarget(event.target.value)}
          aria-describedby={result?.status === 422 ? `target-${item.id}` : undefined}
        />
      </label>
      {result?.status === 422 && <ActionResultView result={result} fieldId={`target-${item.id}`} />}
      {item.target_exists && (
        <label className="flex items-center gap-2 min-h-11">
          <input
            type="checkbox"
            className="size-11"
            checked={overwrite}
            onChange={(event) => setOverwrite(event.target.checked)}
          />
          既存ページを上書き
        </label>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={sender.pending}
          onClick={() =>
            void sender.run([
              {
                id: item.id,
                path: `/api/knowledge/inbox/${encodeURIComponent(item.id)}/accept`,
                body: { path: target, overwrite },
              },
            ])
          }
        >
          採用
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() =>
            void sender.run([
              { id: item.id, path: `/api/knowledge/inbox/${encodeURIComponent(item.id)}/reject`, body: {} },
            ])
          }
        >
          却下
        </Button>
      </div>
    </li>
  );
}

export function KnowledgeInboxScreen() {
  const sender = useActionResult(knowledgeKeys.all);
  const inbox = useQuery({
    queryKey: [...knowledgeKeys.all, "inbox"],
    queryFn: ({ signal }) => apiGet<KnowledgeInbox>("/api/knowledge/inbox", signal),
  });
  return (
    <ScreenFrame title="知識の候補" route="/knowledge/inbox">
      <Link className="underline inline-flex min-w-11 min-h-11 items-center" to="/knowledge">
        知識に戻る
      </Link>
      <section aria-label="操作の結果">
        {Object.entries(sender.results)
          .filter(([, result]) => result.status !== 422)
          .map(([id, result]) => (
            <div key={id}>
              <strong>{id}: </strong>
              <ActionResultView result={result} />
            </div>
          ))}
      </section>
      <FetchFrame query={inbox}>
        <ul className="space-y-3">
          {inbox.data?.items.map((item) => (
            <Candidate key={item.id} item={item} sender={sender} />
          ))}
        </ul>
        {inbox.data?.items.length === 0 && <p>候補はありません。</p>}
      </FetchFrame>
    </ScreenFrame>
  );
}
