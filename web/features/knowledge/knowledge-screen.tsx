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
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { cn } from "../../lib/utils";
import { MetaLine } from "./meta-line";
import { useFocusOnChange } from "./use-focus-on-change";

type Sender = ReturnType<typeof useActionResult>;

// 入力欄の枠は --color-input（DESIGN.md「Form」）。
const field =
  "block w-full min-w-0 min-h-11 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const labelText = "text-label font-medium";
const navLink = "inline-flex min-h-11 min-w-11 items-center text-primary underline";
// 広い幅の 2 ペインの一覧側。window の scroll に付いて来て、長い一覧は枠の中で scroll する。
const listPane = "min-w-0 lg:sticky lg:top-0 lg:col-span-2 lg:max-h-dvh lg:self-start lg:overflow-y-auto";
const contentPane = "min-w-0 rounded-lg border border-border bg-surface p-4 lg:col-span-3";
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
  return <span className="break-all">{sources.join("、")}</span>;
}

function Scope({ scope }: { scope?: string | null }) {
  return scope ? <span className="break-all">{scope}</span> : <span className="text-muted-foreground">未設定</span>;
}

function Updated({ value }: { value?: string | null }) {
  return value ? <time dateTime={value}>{value}</time> : <span className="text-muted-foreground">不明</span>;
}

/** 本文の先頭の見出しが Section の title と同じなら外す（`# <title>` で始まる本文で見出しが二重に出ないように）。 */
export function withoutLeadingTitle(source: string, title: string): string {
  const frontMatter = /^---\n[\s\S]*?\n---\n/.exec(source)?.[0] ?? "";
  const rest = source.slice(frontMatter.length);
  const heading = /^\s*#{1,6}[ \t]+(.+?)[ \t#]*(?:\n|$)/.exec(rest);
  if (!heading || heading[1]?.trim() !== title.trim()) return source;
  return frontMatter + rest.slice(heading[0].length);
}

/** 見出しを外した本文。残りが空なら空欄にせず、そう書く。 */
function Body({ source, title }: { source: string; title: string }) {
  const body = withoutLeadingTitle(source, title);
  return (
    <div className="max-w-prose-ja">
      {body.trim() ? <Markdown source={body} /> : <p className="text-muted-foreground">本文は見出しだけです。</p>}
    </div>
  );
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

type TreeItem = KnowledgeTree["items"][number];

/** 一覧の行の 1 行の補助情報（更新日・scope・出典の件数）。出典の全文は中身の側で見せる。何も無ければ path。 */
function rowMeta(item: TreeItem): string {
  const parts = [
    item.updated ? `更新 ${item.updated}` : "",
    item.scope ? `scope ${item.scope}` : "",
    item.sources?.length ? `出典 ${item.sources.length} 件` : "",
  ].filter(Boolean);
  return parts.length ? parts.join("・") : item.path;
}

/** 行を並べた一覧（狭い幅と、選んだ時の一覧ペイン）。選んだ行は aria-current と背景で示す。 */
function ResultRows({ items, q, selected }: { items: readonly TreeItem[]; q: string; selected: string }) {
  return (
    <ul className="divide-y divide-border">
      {items.map((item) => {
        const current = item.path === selected;
        return (
          <li
            key={item.path}
            className={cn("min-w-0 rounded-md px-2 pb-2", current && "bg-accent text-accent-foreground")}
          >
            <Link
              className={`${navLink} break-all`}
              to={pathFor(q, item.path)}
              aria-current={current ? "true" : undefined}
            >
              {item.title || item.path}
            </Link>
            <p className="break-all text-label text-muted-foreground">{rowMeta(item)}</p>
          </li>
        );
      })}
    </ul>
  );
}

function KnowledgeResults({ items, q, selected }: { items: KnowledgeTree["items"]; q: string; selected: string }) {
  if (items.length === 0) return <p>一致する知識はありません。</p>;
  // 選んだ時は一覧ペインが狭いので、表ではなく行の一覧にする。
  if (selected) return <ResultRows items={items} q={q} selected={selected} />;
  return (
    <>
      {/* スマホ: 対象名 → 補助情報 1 行の順の行（DESIGN.md「Table と list」）。 */}
      <div className="md:hidden">
        <ResultRows items={items} q={q} selected={selected} />
      </div>
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
  const edit = params.get("edit") === "1";
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
  const headingRef = useFocusOnChange<HTMLHeadingElement>(path, page.data?.path === path);
  return (
    <ScreenFrame
      title="知識"
      route="/knowledge"
      actions={
        <nav aria-label="知識の画面" className="flex flex-wrap gap-x-4">
          <Link className={navLink} to="/knowledge/inbox">
            候補
          </Link>
          <Link className={navLink} to="/knowledge/skills">
            手順書（skills）
          </Link>
        </nav>
      }
    >
      <form
        className="flex min-w-0 items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          router.history.push(pathFor(search.trim(), path), {});
        }}
      >
        <label className="block min-w-0 flex-1">
          <span className="sr-only">検索</span>
          <input
            className={field}
            value={search}
            placeholder="タイトル・本文で検索"
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
        <Button type="submit">検索</Button>
      </form>
      {/* 未選択のときは本文枠を出さず、検索結果を全幅で並べる（空の枠で画面の 2/3 を空けない）。
          選んだ時は広い幅で一覧と中身の 2 ペイン、狭い幅では一覧を畳んで中身を見出しの直下に出す。 */}
      <div className={path ? "grid min-w-0 gap-6 lg:grid-cols-5" : "min-w-0"}>
        {/* 名前付きの region にすると「検索」の label と取り違えるので、見出しだけで区切る。 */}
        <div className={path ? cn("hidden lg:block", listPane) : "min-w-0"}>
          <h2 className={path ? "text-section font-semibold text-foreground" : "sr-only"}>検索結果</h2>
          <FetchFrame query={tree}>
            {tree.data && <KnowledgeResults items={tree.data.items} q={q} selected={path} />}
            {tree.data?.truncated && (
              <p className="text-label text-muted-foreground">結果の一部だけを表示しています。</p>
            )}
          </FetchFrame>
        </div>
        {path && (
          <div className={contentPane}>
            <FetchFrame query={page}>
              {page.data && (
                <section aria-labelledby="knowledge-page-title" className="min-w-0 space-y-3">
                  <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4">
                    <h2
                      id="knowledge-page-title"
                      ref={headingRef}
                      tabIndex={-1}
                      className="min-w-0 break-words pt-2 text-section font-semibold text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
                    >
                      {page.data.title || page.data.path}
                    </h2>
                    <div className="flex flex-wrap gap-x-4">
                      {!edit && (
                        <Link className={navLink} to={pathFor(q, path, true)}>
                          編集
                        </Link>
                      )}
                      <Link className={cn(navLink, "lg:hidden")} to={pathFor(q)}>
                        一覧に戻る
                      </Link>
                    </div>
                  </div>
                  <MetaLine
                    items={[
                      { label: "場所", value: page.data.path },
                      { label: "scope", value: <Scope scope={page.data.scope} /> },
                      { label: "更新日", value: <Updated value={page.data.updated} /> },
                      { label: "出典", value: <Sources sources={page.data.sources} /> },
                    ]}
                  />
                  {edit ? (
                    <PageEditor key={page.data.path} page={page.data} sender={sender} />
                  ) : (
                    <Body source={page.data.raw} title={page.data.title || page.data.path} />
                  )}
                </section>
              )}
            </FetchFrame>
          </div>
        )}
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
          <MetaLine
            items={[
              {
                label: "提案された取り込み先",
                value: `${item.target}${item.target_exists ? "（既存ページあり）" : "（新規ページ）"}`,
              },
              { label: "scope", value: <Scope scope={item.scope} /> },
              { label: "更新日", value: <Updated value={item.created} /> },
              { label: "出典", value: <Sources sources={item.sources} /> },
            ]}
          />
          <Body source={item.body} title={item.title} />
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
    <ScreenFrame
      title="知識の候補"
      route="/knowledge/inbox"
      actions={
        <Link className={navLink} to="/knowledge">
          知識に戻る
        </Link>
      }
    >
      {orphanResults.length > 0 && (
        <section aria-label="操作の結果">
          {orphanResults.map(([id, result]) => (
            <div key={id} className="flex flex-wrap gap-1">
              <strong>{id}:</strong>
              <ActionResultView result={result} />
            </div>
          ))}
        </section>
      )}
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
