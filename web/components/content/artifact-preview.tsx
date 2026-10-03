import { useEffect, useState } from "react";
import { ErrorNotice } from "../fetch-state/fetch-frame";
import { buttonClassName } from "../ui/button";
import { Markdown } from "./markdown";

/** 承認項目と後続の成果物画面で共有。Markdown のみ同一画面で表示し、他形式は download リンク。 */
export function ArtifactPreview({ taskId, idx, name }: { taskId: string; idx: number; name: string }) {
  const href = `/files/tasks/${encodeURIComponent(taskId)}/artifacts/${idx}`;
  const markdown = /\.md(?:own)?$/i.test(name);
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState<string>();
  const [error, setError] = useState(false);
  const [attempt, setAttempt] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: attempt は明示的な再取得操作で effect をやり直すための世代。
  useEffect(() => {
    if (!open || !markdown) return;
    const controller = new AbortController();
    setError(false);
    setContent(undefined);
    fetch(href, { signal: controller.signal, credentials: "same-origin" })
      .then((response) => {
        if (!response.ok) throw new Error("preview failed");
        return response.text();
      })
      .then((text) => {
        if (!controller.signal.aborted) setContent(text);
      })
      .catch(() => {
        if (!controller.signal.aborted) setError(true);
      });
    return () => controller.abort();
  }, [open, href, markdown, attempt]);
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
        <a className={buttonClassName} href={`${href}?download=1`} download>
          ダウンロード
        </a>
      )}
      {open && markdown && (
        <div className="max-h-96 overflow-auto rounded-sm border border-border p-3">
          {error ? (
            <ErrorNotice subject="本文" onRetry={() => setAttempt((value) => value + 1)} />
          ) : content === undefined ? (
            <p role="status" aria-busy="true">
              読み込み中…
            </p>
          ) : (
            <Markdown source={content} />
          )}
        </div>
      )}
    </div>
  );
}
