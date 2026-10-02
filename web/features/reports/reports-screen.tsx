import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { Report, ReportDetail, ReportList } from "../../api/generated/types";
import { reportKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { Route } from "../../routes/reports";
import { NotificationsEnable } from "./notifications-enable";

export function reportsPath(filter: string, level: string): string {
  const query = new URLSearchParams();
  if (filter !== "all") query.set("unread", "true");
  if (level !== "") query.set("level", level);
  return `/api/reports?${query}`;
}

function ReportRow({ report }: { report: Report }) {
  const [expanded, setExpanded] = useState(false);
  const detail = useQuery({
    queryKey: ["reports", "detail", report.id],
    queryFn: ({ signal }) => apiGet<ReportDetail>(`/api/reports/${encodeURIComponent(report.id)}`, signal),
    enabled: expanded,
  });
  const sender = useActionResult(reportKeys.all);
  return (
    <li data-report-id={report.id} className="min-w-0 rounded border p-3 space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <strong className="break-words">{report.headline}</strong>
        <span className="text-sm">
          {report.kind} / level {report.level}
        </span>
        <span>{report.read_at ? "既読" : "未読"}</span>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
          {expanded ? "閉じる" : "展開"}
        </Button>
        {!report.read_at && (
          <Button
            disabled={sender.pending}
            onClick={() => void sender.run([{ id: report.id, path: "/api/reports/read", body: { ids: [report.id] } }])}
          >
            既読にする
          </Button>
        )}
      </div>
      <ActionResultView result={sender.results[report.id]} />
      {expanded && (
        <FetchFrame query={detail}>
          <div className="space-y-3">
            <Markdown source={detail.data?.report.body ?? ""} />
            {detail.data?.sources_expanded.length ? (
              <section>
                <h3 className="font-medium">元の報告</h3>
                <ul className="space-y-2">
                  {detail.data.sources_expanded.map((source) => (
                    <li key={source.id} className="rounded bg-neutral-50 p-2">
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
  const filter = search.filter === "all" ? "all" : "unread";
  const level = search.level === undefined ? "0" : String(search.level);
  const query = useQuery({
    queryKey: reportKeys.list({ filter, level, project: search.project }),
    queryFn: ({ signal }) =>
      apiGet<ReportList>(
        reportsPath(filter, level) + (search.project ? `&project=${encodeURIComponent(search.project)}` : ""),
        signal,
      ),
  });
  const sender = useActionResult(reportKeys.all);
  const unreadIds = query.data?.items.filter((item) => !item.read_at).map((item) => item.id) ?? [];
  return (
    <ScreenFrame title="報告" route="/reports">
      <div className="flex flex-wrap items-center gap-3">
        <NotificationsEnable />
        <span className="text-sm text-neutral-600">報告のブラウザ通知</span>
      </div>
      <div className="flex flex-wrap items-end gap-3">
        <label>
          表示
          <select
            className="block min-h-11 rounded border px-2"
            value={filter}
            onChange={(event) => void navigate({ to: "/reports", search: { ...search, filter: event.target.value } })}
          >
            <option value="unread">未読だけ</option>
            <option value="all">全部</option>
          </select>
        </label>
        <label>
          段
          <select
            className="block min-h-11 rounded border px-2"
            value={level}
            onChange={(event) =>
              void navigate({
                to: "/reports",
                search: { ...search, level: event.target.value ? Number(event.target.value) : undefined },
              })
            }
          >
            <option value="0">CoS</option>
            <option value="1">部</option>
            <option value="2">課</option>
            <option value="">すべて</option>
          </select>
        </label>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={sender.pending || unreadIds.length === 0}
          onClick={() => void sender.run([{ id: "all", path: "/api/reports/read", body: { ids: unreadIds } }])}
        >
          表示中をすべて既読にする
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() => void sender.run([{ id: "notify-test", path: "/api/notify/test" }])}
        >
          通知を試す
        </Button>
      </div>
      <ActionResultView result={sender.results.all} />
      <ActionResultView result={sender.results["notify-test"]} />
      <FetchFrame query={query}>
        {query.data?.items.length ? (
          <ul className="space-y-2">
            {query.data.items.map((item) => (
              <ReportRow key={item.id} report={item} />
            ))}
          </ul>
        ) : (
          <p>報告はありません。</p>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
