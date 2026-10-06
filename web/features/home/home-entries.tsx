import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { apiGet } from "../../api/client";
import type { ProjectList } from "../../api/generated/types";
import { notificationsUnreadBadge } from "../../api/queries/badges";
import { inboxItemsQuery, unreadCountQuery } from "../../api/queries/inbox-notifications";
import { projectKeys } from "../../api/queries/keys";
import { useConnectionState } from "../../api/realtime/use-realtime";
import { Badge } from "../../components/ui/badge";
import { shortId } from "../../components/ui/short-id";
import { formatAbsolute, formatRelative, serverNowMs } from "../../lib/time";
import { useUnconfirmedConnection } from "./use-unconfirmed";

// ホームの上部: 受信箱と通知の入口と期限の近い判断待ち（routes/index.tsx から分けた。route は 150 行以内）。
const entry =
  "inline-flex min-h-11 items-center gap-2 rounded-md border border-input bg-surface px-3 text-label font-medium text-foreground hover:bg-accent";

/** 電話幅（md 未満）で見せる期限の近い判断待ちの件数。残りは「ほか n 件（受信箱へ）」の入口が示す。 */
const NARROW_URGENT = 2;

/**
 * 判断待ちの期限順の入口と未読通知への短い入口。通知件数は shell の cache を読む。
 * 電話幅で未確認の間は、黄帯の文を短くし期限の近い判断待ちを最も近い 1 件に絞る（件数は受信箱の入口が示す）。
 * 黄帯と 3 件で入口が 480px を超え、初期 viewport の会話が固定の送信欄の上に約 50px しか見えなかった（fix-r7）。
 * 画面下のタブ（64px）が入った後の電話幅では、判断待ちを 2 件（未確認の間は 1 件）の 1 行ずつにし、通知の入口を
 * 見出しの行へ、受信箱の入口を「ほか n 件（受信箱へ）」として一覧の下へ置く。md 以上の並びは変えない。
 */
export function HomeEntries() {
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
  const narrowShown = Math.min(urgent.length, unconfirmed ? 1 : NARROW_URGENT);
  const narrowRest = Math.max(0, (inbox.data?.counts.total ?? 0) - narrowShown);
  const notifications = (className: string) => (
    <Link to="/notifications" className={`${entry} ${className}`}>
      通知
      {unread.data !== undefined && <span className="tabular-nums text-muted-foreground">未読 {unread.data} 件</span>}
    </Link>
  );
  return (
    <nav aria-label="受信箱と通知" className="flex min-w-0 flex-col gap-3 max-md:gap-2">
      {unconfirmed ? (
        <p role="status" className="rounded-md bg-warning p-3 text-warning-foreground max-md:py-2 max-md:text-label">
          <span className="max-md:hidden">接続を確認しています。</span>
          判断待ちと会話の最新の状態は未確認です。
          <span className="max-md:hidden">再接続後に受信箱を開いて確かめてください。</span>
        </p>
      ) : null}
      <div className="rounded-md border border-input bg-surface p-3 max-md:p-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-section font-semibold">
            判断待ち {inbox.data ? `${inbox.data.counts.total} 件` : "を確認中"}
          </h2>
          {/* 電話幅では上の黄帯が同じことを示すので、見出しの行を通知の入口に空ける。 */}
          {unconfirmed ? (
            <Badge tone="warning" className="max-md:hidden">
              未確認
            </Badge>
          ) : null}
          <Link to="/inbox" className={`${entry} max-md:hidden`}>
            受信箱{" "}
            <span className="tabular-nums text-muted-foreground">判断待ち {inbox.data?.counts.total ?? 0} 件</span>
          </Link>
          {notifications("md:hidden")}
        </div>
        {urgent.length > 0 ? (
          <ul className="mt-2 flex flex-col divide-y divide-border max-md:mt-1" aria-label="期限の近い判断待ち">
            {urgent.map((item, index) => {
              const soon = item.due_at && new Date(item.due_at).getTime() - serverNowMs() < 86_400_000;
              return (
                <li
                  key={item.id}
                  className={`flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 py-2 max-md:flex-nowrap${index >= narrowShown ? " max-md:hidden" : index === narrowShown - 1 ? " max-md:border-b-0" : ""}`}
                >
                  {/* 電話幅は 1 行に収め、件名は省略する（全文は title と受信箱）。 */}
                  <span
                    title={item.title}
                    className="min-w-0 flex-1 basis-0 font-medium max-md:truncate md:break-words"
                  >
                    {item.title}
                  </span>
                  {item.due_at ? (
                    <span className="text-label max-md:flex max-md:shrink-0 max-md:items-center max-md:gap-1 max-md:whitespace-nowrap">
                      <time dateTime={item.due_at} title={formatAbsolute(item.due_at)}>
                        {formatRelative(item.due_at)}
                      </time>
                      {soon ? (
                        <Badge tone="warning" className="max-md:whitespace-nowrap">
                          まもなく期限
                        </Badge>
                      ) : null}
                    </span>
                  ) : (
                    <span className="text-label text-muted-foreground max-md:shrink-0 max-md:whitespace-nowrap">
                      期限なし
                    </span>
                  )}
                  {item.project_id ? (
                    <span className="text-label text-muted-foreground max-md:hidden">
                      案件 {projectTitles.get(item.project_id) ?? shortId(item.project_id)}
                    </span>
                  ) : null}
                </li>
              );
            })}
          </ul>
        ) : inbox.data ? (
          <p className="mt-2 text-label text-muted-foreground max-md:mt-1">いま決めることはありません。</p>
        ) : null}
        <Link to="/inbox" className={`${entry} mt-1 md:hidden`}>
          {narrowRest > 0 ? <span className="tabular-nums">ほか {narrowRest} 件（受信箱へ）</span> : "受信箱へ"}
        </Link>
      </div>
      <ul className="flex flex-wrap gap-2 max-md:hidden">
        <li>{notifications("")}</li>
      </ul>
    </nav>
  );
}
