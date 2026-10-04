import type { UnreadCountView } from "../../api/generated/types";

// ブラウザ通知の規則（ADR-0133 D5 の未読数へ寄せた）。根拠は `GET /notifications/unread-count`。
// 未読の束の出来事の和（events）が前に見た値より増えたときだけ知らせる。減ったら黙って基準を下げる。

/** 前に見た出来事の数と今の値から、知らせるかを決める。`seen` が無い（初回）は 0 とみなす。 */
export function shouldNotify(view: UnreadCountView | null | undefined, seen: number | null): boolean {
  if (!view || view.unread <= 0) return false;
  return view.events > (seen ?? 0);
}

export function notificationBody(view: UnreadCountView): string {
  const bad = view.by_kind.bad_news ?? 0;
  return bad > 0 ? `悪い知らせ ${bad} 件 / 未読の通知 ${view.unread} 件` : `未読の通知 ${view.unread} 件`;
}
