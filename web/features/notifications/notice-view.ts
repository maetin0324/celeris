// 通知の束（ADR-0133 D5 の Notice）を画面の言葉と行き先にする純関数。
import type { Notice, NoticeKind } from "../../api/generated/types";
import type { BadgeTone } from "../../components/ui/badge";

export const noticeKinds: readonly { key: NoticeKind; label: string; tone: BadgeTone }[] = [
  { key: "bad_news", label: "悪い知らせ", tone: "danger" },
  { key: "requeue_limit_near", label: "再試行の上限が近い", tone: "warning" },
  { key: "report", label: "報告", tone: "info" },
  { key: "secretary_reply", label: "秘書の返信", tone: "info" },
  { key: "task_done", label: "task の完了", tone: "success" },
  { key: "delivery", label: "配送", tone: "neutral" },
  { key: "release", label: "リリース", tone: "neutral" },
  { key: "cron_run", label: "定期実行", tone: "neutral" },
  { key: "auto_recovered", label: "自動復旧", tone: "neutral" },
];

const byKey = new Map(noticeKinds.map((kind) => [kind.key, kind]));

export function isNoticeKind(value: string | undefined): value is NoticeKind {
  return value !== undefined && byKey.has(value as NoticeKind);
}

export function noticeKindView(kind: NoticeKind): { label: string; tone: BadgeTone } {
  return byKey.get(kind) ?? { label: kind, tone: "neutral" };
}

export type NoticeLinkView = { href: string; label: string };

/** 束の行き先。`links` を先に、`target`（task・報告・案件）と `task_id` を後に。同じ href は 1 つにまとめる。 */
export function noticeLinks(notice: Notice): NoticeLinkView[] {
  const out: NoticeLinkView[] = [];
  const push = (href: string, label: string) => {
    if (href.startsWith("/") && !href.startsWith("//") && !out.some((link) => link.href === href))
      out.push({ href, label });
  };
  for (const link of notice.links ?? []) push(link.href, link.label);
  const target = notice.target;
  if (target?.kind === "report") push(`/reports?report=${encodeURIComponent(target.id)}`, "報告の本文");
  if (target?.kind === "task") push(`/tasks/${encodeURIComponent(target.id)}`, `task ${target.id}`);
  if (target?.kind === "project") push(`/projects/${encodeURIComponent(target.id)}`, `案件 ${target.id}`);
  if (notice.task_id) push(`/tasks/${encodeURIComponent(notice.task_id)}`, `task ${notice.task_id}`);
  if (notice.kind === "report" && !target) push("/reports", "報告の一覧");
  return out;
}
