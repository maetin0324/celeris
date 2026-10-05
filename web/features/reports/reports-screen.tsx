import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useId, useState } from "react";
import { apiGet } from "../../api/client";
import type { OrgList, Report, ReportDetail, ReportKind, ReportList } from "../../api/generated/types";
import { orgKeys, reportKeys } from "../../api/queries/keys";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Select } from "../../components/ui/select";
import { ShortId } from "../../components/ui/short-id";
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

// 報告の種類は和名で出し、悪い知らせ（danger）と質問（warning）を一覧の中で目立たせる。
export const reportKindView: Record<ReportKind, { label: string; tone: BadgeTone }> = {
  bad_news: { label: "悪い知らせ", tone: "danger" },
  question: { label: "質問", tone: "warning" },
  proposal: { label: "提案", tone: "info" },
  result: { label: "結果", tone: "neutral" },
  progress: { label: "進捗", tone: "neutral" },
};

// 段（level）は組織の階層。フィルタと行の表示で同じ語を使う。
export const levelLabels: Record<number, string> = { 0: "CoS", 1: "部", 2: "課" };

export function levelLabel(level: number): string {
  return levelLabels[level] ?? `段 ${level}`;
}

const linkClass =
  "inline-flex min-h-11 items-center underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

function ReportRow({
  report,
  initiallyOpen,
  names,
}: {
  report: Report;
  initiallyOpen: boolean;
  names: Map<string, string>;
}) {
  const [expanded, setExpanded] = useState(initiallyOpen);
  const bodyId = useId();
  const detail = useQuery({
    queryKey: ["reports", "detail", report.id],
    queryFn: ({ signal }) => apiGet<ReportDetail>(`/api/reports/${encodeURIComponent(report.id)}`, signal),
    enabled: expanded,
  });
  const kind = reportKindView[report.kind] ?? { label: report.kind, tone: "neutral" as const };
  const sender = names.get(report.node_id);
  const needsAnswer = report.kind === "question" || report.kind === "proposal";
  return (
    <li data-report-id={report.id} className="flex min-w-0 flex-col gap-2 border-b border-border py-3 last:border-b-0">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h2 className="break-words text-body font-semibold">{report.headline}</h2>
        <Badge tone={kind.tone}>{kind.label}</Badge>
        <time dateTime={report.created_at} className="text-label text-muted-foreground">
          {formatAbsolute(report.created_at)}
        </time>
      </div>
      <p className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-label text-muted-foreground">
        <span>
          送り手: {sender ?? <ShortId value={report.node_id} label="送り手の ID" length={16} copyable={false} />}（
          {levelLabel(report.level)}）
        </span>
        {report.task_id ? (
          <Link to="/tasks/$id" params={{ id: report.task_id }} className={linkClass}>
            元のタスクを開く
          </Link>
        ) : null}
        {report.project_id ? (
          <Link to="/projects/$id" params={{ id: report.project_id }} className={linkClass}>
            案件を開く
          </Link>
        ) : null}
        {needsAnswer ? (
          <Link to="/inbox" className={linkClass}>
            受信箱で答える
          </Link>
        ) : null}
      </p>
      <div>
        <Button
          size="sm"
          aria-expanded={expanded}
          aria-controls={bodyId}
          aria-label={`「${report.headline}」を${expanded ? "閉じる" : "展開"}`}
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? "閉じる" : "展開"}
        </Button>
      </div>
      <div id={bodyId} hidden={!expanded} className="min-w-0">
        {expanded && (
          <FetchFrame query={detail} subject="報告の本文">
            <div className="flex min-w-0 max-w-prose-ja flex-col gap-3">
              <Markdown source={detail.data?.report.body ?? ""} />
              {detail.data?.sources_expanded.length ? (
                <section className="flex flex-col gap-2">
                  <h3 className="font-medium">元の報告</h3>
                  <ul className="flex flex-col gap-2">
                    {detail.data.sources_expanded.map((source) => (
                      <li key={source.id} className="min-w-0 rounded-md bg-muted p-2">
                        <strong className="break-words">{source.headline}</strong>
                        <p className="text-label text-muted-foreground">
                          {reportKindView[source.kind]?.label ?? source.kind}・
                          {names.get(source.node_id) ?? source.node_id}・{formatAbsolute(source.created_at)}
                        </p>
                        <Markdown source={source.body ?? ""} />
                      </li>
                    ))}
                  </ul>
                </section>
              ) : null}
            </div>
          </FetchFrame>
        )}
      </div>
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
  const org = useQuery({ queryKey: orgKeys.list(), queryFn: ({ signal }) => apiGet<OrgList>("/api/org", signal) });
  const names = new Map((org.data?.items ?? []).map((node) => [node.id, node.name]));
  const levelId = useId();
  const items = query.data?.items ?? [];
  const ordered = search.report
    ? [...items.filter((item) => item.id === search.report), ...items.filter((item) => item.id !== search.report)]
    : items;
  return (
    <ScreenFrame
      title="報告"
      route="/reports"
      description="下から上がった報告の本文を開いて読みます。新着の知らせと既読は通知で扱います。"
    >
      <div className="flex min-w-0 flex-wrap items-end gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-col gap-1">
          <label htmlFor={levelId} className="text-label font-medium">
            報告元の段
          </label>
          <Select
            id={levelId}
            className="w-fit"
            value={level}
            onChange={(event) =>
              void navigate({
                to: "/reports",
                search: { ...search, level: event.target.value ? Number(event.target.value) : undefined },
              })
            }
          >
            <option value="">すべて</option>
            {Object.entries(levelLabels).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </Select>
        </div>
        <Link to="/notifications" search={{ kind: "report" }} className={linkClass}>
          通知で報告の知らせを見る
        </Link>
      </div>
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
            <ReportRow key={item.id} report={item} initiallyOpen={item.id === search.report} names={names} />
          ))}
        </ul>
      </FetchFrame>
    </ScreenFrame>
  );
}
