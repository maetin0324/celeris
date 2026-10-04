import { useQuery } from "@tanstack/react-query";
import { Link, useRouter, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { apiGet } from "../../api/client";
import type { SkillDetailView, SkillList } from "../../api/generated/types";
import { skillKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { StatusBadge } from "../../components/ui/status-badge";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";

type Sender = ReturnType<typeof useActionResult>;

// 入力欄の枠は --color-input（DESIGN.md「Form」）。
const field =
  "block w-full min-w-0 min-h-11 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const labelText = "text-label font-medium";
const navLink = "inline-flex min-h-11 min-w-11 items-center text-primary underline";
const href = (name?: string, edit = false) =>
  name ? `/knowledge/skills?name=${encodeURIComponent(name)}${edit ? "&edit=1" : ""}` : "/knowledge/skills";
const createHref: string = `${href()}?create=1`;
type ActionResult = Sender["results"][string];

/** skill の状態（登録・配送・失敗）を色と文字の両方で示す。色だけに頼らない（DESIGN.md §Badge）。 */
function SkillState({ mountedBy, result }: { mountedBy?: readonly string[]; result?: ActionResult }) {
  return (
    <span className="flex flex-wrap gap-1">
      <Badge tone="success">登録済み</Badge>
      {mountedBy?.length ? (
        <Badge tone="info">配送 {mountedBy.length} 課</Badge>
      ) : (
        <Badge tone="neutral">配送なし</Badge>
      )}
      {result && !result.ok ? <StatusBadge status="failed" /> : null}
    </span>
  );
}

function MountedBy({ mountedBy }: { mountedBy?: readonly string[] }) {
  if (!mountedBy?.length) return <span className="text-muted-foreground">どの課にも配送していません</span>;
  return <span className="break-all">{mountedBy.join("、")}</span>;
}

function Updated({ value }: { value?: string | null }) {
  return value ? <time dateTime={value}>{value}</time> : <span className="text-muted-foreground">不明</span>;
}

type FileRow = { id: string; path: string; content: string };

function SkillForm({ name: selected, detail, sender }: { name?: string; detail?: SkillDetailView; sender: Sender }) {
  const router = useRouter();
  const [name, setName] = useState(selected ?? "");
  const [markdown, setMarkdown] = useState(
    detail?.skill_md ?? "---\nname: new-skill\ndescription: \n---\n\n# New skill\n",
  );
  const [files, setFiles] = useState<FileRow[]>([{ id: "initial", path: "", content: "" }]);
  useEffect(() => {
    setName(selected ?? "");
    setMarkdown(detail?.skill_md ?? "---\nname: new-skill\ndescription: \n---\n\n# New skill\n");
    setFiles([{ id: "initial", path: "", content: "" }]);
  }, [selected, detail?.skill_md]);
  const result = sender.results[name];
  const errorField =
    result?.status === 422
      ? /file|ファイル/i.test(result.message)
        ? "files"
        : /name|名前/i.test(result.message)
          ? "name"
          : "markdown"
      : null;
  const updateFile = (index: number, key: keyof FileRow, value: string) =>
    setFiles((rows) => rows.map((row, i) => (i === index ? { ...row, [key]: value } : row)));
  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const nextName = name.trim();
    if (!nextName) return;
    const outcomes = await sender.run([
      {
        id: nextName,
        method: "PUT",
        path: `/api/skills/${encodeURIComponent(nextName)}`,
        body: {
          skill_md: markdown,
          files: files
            .filter((file) => file.path.trim())
            .map((file) => ({ path: file.path.trim(), content: file.content })),
        },
      },
    ]);
    if (outcomes[0]?.ok) router.history.push(href(nextName), {});
  };
  return (
    <form className="space-y-3 min-w-0" onSubmit={(event) => void submit(event)}>
      <label className="block space-y-1">
        <span className={labelText}>名前</span>
        <input
          className={field}
          value={name}
          disabled={!!selected}
          onChange={(event) => setName(event.target.value)}
          required
          aria-describedby={errorField === "name" ? "skill-name-error" : undefined}
        />
      </label>
      {errorField === "name" && <ActionResultView result={result} fieldId="skill-name-error" />}
      <label className="block space-y-1">
        <span className={labelText}>SKILL.md</span>
        <textarea
          className={`${field} min-h-48 font-mono`}
          aria-label="SKILL.md"
          value={markdown}
          onChange={(event) => setMarkdown(event.target.value)}
        />
      </label>
      {errorField === "markdown" && <ActionResultView result={result} fieldId="skill-markdown-error" />}
      <fieldset className="space-y-2 min-w-0">
        <legend className="text-body font-semibold">付属ファイル</legend>
        {files.map((file, index) => (
          <div key={file.id} className="grid min-w-0 gap-2 rounded-md border border-border p-3">
            <label className="block space-y-1">
              <span className={labelText}>ファイルのパス</span>
              <input
                className={field}
                value={file.path}
                onChange={(event) => updateFile(index, "path", event.target.value)}
              />
            </label>
            <label className="block space-y-1">
              <span className={labelText}>ファイルの内容</span>
              <textarea
                className={`${field} min-h-24`}
                aria-label="ファイルの内容"
                value={file.content}
                onChange={(event) => updateFile(index, "content", event.target.value)}
              />
            </label>
            <Button
              type="button"
              variant="secondary"
              onClick={() => setFiles((rows) => rows.filter((_, i) => i !== index))}
            >
              このファイルを削除
            </Button>
          </div>
        ))}
        {errorField === "files" && <ActionResultView result={result} fieldId="skill-files-error" />}
      </fieldset>
      <Button
        type="button"
        variant="secondary"
        onClick={() => setFiles((rows) => [...rows, { id: crypto.randomUUID(), path: "", content: "" }])}
      >
        ファイルを追加
      </Button>
      <div>
        <Button disabled={sender.pending} type="submit">
          {sender.pending ? "保存中…" : "保存"}
        </Button>
      </div>
      {!errorField && <ActionResultView result={result} />}
    </form>
  );
}

function SkillDetail({ name, edit, sender }: { name: string; edit: boolean; sender: Sender }) {
  const router = useRouter();
  const detail = useQuery({
    queryKey: [...skillKeys.all, "detail", name],
    queryFn: ({ signal }) => apiGet<SkillDetailView>(`/api/skills/${encodeURIComponent(name)}`, signal),
  });
  const result = sender.results[name];
  return (
    <FetchFrame query={detail}>
      {detail.data &&
        (edit ? (
          <Section title={`${detail.data.name} の編集`}>
            <SkillForm name={name} detail={detail.data} sender={sender} />
          </Section>
        ) : (
          <Section
            title={<span className="break-all">{detail.data.name}</span>}
            actions={
              <>
                <Link className={navLink} to={href(name, true)}>
                  編集
                </Link>
                <ConfirmDialog
                  trigger={
                    <Button variant="secondary" disabled={sender.pending}>
                      削除
                    </Button>
                  }
                  title="skill を削除しますか"
                  target={`skill「${detail.data.name}」`}
                  consequence={
                    detail.data.mounted_by?.length
                      ? `SKILL.md と付属ファイルを消します。配送先の課（${detail.data.mounted_by.join("、")}）にも届かなくなります。`
                      : "SKILL.md と付属ファイルを消します。"
                  }
                  reversibility="削除は戻せません。必要なら同じ名前で作り直します。"
                  followUp="skill 一覧から消えたことで確かめられます。"
                  confirmLabel={`skill「${detail.data.name}」を削除`}
                  onConfirm={async () => {
                    const [outcome] = await sender.run([
                      { id: name, method: "DELETE", path: `/api/skills/${encodeURIComponent(name)}` },
                    ]);
                    if (outcome && !outcome.ok) throw new Error(outcome.message);
                    router.history.push(href(), {});
                  }}
                />
              </>
            }
          >
            <div className="space-y-3">
              <DataList
                items={[
                  { label: "状態", value: <SkillState mountedBy={detail.data.mounted_by} result={result} /> },
                  { label: "配送先", value: <MountedBy mountedBy={detail.data.mounted_by} /> },
                  { label: "更新日", value: <Updated value={detail.data.updated} /> },
                  {
                    label: "付属ファイル",
                    value: detail.data.files?.length ? (
                      <ul className="space-y-1">
                        {detail.data.files.map((file) => (
                          <li className="break-all" key={file}>
                            {file}
                          </li>
                        ))}
                      </ul>
                    ) : (
                      <span className="text-muted-foreground">なし</span>
                    ),
                  },
                ]}
              />
              {result?.status === 422 && <ActionResultView result={result} />}
              <div className="max-w-prose-ja">
                <Markdown source={detail.data.skill_md} />
              </div>
            </div>
          </Section>
        ))}
    </FetchFrame>
  );
}

function SkillTable({ items, results }: { items: SkillList["items"]; results: Sender["results"] }) {
  if (items.length === 0) return <p>skill はありません。</p>;
  return (
    <>
      {/* スマホ: 対象名 → 状態 → 補助情報の順の行（DESIGN.md「Table と list」）。 */}
      <ul className="divide-y divide-border md:hidden">
        {items.map((item) => (
          <li key={item.name} className="min-w-0 space-y-1 py-2">
            <Link className={`${navLink} break-all`} to={href(item.name)}>
              {item.name}
            </Link>
            <SkillState mountedBy={item.mounted_by} result={results[item.name]} />
            <p className="break-words text-label text-muted-foreground">{item.description}</p>
          </li>
        ))}
      </ul>
      <div className="hidden md:block">
        <Table aria-label="skill の一覧">
          <TableCaption>{items.length} 件</TableCaption>
          <TableHeader>
            <TableRow>
              <TableHead>名前</TableHead>
              <TableHead>状態</TableHead>
              <TableHead>更新日</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {items.map((item) => (
              <TableRow key={item.name}>
                <TableCell>
                  <Link className={`${navLink} break-all`} to={href(item.name)}>
                    {item.name}
                  </Link>
                  <p className="break-words text-muted-foreground">{item.description}</p>
                </TableCell>
                <TableCell>
                  <SkillState mountedBy={item.mounted_by} result={results[item.name]} />
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

export function SkillsScreen() {
  const sender = useActionResult(skillKeys.all);
  const search = useSearch({ from: "/knowledge/skills" });
  const name = search.name || "";
  const create = search.create === "1";
  const edit = search.edit === "1";
  const list = useQuery({
    queryKey: skillKeys.list(),
    queryFn: ({ signal }) => apiGet<SkillList>("/api/skills", signal),
  });
  return (
    <ScreenFrame title="skills" route="/knowledge/skills">
      <nav aria-label="skills の操作" className="flex flex-wrap gap-4">
        <Link className={navLink} to="/knowledge">
          知識に戻る
        </Link>
        <Link className={navLink} to={createHref}>
          作成
        </Link>
      </nav>
      <section aria-label="操作の結果">
        {Object.entries(sender.results)
          .filter(([, result]) => result.status !== 422)
          .map(([id, result]) => (
            <div key={id} className="flex flex-wrap gap-1">
              <strong>{id}:</strong>
              <ActionResultView result={result} />
            </div>
          ))}
      </section>
      <div className="grid min-w-0 gap-6 lg:grid-cols-5">
        <Section title="skill 一覧" className="lg:col-span-2">
          <FetchFrame query={list}>
            {list.data && <SkillTable items={list.data.items} results={sender.results} />}
          </FetchFrame>
        </Section>
        <div className="min-w-0 rounded-lg border border-border bg-surface p-4 lg:col-span-3">
          {create ? (
            <Section title="skill の作成">
              <SkillForm sender={sender} />
            </Section>
          ) : name ? (
            <SkillDetail key={name} name={name} edit={edit} sender={sender} />
          ) : (
            <p className="text-muted-foreground">skill 一覧から skill を選んでください。</p>
          )}
        </div>
      </div>
    </ScreenFrame>
  );
}
