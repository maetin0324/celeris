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
import { Button } from "../../components/ui/button";

type Sender = ReturnType<typeof useActionResult>;

const field = "block w-full min-w-0 min-h-11 rounded border border-neutral-400 p-2 text-base";
const href = (name?: string, edit = false) =>
  name ? `/knowledge/skills?name=${encodeURIComponent(name)}${edit ? "&edit=1" : ""}` : "/knowledge/skills";
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
      <label className="block">
        名前
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
      <label className="block">
        SKILL.md
        <textarea
          className={`${field} min-h-48 font-mono`}
          aria-label="SKILL.md"
          value={markdown}
          onChange={(event) => setMarkdown(event.target.value)}
        />
      </label>
      {errorField === "markdown" && <ActionResultView result={result} fieldId="skill-markdown-error" />}
      <fieldset className="space-y-2 min-w-0">
        <legend className="font-semibold">付属ファイル</legend>
        {files.map((file, index) => (
          <div key={file.id} className="grid min-w-0 gap-2 rounded border p-2">
            <label className="block">
              ファイルのパス
              <input
                className={field}
                value={file.path}
                onChange={(event) => updateFile(index, "path", event.target.value)}
              />
            </label>
            <label className="block">
              ファイルの内容
              <textarea
                className={`${field} min-h-24`}
                aria-label="ファイルの内容"
                value={file.content}
                onChange={(event) => updateFile(index, "content", event.target.value)}
              />
            </label>
            <Button type="button" onClick={() => setFiles((rows) => rows.filter((_, i) => i !== index))}>
              このファイルを削除
            </Button>
          </div>
        ))}
        {errorField === "files" && <ActionResultView result={result} fieldId="skill-files-error" />}
      </fieldset>
      <Button
        type="button"
        onClick={() => setFiles((rows) => [...rows, { id: crypto.randomUUID(), path: "", content: "" }])}
      >
        ファイルを追加
      </Button>
      <div>
        <Button disabled={sender.pending} type="submit">
          保存
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
          <SkillForm name={name} detail={detail.data} sender={sender} />
        ) : (
          <div className="space-y-3 min-w-0">
            <h2 className="text-lg font-semibold break-all">{detail.data.name}</h2>
            <Markdown source={detail.data.skill_md} />
            {!!detail.data.files?.length && (
              <div>
                <h3 className="font-semibold">付属ファイル</h3>
                <ul>
                  {detail.data.files.map((file) => (
                    <li className="break-all" key={file}>
                      {file}
                    </li>
                  ))}
                </ul>
              </div>
            )}
            <Link className="underline inline-flex min-w-11 min-h-11 items-center" to={href(name, true)}>
              編集
            </Link>
            <Button
              disabled={sender.pending}
              onClick={() =>
                void sender
                  .run([{ id: name, method: "DELETE", path: `/api/skills/${encodeURIComponent(name)}` }])
                  .then((results) => {
                    if (results[0]?.ok) router.history.push(href(), {});
                  })
              }
            >
              削除
            </Button>
            {result?.status === 422 && <ActionResultView result={result} />}
          </div>
        ))}
    </FetchFrame>
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
      <nav className="flex flex-wrap gap-3">
        <Link className="underline inline-flex min-w-11 min-h-11 items-center" to="/knowledge">
          知識に戻る
        </Link>
        <Link className="underline inline-flex min-w-11 min-h-11 items-center" to={href() + "?create=1"}>
          作成
        </Link>
      </nav>
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
      <div className="grid min-w-0 gap-4 lg:grid-cols-[minmax(12rem,1fr)_minmax(0,2fr)]">
        <section className="min-w-0 rounded border p-3">
          <h2 className="font-semibold">skill 一覧</h2>
          <FetchFrame query={list}>
            <ul className="space-y-2">
              {list.data?.items.map((item) => (
                <li key={item.name}>
                  <Link className="underline inline-flex min-w-11 min-h-11 items-center break-all" to={href(item.name)}>
                    {item.name}
                  </Link>
                  <p className="text-sm break-words">{item.description}</p>
                </li>
              ))}
            </ul>
            {list.data?.items.length === 0 && <p>skill はありません。</p>}
          </FetchFrame>
        </section>
        <section className="min-w-0 rounded border p-3">
          {create ? (
            <>
              <h2 className="font-semibold">skill の作成</h2>
              <SkillForm sender={sender} />
            </>
          ) : name ? (
            <SkillDetail key={name} name={name} edit={edit} sender={sender} />
          ) : (
            <p>skill を選択してください。</p>
          )}
        </section>
      </div>
    </ScreenFrame>
  );
}
