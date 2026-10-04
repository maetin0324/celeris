import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { Report, ReportDetail, ReportList } from "../../api/generated/types";
import { reportKeys } from "../../api/queries/keys";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { formatAbsolute } from "../../lib/time";
import { Route } from "../../routes/reports";

// /reports は報告本文を読むだけの互換の画面（web ADR 2026-10-04 D4）。
// 既読・未読と新着の知らせは通知（/notifications の「報告」）で扱い、ここでは既読にしない。

export function reportsPath(filter: string, level: string): string {
  const query = new URLSearchParams();
  if (filter !== "all") query.set("unread", "true");
  if (level !== "") query.set("level", level);
  return `/api/reports?${query}`;
}

const control = "block min-h-11 max-w-full rounded-md border bg-background px-2 text-body text-foreground";

function ReportRow({ report, initiallyOpen }: { report: Report; initiallyOpen: boolean }) {
  const [expanded, setExpanded] = useState(initiallyOpen);
  const detail = useQuery({
    queryKey: ["reports", "detail", report.id],
    queryFn: ({ signal }) => apiGet<ReportDetail>(`/api/reports/${encodeURIComponent(report.id)}`, signal),
    enabled: expanded,
  });
  return (
    <li data-report-id={report.id} className="flex min-w-0 flex-col gap-2 border-b border-border py-3 last:border-b-0">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h2 className="break-words text-body font-semibold">{report.headline}</h2>
        <Badge tone="neutral">
          {report.kind} / 段 {report.level}
        </Badge>
        <time dateTime={report.created_at} className="text-label text-muted-foreground">
          {formatAbsolute(report.created_at)}
        </time>
      </div>
      <div>
        <Button size="sm" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          {expanded ? "閉じる" : "展開"}
        </Button>
      </div>
      {expanded && (
        <FetchFrame query={detail} subject="報告の本文">
          <div className="flex min-w-0 flex-col gap-3">
            <Markdown source={detail.data?.report.body ?? ""} />
            {detail.data?.sources_expanded.length ? (
              <section className="flex flex-col gap-2">
                <h3 className="font-medium">元の報告</h3>
                <ul className="flex flex-col gap-2">
                  {detail.data.sources_expanded.map((source) => (
                    <li key={source.id} className="rounded-md bg-muted p-2">
                      <strong>{source.headline}</strong>
                      <Markdown source={source.body ?? ""} />
                    </li>
                  ))}
                </ul>
              </section>
            ) : null}
          </div>
        </FetchFrame>
      )}
    </li>
  );
}

export function ReportsScreen() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  // 既読は通知で扱うので、ここは常に全件（filter=unread は互換で受けるだけ）。
  const level = search.level === undefined ? "" : String(search.level);
  const query = useQuery({
    queryKey: reportKeys.list({ filter: "all", level, project: search.project }),
    queryFn: ({ signal }) =>
      apiGet<ReportList>(
        reportsPath("all", level) + (search.project ? `&project=${encodeURIComponent(search.project)}` : ""),
        signal,
      ),
  });
  const items = query.data?.items ?? [];
  const ordered = search.report
    ? [...items.filter((item) => item.id === search.report), ...items.filter((item) => item.id !== search.report)]
    : items;
  return (
    <ScreenFrame
      title="報告"
      route="/reports"
      description="報告の本文を読む画面です。新しい報告の知らせと既読は通知で扱います。"
      actions={
        <Link to="/notifications" search={{ kind: "report" }} className="inline-flex min-h-11 items-center underline">
          通知で報告の知らせを見る
        </Link>
      }
    >
      <label className="flex w-fit min-w-0 flex-col gap-1 text-label">
        段
        <select
          className={control}
          value={level}
          onChange={(event) =>
            void navigate({
              to: "/reports",
              search: { ...search, level: event.target.value ? Number(event.target.value) : undefined },
            })
          }
        >
          <option value="">すべて</option>
          <option value="0">CoS</option>
          <option value="1">部</option>
          <option value="2">課</option>
        </select>
      </label>
      <FetchFrame
        query={query}
        subject="報告"
        empty={query.data !== undefined && items.length === 0}
        emptyMessage="報告はありません。"
      >
        <ul
          aria-label="報告の一覧"
          className="flex min-w-0 flex-col rounded-md border border-border bg-surface px-3 md:px-4"
        >
          {ordered.map((item) => (
            <ReportRow key={item.id} report={item} initiallyOpen={item.id === search.report} />
          ))}
        </ul>
      </FetchFrame>
    </ScreenFrame>
  );
}
