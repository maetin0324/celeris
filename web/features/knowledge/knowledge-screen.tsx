import { useQuery } from "@tanstack/react-query";
import { Link, useRouter, useRouterState } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { apiGet } from "../../api/client";
import type { KnowledgeCandidate, KnowledgeInbox, KnowledgePage, KnowledgeTree } from "../../api/generated/types";
import { knowledgeKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";

type Sender = ReturnType<typeof useActionResult>;

// 入力欄の枠は --color-input（DESIGN.md「Form」）。
const field =
  "block w-full min-w-0 min-h-11 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const labelText = "text-label font-medium";
const navLink = "inline-flex min-h-11 min-w-11 items-center text-primary underline";
const pathFor = (q: string, path?: string, edit = false) => {
  const params = new URLSearchParams();
  if (q) params.set("q", q);
  if (path) params.set("path", path);
  if (edit) params.set("edit", "1");
  return `/knowledge${params.size ? `?${params}` : ""}`;
};

/** 出典の一覧。無い時は「なし」と書き、空欄にしない。 */
function Sources({ sources }: { sources?: readonly string[] }) {
  if (!sources?.length) return <span className="text-muted-foreground">なし</span>;
  return (
    <ul className="min-w-0 space-y-1">
      {sources.map((source) => (
        <li key={source} className="break-all">
          {source}
        </li>
      ))}
    </ul>
  );
}

function Scope({ scope }: { scope?: string | null }) {
  return scope ? <span className="break-all">{scope}</span> : <span className="text-muted-foreground">未設定</span>;
}

function Updated({ value }: { value?: string | null }) {
  return value ? <time dateTime={value}>{value}</time> : <span className="text-muted-foreground">不明</span>;
}

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
      <label className="block space-y-1">
        <span className={labelText}>本文</span>
        <textarea
          className={`${field} min-h-48 font-mono`}
          aria-label="本文"
          value={body}
          onChange={(event) => setBody(event.target.value)}
          aria-describedby={result?.status === 422 ? "knowledge-body-error" : undefined}
        />
      </label>
      {result?.status === 422 && <ActionResultView result={result} fieldId="knowledge-body-error" />}
      <label className="block space-y-1">
        <span className={labelText}>コミットメッセージ（任意）</span>
        <input className={field} value={message} onChange={(event) => setMessage(event.target.value)} />
      </label>
      <Button disabled={sender.pending} type="submit">
        {sender.pending ? "保存中…" : "保存"}
      </Button>
      {result?.status !== 422 && <ActionResultView result={result} />}
    </form>
  );
}

function KnowledgeResults({ items, q }: { items: KnowledgeTree["items"]; q: string }) {
  if (items.length === 0) return <p>一致する知識はありません。</p>;
  return (
    <>
      {/* スマホ: 対象名 → 補助情報の順の行（DESIGN.md「Table と list」）。 */}
      <ul className="divide-y divide-border md:hidden">
        {items.map((item) => (
          <li key={item.path} className="min-w-0 py-2">
            <Link className={`${navLink} break-all`} to={pathFor(q, item.path)}>
              {item.title || item.path}
            </Link>
            <DataList
              items={[
                { label: "出典", value: <Sources sources={item.sources} /> },
                { label: "scope", value: <Scope scope={item.scope} /> },
                { label: "更新日", value: <Updated value={item.updated} /> },
              ]}
            />
          </li>
        ))}
      </ul>
      <div className="hidden md:block">
        <Table aria-label="知識の一覧">
          <TableCaption>{items.length} 件</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead>タイトル</TableHead>
              <TableHead>出典</TableHead>
              <TableHead>scope</TableHead>
              <TableHead>更新日</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((item) => (
              <TableRow key={item.path}>
                <TableCell>
                  <Link className={`${navLink} break-all`} to={pathFor(q, item.path)}>
                    {item.title || item.path}
                  </Link>
                  <p className="break-all text-muted-foreground">{item.path}</p>
                </TableCell>
                <TableCell>
                  <Sources sources={item.sources} />
                </TableCell>
                <TableCell>
                  <Scope scope={item.scope} />
                </TableCell>
                <TableCell className="whitespace-nowrap">
                  <Updated value={item.updated} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    </>
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
      <nav aria-label="知識の画面" className="flex flex-wrap gap-4">
        <Link className={navLink} to="/knowledge/inbox">
          候補
        </Link>
        <Link className={navLink} to="/knowledge/skills">
          skills
        </Link>
      </nav>
      <form
        className="flex flex-wrap items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          router.history.push(pathFor(search.trim(), path), {});
        }}
      >
        <label className="block min-w-0 flex-1 space-y-1">
          <span className={labelText}>検索</span>
          <input className={field} value={search} onChange={(event) => setSearch(event.target.value)} />
        </label>
        <Button type="submit">検索</Button>
      </form>
      <div className="grid min-w-0 gap-6 lg:grid-cols-5">
        {/* 名前付きの region にすると「検索」の label と取り違えるので、見出しだけで区切る。 */}
        <div className="min-w-0 lg:col-span-2">
          <h2 className="text-section font-semibold text-foreground">検索結果</h2>
          <FetchFrame query={tree}>
            {tree.data && <KnowledgeResults items={tree.data.items} q={q} />}
            {tree.data?.truncated && (
              <p className="text-label text-muted-foreground">結果の一部だけを表示しています。</p>
            )}
          </FetchFrame>
        </div>
        <div className="min-w-0 rounded-lg border border-border bg-surface p-4 lg:col-span-3">
          {path ? (
            <FetchFrame query={page}>
              {page.data && (
                <div className="space-y-3">
                  <Section
                    title={page.data.title || page.data.path}
                    description={<span className="break-all">{page.data.path}</span>}
                    actions={
                      params.get("edit") === "1" ? null : (
                        <Link className={navLink} to={pathFor(q, path, true)}>
                          編集
                        </Link>
                      )
                    }
                  >
                    <DataList
                      items={[
                        { label: "出典", value: <Sources sources={page.data.sources} /> },
                        { label: "scope", value: <Scope scope={page.data.scope} /> },
                        { label: "更新日", value: <Updated value={page.data.updated} /> },
                      ]}
                    />
                  </Section>
                  {params.get("edit") === "1" ? (
                    <PageEditor key={page.data.path} page={page.data} sender={sender} />
                  ) : (
                    <div className="max-w-prose-ja">
                      <Markdown source={page.data.raw} />
                    </div>
                  )}
                </div>
              )}
            </FetchFrame>
          ) : (
            <p className="text-muted-foreground">検索結果から知識を選んでください。</p>
          )}
        </div>
      </div>
    </ScreenFrame>
  );
}

/** ConfirmDialog は onConfirm が投げると dialog を閉じず理由を示す。失敗した結果は例外にして渡す。 */
async function runOrThrow(sender: Sender, target: Parameters<Sender["run"]>[0][number]) {
  const [result] = await sender.run([target]);
  if (result && !result.ok) throw new Error(result.message);
}

function Candidate({ item, sender }: { item: KnowledgeCandidate; sender: Sender }) {
  const [target, setTarget] = useState(item.target);
  const [overwrite, setOverwrite] = useState(false);
  const result = sender.results[item.id];
  const targetId = `target-${item.id}`;
  const destination = target.trim() || item.target;
  const summary = [
    `取り込み先 ${destination}`,
    `scope ${item.scope || "未設定"}`,
    `出典 ${item.sources?.length ? item.sources.join("、") : "なし"}`,
  ].join("・");
  return (
    <li className="min-w-0 rounded-lg border border-border bg-surface p-4">
      <Section title={item.title} description={<span className="break-all">{item.path}</span>}>
        <div className="space-y-3">
          <DataList
            items={[
              { label: "出典", value: <Sources sources={item.sources} /> },
              { label: "scope", value: <Scope scope={item.scope} /> },
              { label: "更新日", value: <Updated value={item.created} /> },
              {
                label: "取り込み先",
                value: (
                  <span className="break-all">
                    {item.target}
                    {item.target_exists ? "（既存ページあり）" : "（新規ページ）"}
                  </span>
                ),
              },
            ]}
          />
          <div className="max-w-prose-ja">
            <Markdown source={item.body} />
          </div>
          <label className="block space-y-1">
            <span className={labelText}>取り込み先</span>
            <input
              className={field}
              value={target}
              onChange={(event) => setTarget(event.target.value)}
              aria-describedby={result?.status === 422 ? targetId : undefined}
            />
          </label>
          {result?.status === 422 && <ActionResultView result={result} fieldId={targetId} />}
          {item.target_exists && (
            <label className="flex min-h-11 items-center gap-2">
              <input
                type="checkbox"
                className="size-11 accent-primary"
                checked={overwrite}
                onChange={(event) => setOverwrite(event.target.checked)}
              />
              既存ページを上書き
            </label>
          )}
          <div className="flex flex-wrap gap-2">
            <ConfirmDialog
              trigger={<Button disabled={sender.pending}>採用</Button>}
              title="候補を正本に採用しますか"
              target={`「${item.title}」`}
              consequence={`${summary} として正本に入ります。${
                item.target_exists ? (overwrite ? "既存ページを上書きします。" : "既存ページは上書きしません。") : ""
              }`}
              reversibility="正本に入った後は、知識の画面でそのページを編集して戻します。"
              followUp="この画面の操作の結果と、知識の画面の検索結果で確かめられます。"
              confirmLabel={`「${item.title}」を正本に採用`}
              onConfirm={() =>
                runOrThrow(sender, {
                  id: item.id,
                  path: `/api/knowledge/inbox/${encodeURIComponent(item.id)}/accept`,
                  body: { path: target, overwrite },
                })
              }
            />
            <ConfirmDialog
              trigger={
                <Button variant="secondary" disabled={sender.pending}>
                  却下
                </Button>
              }
              title="候補を却下しますか"
              target={`「${item.title}」`}
              consequence="正本には入らず、候補の一覧から外れます。"
              reversibility="却下した候補は戻せません。必要なら知識の画面で新しく書きます。"
              followUp="この画面の操作の結果で確かめられます。"
              confirmLabel={`「${item.title}」を却下`}
              onConfirm={() =>
                runOrThrow(sender, {
                  id: item.id,
                  path: `/api/knowledge/inbox/${encodeURIComponent(item.id)}/reject`,
                  body: {},
                })
              }
            />
          </div>
          {result && result.status !== 422 && <ActionResultView result={result} />}
        </div>
      </Section>
    </li>
  );
}

export function KnowledgeInboxScreen() {
  const sender = useActionResult(knowledgeKeys.all);
  const inbox = useQuery({
    queryKey: [...knowledgeKeys.all, "inbox"],
    queryFn: ({ signal }) => apiGet<KnowledgeInbox>("/api/knowledge/inbox", signal),
  });
  // 一覧から消えた候補の結果も読めるよう、行に無い結果だけをまとめて出す。
  const shown = new Set(inbox.data?.items.map((item) => item.id));
  const orphanResults = Object.entries(sender.results).filter(
    ([id, result]) => result.status !== 422 && !shown.has(id),
  );
  return (
    <ScreenFrame title="知識の候補" route="/knowledge/inbox">
      <Link className={navLink} to="/knowledge">
        知識に戻る
      </Link>
      <section aria-label="操作の結果">
        {orphanResults.map(([id, result]) => (
          <div key={id} className="flex flex-wrap gap-1">
            <strong>{id}:</strong>
            <ActionResultView result={result} />
          </div>
        ))}
      </section>
      <FetchFrame query={inbox}>
        {!!inbox.data?.items.length && (
          <p className="text-label text-muted-foreground">
            {inbox.data.items.length} 件。採用すると取り込み先のページとして正本に入ります。
          </p>
        )}
        <ul className="space-y-4">
          {inbox.data?.items.map((item) => (
            <Candidate key={item.id} item={item} sender={sender} />
          ))}
        </ul>
        {inbox.data?.items.length === 0 && <p>候補はありません。</p>}
      </FetchFrame>
    </ScreenFrame>
  );
}
