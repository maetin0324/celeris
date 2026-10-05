import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { apiGet } from "../api/client";
import type { ProjectList } from "../api/generated/types";
import { notificationsUnreadBadge } from "../api/queries/badges";
import { inboxItemsQuery, unreadCountQuery } from "../api/queries/inbox-notifications";
import { projectKeys } from "../api/queries/keys";
import { useConnectionState } from "../api/realtime/use-realtime";
import { ScreenFrame } from "../components/shell/screen-frame";
import { Badge } from "../components/ui/badge";
import { shortId } from "../components/ui/short-id";
import { ConsoleView, normalizeScope } from "../features/console/console-view";
import { ConsoleRegion } from "../features/home/console-region";
import { useUnconfirmedConnection } from "../features/home/use-unconfirmed";
import { optionalString } from "../lib/search";
import { formatAbsolute, formatRelative, serverNowMs } from "../lib/time";

// R01 /（Console。中身は features/console）。会話は features/home の枠の中で scroll し、ページは viewport に収める。
export const Route = createFileRoute("/")({
  validateSearch: (search: Record<string, unknown>): { scope?: string } => ({
    scope: optionalString(search.scope),
  }),
  component: Screen,
});

const entry =
  "inline-flex min-h-11 items-center gap-2 rounded-md border border-input bg-surface px-3 text-label font-medium text-foreground hover:bg-accent";

/**
 * 判断待ちの期限順の入口と未読通知への短い入口。通知件数は shell の cache を読む。
 * 電話幅で未確認の間は、黄帯の文を短くし期限の近い判断待ちを最も近い 1 件に絞る（件数は受信箱の入口が示す）。
 * 黄帯と 3 件で入口が 480px を超え、初期 viewport の会話が固定の送信欄の上に約 50px しか見えなかった（fix-r7）。
 */
function HomeEntries() {
  const inbox = useQuery(inboxItemsQuery());
  const projects = useQuery({
    queryKey: projectKeys.list(),
    queryFn: ({ signal }) => apiGet<ProjectList>("/api/projects", signal),
  });
  const connection = useConnectionState();
  const unread = useQuery({ ...unreadCountQuery, ...notificationsUnreadBadge, enabled: false });
  const urgent = [...(inbox.data?.items ?? [])]
    .sort(
      (a, b) =>
        (a.due_at ? new Date(a.due_at).getTime() : Infinity) - (b.due_at ? new Date(b.due_at).getTime() : Infinity),
    )
    .slice(0, 3);
  const unconfirmed = useUnconfirmedConnection(connection) || inbox.isError;
  const projectTitles = new Map(projects.data?.items.map((project) => [project.id, project.title]));
  return (
    <nav aria-label="受信箱と通知" className="flex min-w-0 flex-col gap-3 max-md:gap-2">
      {unconfirmed ? (
        <p role="status" className="rounded-md bg-warning p-3 text-warning-foreground max-md:py-2 max-md:text-label">
          接続を確認しています。判断待ちと会話の最新の状態は未確認です。
          <span className="max-md:hidden">再接続後に受信箱を開いて確かめてください。</span>
        </p>
      ) : null}
      <div className="rounded-md border border-input bg-surface p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-section font-semibold">
            判断待ち {inbox.data ? `${inbox.data.counts.total} 件` : "を確認中"}
          </h2>
          {unconfirmed ? <Badge tone="warning">未確認</Badge> : null}
          <Link to="/inbox" className={entry}>
            受信箱 {/* 電話幅では見出しと同じ件数を省き、見出し・未確認・入口を 1 行に収める。 */}
            <span className="tabular-nums text-muted-foreground max-md:hidden">
              判断待ち {inbox.data?.counts.total ?? 0} 件
            </span>
          </Link>
        </div>
        {urgent.length > 0 ? (
          <ul className="mt-2 flex flex-col divide-y divide-border" aria-label="期限の近い判断待ち">
            {urgent.map((item, index) => {
              const soon = item.due_at && new Date(item.due_at).getTime() - serverNowMs() < 86_400_000;
              return (
                <li
                  key={item.id}
                  className={`flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 py-2${unconfirmed ? (index > 0 ? " max-md:hidden" : " max-md:border-b-0") : ""}`}
                >
                  <span className="min-w-0 basis-full break-words font-medium md:basis-0 md:flex-1">{item.title}</span>
                  {item.due_at ? (
                    <span className="text-label">
                      <time dateTime={item.due_at} title={formatAbsolute(item.due_at)}>
                        {formatRelative(item.due_at)}
                      </time>
                      {soon ? <Badge tone="warning">まもなく期限</Badge> : null}
                    </span>
                  ) : (
                    <span className="text-label text-muted-foreground">期限なし</span>
                  )}
                  {item.project_id ? (
                    <span className="text-label text-muted-foreground">
                      案件 {projectTitles.get(item.project_id) ?? shortId(item.project_id)}
                    </span>
                  ) : null}
                </li>
              );
            })}
          </ul>
        ) : inbox.data ? (
          <p className="mt-2 text-label text-muted-foreground">いま決めることはありません。</p>
        ) : null}
      </div>
      <ul className="flex flex-wrap gap-2">
        <li>
          <Link to="/notifications" className={entry}>
            通知
            {unread.data !== undefined && (
              <span className="tabular-nums text-muted-foreground">未読 {unread.data} 件</span>
            )}
          </Link>
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
      <ConsoleRegion>
        <ConsoleView scope={scope} label={scope === "all" ? "CoS" : scope} contained />
      </ConsoleRegion>
    </ScreenFrame>
  );
}
