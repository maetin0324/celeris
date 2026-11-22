// CoS の返事に添えた案・要約の file の引用（ADR 2026-11-21-chat-discussion-mode D6・D9）。
// file ごとに、引いた発言の状態（最新・編集された・撤回された・見つからない）と、その発言へ移る link を出す。
// 1 つでも最新でない file は失効の警告を出す（その案・要約は、今の議論の記録と食い違う）。
import type { MouseEvent } from "react";
import type { ChatCitationState, ChatMessage } from "../../../api/generated/types";
import { Badge, type BadgeTone } from "../../../components/ui/badge";
import { Notice } from "../../../components/ui/notice";
import {
  CITATION_STATE_LABEL,
  chatMessageElementId,
  citedMessageHref,
  type FileCitations,
  fileCitations,
} from "./workspace-links";

const stateTone: Record<ChatCitationState, BadgeTone> = {
  current: "success",
  edited: "warning",
  retracted: "danger",
  missing: "danger",
};

export const CITATION_STALE_TEXT =
  "失効: 引いた発言が編集・撤回された（または見つからない）ため、この file は今の議論の記録と食い違います。";

/** 同じ thread の吹き出しへは画面内で移る（読み込んでいない古い発言なら link の既定の動き）。 */
function jumpWithin(event: MouseEvent<HTMLAnchorElement>, messageId: string) {
  const target = document.getElementById(chatMessageElementId(messageId));
  if (!target) return;
  event.preventDefault();
  target.scrollIntoView({ block: "center", behavior: "smooth" });
  target.focus({ preventScroll: true });
}

function FileCitationItem({ entry, threadId }: { entry: FileCitations; threadId: string }) {
  return (
    <li data-slot="chat-citation-file" data-stale={entry.stale ? "true" : "false"} className="flex flex-col gap-2">
      <p className="break-all font-mono text-label">{entry.path}</p>
      {entry.stale ? (
        <Notice data-slot="chat-citation-stale" tone="warning">
          {CITATION_STALE_TEXT}
        </Notice>
      ) : null}
      <ul aria-label={`${entry.path} が引いた発言`} className="flex flex-col gap-1">
        {entry.citations.map((view) => {
          const { citation } = view;
          const within = citation.thread_id === threadId;
          return (
            <li
              key={`${citation.thread_id}:${citation.message_id}:${citation.revision}`}
              data-slot="chat-citation"
              data-state={view.state}
              className="flex flex-wrap items-center gap-2 text-label"
            >
              <Badge tone={stateTone[view.state]}>{CITATION_STATE_LABEL[view.state]}</Badge>
              <a
                href={citedMessageHref(citation, threadId)}
                onClick={within ? (event) => jumpWithin(event, citation.message_id) : undefined}
                className="underline underline-offset-2 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
              >
                {`引いた発言（rev ${citation.revision}）へ${within ? "" : "（統括の会話）"}`}
              </a>
            </li>
          );
        })}
      </ul>
    </li>
  );
}

/** 返事の案・要約の引用の一覧。引用の無い返事には何も出さない。 */
export function CitationList({
  message,
}: {
  message: Pick<ChatMessage, "thread_id" | "citations" | "workspace_files">;
}) {
  const entries = fileCitations(message);
  if (entries.length === 0) return null;
  const stale = entries.some((entry) => entry.stale);
  return (
    <section
      data-slot="chat-citations"
      data-stale={stale ? "true" : "false"}
      aria-label="案・要約の出典"
      className="flex w-full min-w-0 flex-col gap-2 rounded-md border border-border px-3 py-2"
    >
      <p className="text-label text-muted-foreground">案・要約の出典</p>
      <ul className="flex flex-col gap-3">
        {entries.map((entry) => (
          <FileCitationItem key={entry.path} entry={entry} threadId={message.thread_id} />
        ))}
      </ul>
    </section>
  );
}
