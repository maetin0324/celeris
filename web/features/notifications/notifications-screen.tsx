import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useRouter } from "@tanstack/react-router";
import type { MouseEvent } from "react";
import { apiGet } from "../../api/client";
import type { Notice, ProjectList } from "../../api/generated/types";
import {
  markAllNotificationsRead,
  markNotificationRead,
  type NotificationsFilters,
  notificationReadInvalidates,
  notificationsQuery,
} from "../../api/queries/inbox-notifications";
import { projectKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { shortId } from "../../components/ui/short-id";
import { formatAbsolute, formatRelative } from "../../lib/time";
import { Route } from "../../routes/notifications";
import { NotificationsEnable } from "../reports/notifications-enable";
import { noticeKinds, noticeKindView, noticeLinks } from "./notice-view";

// /notifications（ADR-0133 D5・web ADR 2026-10-04 D4）。判断の要らない知らせを束で読み、既読にする。
// 判断が要るものは受信箱（/inbox）。絞り込みと頁は URL の search param に持つ。

const PAGE_SIZE = 50;
const control =
  "block min-h-11 max-w-full rounded-md border border-input bg-background px-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

const projectsQuery = {
  queryKey: projectKeys.list(),
  queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<ProjectList>("/api/projects", signal),
} as const;

function AppLink({ href, children, onOpen }: { href: string; children: string; onOpen?: () => void }) {
  const router = useRouter();
  const open = (event: MouseEvent<HTMLAnchorElement>) => {
    onOpen?.();
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
    event.preventDefault();
    router.history.push(href);
  };
  return (
    <a href={href} onClick={open} className="inline-flex min-h-11 items-center underline">
      {children}
    </a>
  );
}

function NoticeRow({ notice, projectTitle }: { notice: Notice; projectTitle?: string }) {
  const queryClient = useQueryClient();
  const reader = useActionResult(notificationReadInvalidates[0]);
  const kind = noticeKindView(notice.kind);
  const unread = !notice.read_at;
  const links = noticeLinks(notice);
  const markRead = () => {
    if (!unread) return;
    void markNotificationRead(notice.id).then(
      () => queryClient.invalidateQueries({ queryKey: notificationReadInvalidates[0] }),
      () => undefined,
    );
  };
  const titleId = `notice-${notice.id}-title`;
  return (
    <li
      data-notice-id={notice.id}
      data-unread={unread ? "true" : "false"}
      aria-labelledby={titleId}
      className="flex min-w-0 flex-col gap-2 border-b border-border py-3 last:border-b-0 md:flex-row md:items-start md:gap-4"
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          {unread ? <Badge tone="warning">未読</Badge> : <span className="text-label text-muted-foreground">既読</span>}
          <Badge tone={kind.tone}>{kind.label}</Badge>
          {notice.count > 1 ? <span className="text-label text-muted-foreground">{notice.count} 件の束</span> : null}
          <time
            dateTime={notice.last_at}
            title={formatAbsolute(notice.last_at)}
            className="text-label text-muted-foreground"
          >
            最終 {formatRelative(notice.last_at)}
          </time>
        </div>
        <h2 id={titleId} className={unread ? "break-words text-body font-semibold" : "break-words text-body"}>
          {notice.title}
        </h2>
        {notice.summary ? <p className="break-words text-body text-muted-foreground">{notice.summary}</p> : null}
        {links.length > 0 || notice.project_id ? (
          <div className="flex min-w-0 flex-wrap items-center gap-x-4">
            {links.map((link) => (
              <AppLink key={link.href} href={link.href} onOpen={markRead}>
                {link.label}
              </AppLink>
            ))}
            {notice.project_id ? (
              <span className="break-words text-label text-muted-foreground">
                案件 {projectTitle ?? shortId(notice.project_id)}
              </span>
            ) : null}
          </div>
        ) : null}
        <ActionResultView result={reader.results[notice.id]} />
      </div>
      {unread ? (
        <Button
          size="sm"
          variant="secondary"
          className="self-start"
          aria-label={`「${notice.title}」を既読にする`}
          disabled={reader.pending}
          onClick={() =>
            void reader.run([{ id: notice.id, path: `/api/notifications/${encodeURIComponent(notice.id)}/read` }])
          }
        >
          既読にする
        </Button>
      ) : null}
    </li>
  );
}

export function NotificationsScreen() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const filters: NotificationsFilters = {
    unread: search.unread,
    kind: search.kind,
    project: search.project,
    before: search.before,
    limit: PAGE_SIZE,
  };
  const query = useQuery(notificationsQuery(filters));
  const projects = useQuery(projectsQuery);
  const tester = useActionResult(["notify"]);
  const setSearch = (next: Partial<typeof search>) =>
    void navigate({ to: "/notifications", search: { ...search, before: undefined, ...next } });
  const unreadTotal = query.data?.unread;
  const scope = [
    search.kind ? `種類「${noticeKindView(search.kind).label}」` : null,
    search.project ? `案件 ${search.project}` : null,
  ]
    .filter(Boolean)
    .join("・");

  return (
    <ScreenFrame
      title="通知"
      route="/notifications"
      description="判断の要らない知らせを束ねて並べます。判断が要るものは受信箱にあります。"
      actions={
        <>
          <Link className="inline-flex min-h-11 items-center underline" to="/inbox">
            受信箱を開く
          </Link>
          <ConfirmDialog
            trigger={
              <Button variant="secondary" disabled={!unreadTotal}>
                すべて既読にする
              </Button>
            }
            title="通知をまとめて既読にする"
            target={scope ? `${scope}の未読の通知` : "すべての未読の通知"}
            consequence="未読の束が既読になり、nav の未読数から外れます。通知そのものは消えません。"
            reversibility="未読には戻せません。既読の束は「全部」の表示で読み返せます。"
            followUp="この一覧の「全部」の表示と nav の未読数で確かめられます。"
            confirmLabel="既読にする"
            onConfirm={async () => {
              await markAllNotificationsRead({ kind: search.kind ?? null, project: search.project ?? null });
              await queryClient.invalidateQueries({ queryKey: notificationReadInvalidates[0] });
            }}
          />
        </>
      }
    >
      <form
        aria-label="通知の絞り込み"
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => event.preventDefault()}
      >
        <label className="flex min-w-0 flex-col gap-1 text-label">
          表示
          <select
            className={control}
            value={search.unread ? "unread" : "all"}
            onChange={(event) => setSearch({ unread: event.target.value === "unread" ? true : undefined })}
          >
            <option value="all">全部</option>
            <option value="unread">未読だけ</option>
          </select>
        </label>
        <label className="flex min-w-0 flex-col gap-1 text-label">
          種類
          <select
            className={control}
            value={search.kind ?? ""}
            onChange={(event) => setSearch({ kind: noticeKinds.find((kind) => kind.key === event.target.value)?.key })}
          >
            <option value="">すべての種類</option>
            {noticeKinds.map((kind) => (
              <option key={kind.key} value={kind.key}>
                {kind.label}
              </option>
            ))}
          </select>
        </label>
        <label className="flex min-w-0 flex-col gap-1 text-label">
          案件
          <select
            className={control}
            value={search.project ?? ""}
            onChange={(event) => setSearch({ project: event.target.value || undefined })}
          >
            <option value="">すべての案件</option>
            {search.project && !projects.data?.items.some((project) => project.id === search.project) ? (
              <option value={search.project}>{search.project}</option>
            ) : null}
            {projects.data?.items.map((project) => (
              <option key={project.id} value={project.id}>
                {project.title}
              </option>
            ))}
          </select>
        </label>
        <p role="status" className="min-h-11 content-center text-label text-muted-foreground">
          {unreadTotal === undefined ? "" : unreadTotal > 0 ? `未読 ${unreadTotal} 件` : "未読はありません"}
        </p>
      </form>
      <FetchFrame
        query={query}
        subject="通知"
        empty={query.data !== undefined && query.data.items.length === 0}
        emptyMessage={search.unread ? "未読の通知はありません。" : "通知はありません。"}
      >
        <ul
          aria-label="通知の一覧"
          className="flex min-w-0 flex-col rounded-md border border-border bg-surface px-3 md:px-4"
        >
          {query.data?.items.map((notice) => (
            <NoticeRow
              key={notice.id}
              notice={notice}
              projectTitle={projects.data?.items.find((project) => project.id === notice.project_id)?.title}
            />
          ))}
        </ul>
      </FetchFrame>
      <nav aria-label="通知の頁" className="flex flex-wrap gap-2">
        {search.before ? (
          <Button variant="secondary" onClick={() => setSearch({ before: undefined })}>
            最新に戻る
          </Button>
        ) : null}
        {query.data?.next_before ? (
          <Button variant="secondary" onClick={() => setSearch({ before: query.data?.next_before ?? undefined })}>
            古い通知を見る
          </Button>
        ) : null}
      </nav>
      <section aria-labelledby="notify-settings" className="flex flex-col gap-2 border-t border-border pt-4">
        <h2 id="notify-settings" className="text-section font-semibold">
          通知の届け方
        </h2>
        <div className="flex flex-wrap items-center gap-3 text-body">
          <NotificationsEnable />
          <Button
            variant="secondary"
            disabled={tester.pending}
            onClick={() => void tester.run([{ id: "notify-test", path: "/api/notify/test" }])}
          >
            通知を試す
          </Button>
        </div>
        <ActionResultView result={tester.results["notify-test"]} />
      </section>
    </ScreenFrame>
  );
}
