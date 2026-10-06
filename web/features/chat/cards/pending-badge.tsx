import { Badge } from "../../../components/ui/badge";

// 受信箱 thread の人待ち件数（ADR 2026-10-05-cos-chat-home D5「人待ちはその件数だけ控えめに表示する」）。
// 0 件は何も出さない。色は控えめな info、件数は 99+ で止め、読み上げは件数の文 1 つにする。

export const PENDING_BADGE_MAX = 99;

export function pendingBadgeText(count: number): string | null {
  if (!Number.isFinite(count) || count <= 0) return null;
  return count > PENDING_BADGE_MAX ? `${PENDING_BADGE_MAX}+` : String(Math.floor(count));
}

export function PendingCountBadge({ count, className }: { count: number; className?: string }) {
  const text = pendingBadgeText(count);
  if (text === null) return null;
  return (
    <Badge tone="info" className={className}>
      <span aria-hidden="true">{text}</span>
      <span className="sr-only">人待ち {Math.floor(count)} 件</span>
    </Badge>
  );
}
