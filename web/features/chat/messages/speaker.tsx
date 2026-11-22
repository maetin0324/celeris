// 発言の主体と、人が選んだ発言の扱いの表示（ADR 2026-11-21-chat-discussion-mode D7・D9、ADR 2026-10-27-relayed-instructions D1）。
// 主体は message の `author`・`relay` の記録だけから導く（本文からは読まない）。`human:` でない主体の発言を「あなた」と表示しない。
import type { ChatDiscussionMode, ChatMessage } from "../../../api/generated/types";
import { Badge } from "../../../components/ui/badge";
import { DISCUSSION_MODE_LABEL } from "../data/send";

export type SpeakerKind = "human" | "operator" | "mcp" | "cos" | "system" | "unknown";

export type Speaker = { kind: SpeakerKind; label: string };

const PRINCIPAL_PREFIX: ReadonlyArray<readonly [string, SpeakerKind]> = [
  ["human:", "human"],
  ["operator:", "operator"],
  ["mcp:", "mcp"],
  ["cos:", "cos"],
];

function principal(actor: string | null | undefined): { kind: SpeakerKind; name: string } | null {
  if (!actor) return null;
  for (const [prefix, kind] of PRINCIPAL_PREFIX)
    if (actor.startsWith(prefix)) return { kind, name: actor.slice(prefix.length) };
  return { kind: "unknown", name: actor };
}

/** role ごとの主体の名前（人の発言は主体の記録が人のときだけこの名前。messageSpeaker）。 */
export const roleName: Record<ChatMessage["role"], string> = { user: "あなた", assistant: "CoS", system: "システム" };

/** 発言の主体（D7 の表）。人・operator・MCP の client・CoS（返事と統括 CoS の取り次ぎ）・システムを分ける。 */
export function messageSpeaker(message: Pick<ChatMessage, "role" | "author" | "relay">): Speaker {
  if (message.role === "assistant") return { kind: "cos", label: roleName.assistant };
  if (message.role === "system") return { kind: "system", label: roleName.system };
  const author = principal(message.author);
  // author の無い人の発言は、主体を記録する前（ADR 2026-10-27 より前）の web の送信。今までどおり「あなた」。
  if (!author || author.kind === "human") return { kind: "human", label: roleName.user };
  const relayed = message.relay != null;
  switch (author.kind) {
    case "operator":
      return { kind: "operator", label: `operator ${author.name} ${relayed ? "が取り次いだ発言" : "の発言"}` };
    case "mcp":
      return { kind: "mcp", label: `MCP ${author.name} ${relayed ? "が取り次いだ発言" : "の発言"}` };
    case "cos":
      // 統括 CoS の取り次ぎ（chat.relay_forward）。元の人の発言は relay.source に残る。
      return {
        kind: "cos",
        label:
          principal(message.relay?.source?.author)?.kind === "human" ? "統括 CoS が取り次いだ人の発言" : "CoS の発言",
      };
    default:
      return { kind: "unknown", label: `記録の主体 ${author.name} の発言` };
  }
}

/** 発言の扱いの印。議論・案だけ出す（実行と未指定は印を出さない。mode を選ぶ前と同じ表示）。 */
export function discussionModeMark(mode: ChatDiscussionMode | null | undefined): string | undefined {
  return mode === "discuss" || mode === "propose" ? DISCUSSION_MODE_LABEL[mode] : undefined;
}

/** 人の側の発言の上の 1 行: 人以外の主体と、議論・案の印。どちらも無ければ何も出さない。 */
export function MessageMeta({
  message,
}: {
  message: Pick<ChatMessage, "role" | "author" | "relay" | "discussion_mode">;
}) {
  const speaker = message.role === "user" ? messageSpeaker(message) : undefined;
  const shownSpeaker = speaker && speaker.kind !== "human" ? speaker : undefined;
  const mark = discussionModeMark(message.discussion_mode);
  if (!shownSpeaker && !mark) return null;
  return (
    <div data-slot="chat-message-meta" className="flex flex-wrap items-center gap-2 text-label text-muted-foreground">
      {shownSpeaker ? (
        <span data-slot="chat-speaker" data-speaker={shownSpeaker.kind}>
          {shownSpeaker.label}
        </span>
      ) : null}
      {mark ? (
        <Badge tone="info" data-slot="chat-discussion-mode" data-mode={message.discussion_mode ?? undefined}>
          {mark}
        </Badge>
      ) : null}
    </div>
  );
}
