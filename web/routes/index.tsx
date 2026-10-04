import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, useRouter } from "@tanstack/react-router";
import type { MouseEvent } from "react";
import { inboxItemsBadge, notificationsUnreadBadge } from "../api/queries/badges";
import { inboxItemsQuery, unreadCountQuery } from "../api/queries/inbox-notifications";
import { ScreenFrame } from "../components/shell/screen-frame";
import { ConsoleView, normalizeScope } from "../features/console/console-view";
import { optionalString } from "../lib/search";

// R01 /（Console。中身は features/console）。
export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>): { scope?: string } => ({
    scope: optionalString(search.scope),
  }),
  component: Screen,
});

// 通知画面の URL（web ADR 2026-10-04 の notifications 葉が置く）。
const NOTIFICATIONS_HREF = "/notifications";
const entry =
  "inline-flex min-h-11 items-center gap-2 rounded-md border border-input bg-surface px-3 text-label font-medium text-foreground hover:bg-accent";

/**
 * 受信箱の判断待ちと未読通知への短い入口。件数は shell の常駐 query と同じ key の cache を読むだけで、
 * home の初期表示に request を足さない（enabled: false。cache が無い間は件数を出さない）。
 */
function HomeEntries() {
  const router = useRouter();
  const inbox = useQuery({ ...inboxItemsQuery(), ...inboxItemsBadge, enabled: false });
  const unread = useQuery({ ...unreadCountQuery, ...notificationsUnreadBadge, enabled: false });
  function openNotifications(event: MouseEvent<HTMLAnchorElement>) {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    router.history.push(NOTIFICATIONS_HREF, {});
  }
  return (
    <nav aria-label="受信箱と通知" className="min-w-0">
      <ul className="flex flex-wrap gap-2">
        <li>
          <Link to="/inbox" className={entry}>
            受信箱
            {inbox.data !== undefined && (
              <span className="tabular-nums text-muted-foreground">判断待ち {inbox.data} 件</span>
            )}
          </Link>
        </li>
        <li>
          <a href={NOTIFICATIONS_HREF} onClick={openNotifications} className={entry}>
            通知
            {unread.data !== undefined && (
              <span className="tabular-nums text-muted-foreground">未読 {unread.data} 件</span>
            )}
          </a>
        </li>
      </ul>
    </nav>
  );
}

function Screen() {
  const scope = normalizeScope(Route.useSearch().scope);
  return (
    <ScreenFrame title="ホーム" route="/">
      <HomeEntries />
      <ConsoleView scope={scope} label={scope === "all" ? "CoS" : scope} />
    </ScreenFrame>
  );
}
