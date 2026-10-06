// メッセージ表示の純関数（ADR 2026-10-05-cos-chat-home D5 の 2・3 項目目）。DOM に依らず試験する。
// - 追従: 下端を見ているときだけ新着へ追従する。上へ scroll 中は位置を保ち、新着の件数を未読として数える。
// - 古い履歴: before_seq で上に足したら、足した高さだけ scrollTop をずらして見ている位置を保つ。
// - 読み上げ: 確定した返事（streaming が終わった message）だけを 1 件ずつまとめて読む。token ごとに読まない。
// - tool: message_id で返事に付け、無ければ同じ run の返事に付ける。どちらも無ければ末尾（稼働中の run）に出す。

import type { ChatMessage, ChatStatusData } from "../../../api/generated/types";
import type { ChatState, TimelineItem, ToolEntry } from "../data/reducer";

/** 下端からこの px 以内なら「下端を見ている」。慣性 scroll の端数で追従が外れないための遊び。 */
export const BOTTOM_THRESHOLD_PX = 48;

export type ScrollMetrics = { scrollTop: number; scrollHeight: number; clientHeight: number };

export function isAtBottom(metrics: ScrollMetrics, threshold = BOTTOM_THRESHOLD_PX): boolean {
  return metrics.scrollHeight - metrics.clientHeight - metrics.scrollTop <= threshold;
}

/** 上に頁を足した後、同じ内容が同じ位置に見える scrollTop。 */
export function scrollTopAfterPrepend(before: ScrollMetrics, afterScrollHeight: number): number {
  return before.scrollTop + (afterScrollHeight - before.scrollHeight);
}

export type FollowState = {
  /** 利用者が下端を見ているか（scroll のたびに更新）。 */
  atBottom: boolean;
  /** 下端を離れている間に末尾へ届いた項目の数（「最新へ」に出す）。 */
  unread: number;
  /** 前回見た timeline の key（末尾に足されたものを数えるため）。 */
  keys: string[];
};

export function initialFollowState(keys: string[] = []): FollowState {
  return { atBottom: true, unread: 0, keys };
}

export type FollowAction =
  | { type: "scrolled"; atBottom: boolean }
  | { type: "timeline"; items: Pick<TimelineItem, "key" | "kind">[] }
  | { type: "jumped" };

/** prev の末尾の項目より後ろに現れた key（上に足した古い履歴は含めない）。 */
export function appendedKeys(prev: string[], next: string[]): string[] {
  if (prev.length === 0) return [];
  const known = new Set(prev);
  let lastIndex = -1;
  for (let i = next.length - 1; i >= 0; i -= 1) {
    if (known.has(next[i] as string)) {
      lastIndex = i;
      break;
    }
  }
  return next.slice(lastIndex + 1).filter((key) => !known.has(key));
}

export type FollowDecision = { state: FollowState; scrollToBottom: boolean };

/**
 * 追従の判断。timeline が変わったら:
 * - 下端を見ていれば下端へ追従する（streaming の本文が伸びたときも）。
 * - 自分の送信（pending）が末尾に出たら、下端へ移る（送った内容を見せる）。
 * - それ以外で下端を離れていれば位置を保ち、末尾に増えた件数を未読に足す。
 */
export function followReducer(state: FollowState, action: FollowAction): FollowDecision {
  switch (action.type) {
    case "scrolled":
      return {
        state: { ...state, atBottom: action.atBottom, unread: action.atBottom ? 0 : state.unread },
        scrollToBottom: false,
      };
    case "jumped":
      return { state: { ...state, atBottom: true, unread: 0 }, scrollToBottom: true };
    case "timeline": {
      const keys = action.items.map((item) => item.key);
      const added = appendedKeys(state.keys, keys);
      const kinds = new Map(action.items.map((item) => [item.key, item.kind]));
      const ownSend = added.some((key) => kinds.get(key) === "pending");
      if (state.atBottom || ownSend) {
        return { state: { atBottom: true, unread: 0, keys }, scrollToBottom: true };
      }
      return { state: { ...state, keys, unread: state.unread + added.length }, scrollToBottom: false };
    }
    default:
      return { state, scrollToBottom: false };
  }
}

/** 読み上げる 1 件。確定した返事だけを、本文の先頭を短くまとめて読む。 */
export function announcementFor(message: ChatMessage, maxLength = 200): string {
  const who = message.role === "assistant" ? "CoS" : message.role === "user" ? "あなた" : "システム";
  const failed = message.state === "failed" ? "（失敗）" : message.state === "interrupted" ? "（中断）" : "";
  const text = message.text.replace(/\s+/g, " ").trim();
  const body = text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
  return `${who}${failed}: ${body}`;
}

export type AnnounceState = { announced: Set<string>; text: string };

/**
 * timeline から読み上げを決める。初回（snapshot）は既存の message を既読として何も読まない。
 * streaming 中の本文・draft は読まず、確定（running でない）になった返事だけを 1 回読む。
 */
export function nextAnnouncement(prev: AnnounceState | null, items: TimelineItem[]): AnnounceState {
  const settled = items.filter(
    (item): item is Extract<TimelineItem, { kind: "message" }> =>
      item.kind === "message" && !item.streaming && item.message.state !== "running" && item.message.role !== "user",
  );
  if (prev === null) return { announced: new Set(settled.map((item) => item.message.id)), text: "" };
  const fresh = settled.filter((item) => !prev.announced.has(item.message.id));
  if (fresh.length === 0) return prev;
  const announced = new Set(prev.announced);
  for (const item of fresh) announced.add(item.message.id);
  return { announced, text: fresh.map((item) => announcementFor(item.message)).join("\n") };
}

export type ToolGroups = { byMessage: Record<string, ToolEntry[]>; loose: ToolEntry[] };

/** tool を返事ごとに分ける。順序は届いた順（object の挿入順）。 */
export function groupTools(state: Pick<ChatState, "tools" | "messages" | "drafts">): ToolGroups {
  const byRun = new Map<string, string>();
  const ids = [
    ...Object.values(state.messages)
      .filter((message) => message.role === "assistant")
      .sort((a, b) => a.seq - b.seq)
      .map((message) => ({ id: message.id, run: message.run_id ?? null })),
    ...Object.values(state.drafts).map((draft) => ({ id: draft.id, run: draft.run_id })),
  ];
  // 同じ run に返事が複数あれば最初の返事に付ける。
  for (const { id, run } of ids) if (run && !byRun.has(run)) byRun.set(run, id);
  const known = new Set(ids.map(({ id }) => id));
  const byMessage: Record<string, ToolEntry[]> = {};
  const loose: ToolEntry[] = [];
  for (const tool of Object.values(state.tools)) {
    const target =
      (tool.message_id && known.has(tool.message_id) ? tool.message_id : null) ??
      (tool.run_id ? byRun.get(tool.run_id) : undefined);
    if (target) byMessage[target] = [...(byMessage[target] ?? []), tool];
    else loose.push(tool);
  }
  return { byMessage, loose };
}

/** thinking の表示。公開要約があればそれ、無ければ phase の既定文。 */
export function statusLabel(status: Pick<ChatStatusData, "phase" | "summary">): string {
  const summary = status.summary.trim();
  if (summary !== "") return summary;
  switch (status.phase) {
    case "queued":
      return "順番待ち";
    case "starting":
      return "開始中";
    case "thinking":
      return "考え中";
    case "working":
      return "作業中";
    case "waiting":
      return "待機中";
    default:
      return "考え中";
  }
}

/** 容量の表示（1024 進、小数 1 桁）。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** 一番古い確定 message の seq（before_seq に渡す）。無ければ undefined。 */
export function oldestSeq(state: Pick<ChatState, "messages">): number | undefined {
  let min: number | undefined;
  for (const message of Object.values(state.messages)) if (min === undefined || message.seq < min) min = message.seq;
  return min;
}
