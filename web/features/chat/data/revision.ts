// 発言の編集・撤回と、引用の状態の読み直し（ADR 2026-11-21-chat-discussion-mode D5・D6・D9）。
// - 操作を出すのは、人（`human:`）が書いた取り次ぎでない発言で、表示している人（viewer）が書いたものだけ。
//   CoS・システム・operator・MCP・取り次ぎ・主体の記録の無い発言には出さない（daemon も 403 にする。D5.1）。
// - 引用の状態は daemon が導いて GET と SSE の message に付ける。画面に出した後で引いた発言が編集・撤回されたら、
//   reducer が同じ規則（task_core::chat_citation::citation_state の retracted → edited の順）で読み直す。
//   本文の sha256 は画面では照合しない（revision が同じなら daemon の判定をそのまま使う）。

import type {
  ChatCitationState,
  ChatCitationView,
  ChatMessage,
  ChatMessageRevisedData,
} from "../../../api/generated/types";

/** D5.2: message の revision（wire は 1 のとき省く）。 */
export function messageRevision(message: Pick<ChatMessage, "revision">): number {
  return message.revision ?? 1;
}

/** 引いた発言の今の事実（revision と撤回の有無）。 */
export type CitedFacts = { revision: number; retracted: boolean };

export function citedFactsOfMessage(message: ChatMessage): CitedFacts {
  return { revision: messageRevision(message), retracted: message.retracted != null };
}

export function citedFactsOfRevision(data: ChatMessageRevisedData): CitedFacts {
  return { revision: data.revision, retracted: data.kind === "retract" };
}

/** D6: 引いた発言の今の事実から導いた状態。撤回 → retracted、引いた版より新しい → edited、それ以外は daemon の判定のまま。 */
export function citationStateNow(view: ChatCitationView, cited: CitedFacts): ChatCitationState {
  if (cited.retracted) return "retracted";
  if (view.citation.revision < cited.revision) return "edited";
  return view.state;
}

/** message の引用のうち、この thread の `citedId` を引くものの状態を読み直す。変わらなければ同じ object を返す。 */
export function restateMessageCitations(
  message: ChatMessage,
  threadId: string,
  citedId: string,
  cited: CitedFacts,
): ChatMessage {
  const citations = message.citations;
  if (!citations || citations.length === 0) return message;
  let changed = false;
  const next = citations.map((view) => {
    if (view.citation.thread_id !== threadId || view.citation.message_id !== citedId) return view;
    const state = citationStateNow(view, cited);
    if (state === view.state) return view;
    changed = true;
    return { ...view, state };
  });
  return changed ? { ...message, citations: next } : message;
}

/** D5.1: 編集・撤回の操作を出してよい発言か。viewer は表示している人の主体（分かっていれば `human:<name>`）。 */
export function revisableBy(
  message: Pick<ChatMessage, "role" | "author" | "relay" | "state" | "retracted">,
  viewer: string | null,
): { edit: boolean; retract: boolean } {
  const none = { edit: false, retract: false };
  if (message.role !== "user" || message.relay != null || message.retracted != null) return none;
  const author = message.author ?? "";
  if (!author.startsWith("human:")) return none;
  if (viewer !== null && author !== viewer) return none;
  switch (message.state) {
    case "queued":
      // 届く前の発言は編集できる。撤回ではなく既存の取り消し（DELETE）を使う。
      return { edit: true, retract: false };
    case "completed":
    case "failed":
    case "interrupted":
      return { edit: true, retract: true };
    default:
      // running（run が読んでいる）・cancelled は daemon が 409 にする。
      return none;
  }
}

export type RevisionChange = { kind: "edit"; text: string } | { kind: "retract"; reason: string };

/** 編集・撤回の結果。conflict（409）は他の変更と衝突したので、再読込してから操作し直す。 */
export type RevisionOutcome =
  | { kind: "ok"; message: ChatMessage }
  | { kind: "conflict"; detail: string }
  | { kind: "forbidden"; detail: string }
  | { kind: "error"; detail: string };

export const REVISION_CONFLICT_TEXT =
  "この発言は他で変更されたか、CoS が読んでいます。再読込してから操作し直してください。";
export const REVISION_FORBIDDEN_TEXT = "この発言は書いた人だけが編集・撤回できます。";
