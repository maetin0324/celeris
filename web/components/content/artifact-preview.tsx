import { useEffect, useState } from "react";
import { buttonClassName } from "../ui/button";
import { Markdown } from "./markdown";

/** 承認項目と後続の成果物画面で共有。Markdown のみ同一画面で表示し、他形式は download リンク。 */
export function ArtifactPreview({ taskId, idx, name }: { taskId: string; idx: number; name: string }) {
  const href = `/files/tasks/${encodeURIComponent(taskId)}/artifacts/${idx}`;
  const markdown = /\.md(?:own)?$/i.test(name);
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState<string>();
  const [error, setError] = useState(false);
  useEffect(() => {
    if (!open || !markdown) return;
    const controller = new AbortController();
    fetch(href, { signal: controller.signal, credentials: "same-origin" })
      .then((response) => {
        if (!response.ok) throw new Error("preview failed");
        return response.text();
      })
      .then(setContent)
      .catch(() => {
        if (!controller.signal.aborted) setError(true);
      });
    return () => controller.abort();
  }, [open, href, markdown]);
  return (
    <div className="min-w-0 break-words">
      <span>{name}</span>{" "}
      {markdown ? (
        <button
          type="button"
          className={buttonClassName}
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
        >
          {open ? "本文を閉じる" : "本文をここで見る"}
        </button>
      ) : (
        <a className="underline" href={`${href}?download=1`} download>
          ダウンロード
        </a>
      )}
      {open && markdown && (
        <div className="max-h-96 overflow-auto rounded-sm border border-border p-3">
          {error ? (
            <p role="alert">preview を取得できません</p>
          ) : content === undefined ? (
            <p>読み込み中…</p>
          ) : (
            <Markdown source={content} />
          )}
        </div>
      )}
    </div>
  );
}
