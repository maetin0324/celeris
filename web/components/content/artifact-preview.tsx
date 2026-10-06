import { useId, useState } from "react";
import { buttonVariants } from "../ui/button";
import { artifactKind, previewable } from "./artifact-kind";
import { HtmlViewer, ImageViewer, MarkdownViewer, PdfViewer, TextViewer } from "./artifact-viewers";

/**
 * 成果物の名前・開閉・新しいタブ・download（/artifacts・/tasks/:id の成果物で共有）。
 * 形式ごとの本文は artifact-viewers.tsx。HTML・SVG・PDF は gateway の `view=1`（CSP sandbox 等）で読み、
 * 表に無い形式は download だけ（H8、ADR 2026-10-05-web-artifact-inline-view）。
 */
export function ArtifactPreview({
  taskId,
  idx,
  name,
  chunkBytes,
}: {
  taskId: string;
  idx: number;
  name: string;
  /** text の 1 回の範囲取得の大きさ（試験で小さくする）。 */
  chunkBytes?: number;
}) {
  const href = `/files/tasks/${encodeURIComponent(taskId)}/artifacts/${idx}`;
  const view = `${href}?view=1`;
  const kind = artifactKind(name);
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const action = buttonVariants({ variant: "secondary", size: "sm" });
  return (
    <div className="flex min-w-0 flex-col gap-2" data-artifact-kind={kind}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="min-w-0 break-all">{name}</span>
        {previewable(kind) ? (
          <>
            <button
              type="button"
              className={action}
              onClick={() => setOpen((value) => !value)}
              aria-expanded={open}
              aria-controls={open ? bodyId : undefined}
            >
              {open ? "本文を閉じる" : "本文をここで見る"}
            </button>
            <a className={action} href={view} target="_blank" rel="noopener noreferrer">
              新しいタブで開く
            </a>
          </>
        ) : null}
        <a className={action} href={`${href}?download=1`} download>
          ダウンロード
        </a>
      </div>
      {open && previewable(kind) ? (
        <div id={bodyId} className="min-w-0">
          {kind === "markdown" ? (
            <MarkdownViewer href={href} />
          ) : kind === "text" ? (
            <TextViewer href={href} name={name} chunkBytes={chunkBytes} />
          ) : kind === "image" ? (
            <ImageViewer src={href} name={name} />
          ) : kind === "svg" ? (
            <ImageViewer src={view} name={name} />
          ) : kind === "pdf" ? (
            <PdfViewer src={view} name={name} />
          ) : (
            <HtmlViewer src={view} name={name} />
          )}
        </div>
      ) : null}
    </div>
  );
}
