// メッセージ表示の小部品: tool の折り畳み行・添付・考え中・「最新へ」。
import { useId, useState } from "react";
import type { ChatAttachment, ChatStatusData } from "../../../api/generated/types";
import { CodeBlock } from "../../../components/ui/code-block";
import { Icon } from "../../../components/ui/icon";
import { cn } from "../../../lib/utils";
import { chatPaths } from "../data/client";
import type { ToolEntry } from "../data/reducer";
import { formatBytes, statusLabel } from "./logic";

function toolOutcome(tool: ToolEntry): "running" | "completed" | "failed" {
  if (tool.state === "running") return "running";
  return tool.error || tool.state === "failed" ? "failed" : "completed";
}

const outcomeLabel = { running: "実行中", completed: "成功", failed: "失敗" } as const;
const rowClass = "flex min-h-11 w-full min-w-0 items-center gap-2 rounded-md px-3 py-2 text-label";

/** tool call 1 件を「名前・短い要約・成功/失敗」の 1 行に折り畳む。detail は展開したときだけ描く。 */
export function ToolCall({ tool, defaultOpen = false }: { tool: ToolEntry; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  const detailId = useId();
  const outcome = toolOutcome(tool);
  const hasDetail = (tool.detail ?? "") !== "";
  const row = (
    <>
      <span className="shrink-0 font-mono font-medium">{tool.name}</span>
      <span className="min-w-0 flex-1 truncate text-muted-foreground">{tool.summary}</span>
      <span
        className={cn(
          "shrink-0 font-medium",
          outcome === "failed" && "text-danger",
          outcome === "completed" && "text-success",
          outcome === "running" && "text-running",
        )}
      >
        {outcomeLabel[outcome]}
      </span>
    </>
  );
  return (
    <div data-slot="chat-tool" data-state={outcome} className="min-w-0 rounded-md border border-border bg-surface">
      {hasDetail ? (
        <button
          type="button"
          aria-expanded={open}
          aria-controls={open ? detailId : undefined}
          onClick={() => setOpen((value) => !value)}
          className={cn(
            rowClass,
            "text-left hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
          )}
        >
          <Icon name={open ? "chevron-down" : "chevron-right"} size="sm" />
          {row}
        </button>
      ) : (
        <div className={rowClass}>{row}</div>
      )}
      {hasDetail && open ? (
        <div id={detailId} className="px-3 pb-3">
          <CodeBlock label={`${tool.name} の詳細`} wrap>
            {tool.detail}
          </CodeBlock>
          {tool.truncated ? <p className="mt-1 text-label text-muted-foreground">詳細は途中で省略しています</p> : null}
        </div>
      ) : null}
    </div>
  );
}

export function ToolCallList({ tools }: { tools: ToolEntry[] }) {
  if (tools.length === 0) return null;
  return (
    <ul aria-label="道具の実行" className="min-w-0 space-y-1">
      {tools.map((tool) => (
        <li key={tool.call_id}>
          <ToolCall tool={tool} />
        </li>
      ))}
    </ul>
  );
}

/** 添付の一覧。画像は preview_url の縮小、他は原名・容量とダウンロード link。未取得の id は名前の代わりに読み込み中。 */
export function AttachmentList({
  ids,
  attachments,
}: {
  ids: string[];
  attachments: Record<string, ChatAttachment | undefined>;
}) {
  if (ids.length === 0) return null;
  return (
    <ul aria-label="添付" className="flex min-w-0 flex-wrap gap-2">
      {ids.map((id) => {
        const attachment = attachments[id];
        if (!attachment)
          return (
            <li
              key={id}
              className="min-h-11 rounded-md border border-border px-3 py-2 text-label text-muted-foreground"
            >
              添付を読み込み中
            </li>
          );
        if (attachment.state === "deleted")
          return (
            <li
              key={id}
              className="min-h-11 rounded-md border border-border px-3 py-2 text-label text-muted-foreground"
            >
              {`${attachment.name}（削除済み）`}
            </li>
          );
        const size = formatBytes(attachment.size_bytes);
        const image = attachment.media_type.startsWith("image/") && attachment.preview_url;
        return (
          <li key={id} data-slot="chat-attachment" className="min-w-0 max-w-full">
            <a
              href={chatPaths.attachmentContent(id)}
              download={attachment.name}
              className="flex min-h-11 min-w-0 max-w-full items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-label hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
            >
              {image ? (
                <img
                  src={chatPaths.attachmentPreview(id)}
                  alt={attachment.name}
                  loading="lazy"
                  className="max-h-40 max-w-full rounded-sm object-contain"
                />
              ) : (
                <Icon name="folder" size="sm" />
              )}
              <span className="min-w-0 break-all">
                {image ? <span className="sr-only">{attachment.name} </span> : attachment.name}
                <span className="ml-1 text-muted-foreground">{size}</span>
                <span className="sr-only"> をダウンロード</span>
              </span>
            </a>
          </li>
        );
      })}
    </ul>
  );
}

/** 稼働中の run の公開要約（thinking は「考え中」）。読み上げは list の live region に任せる。 */
export function StatusLine({ status }: { status: Pick<ChatStatusData, "phase" | "summary"> }) {
  return (
    <p
      data-slot="chat-status"
      data-phase={status.phase}
      className="flex items-center gap-2 text-label text-muted-foreground"
    >
      <span aria-hidden="true" className="inline-block size-2 animate-pulse rounded-full bg-running" />
      {statusLabel(status)}
    </p>
  );
}

/** 上へ scroll 中に出す「最新へ」。未読があれば件数を出す。 */
export function JumpToLatest({ unread, onClick }: { unread: number; onClick: () => void }) {
  const label = unread > 0 ? `最新へ（未読 ${unread} 件）` : "最新へ";
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      className="inline-flex min-h-11 min-w-11 items-center gap-1 rounded-md border border-border bg-popover px-3 py-2 text-label text-popover-foreground shadow-sm hover:bg-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
    >
      <Icon name="chevron-down" size="sm" />
      <span aria-hidden="true">最新へ</span>
      {unread > 0 ? (
        <span aria-hidden="true" className="rounded-sm bg-primary px-1 text-primary-foreground">
          {unread}
        </span>
      ) : null}
    </button>
  );
}
