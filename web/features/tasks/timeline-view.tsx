import type { Timeline, TimelineItem } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { formatAbsolute } from "../../lib/time";

// /tasks/:id の timeline（P3-08、表示のみ）。`GET /tasks/:id/timeline` を時刻の順に 1 本で出す。
// 行は枠の中で折り返し、ページ全体の横溢れを出さない（S3）。

export function TimelineView({ timeline }: { timeline: Timeline }) {
  const items = timeline.items;
  if (items.length === 0) {
    return (
      <p className="text-label text-muted-foreground" role="status" data-testid="timeline-empty">
        まだ何も起きていません。
      </p>
    );
  }
  return (
    <ol className="flex flex-col gap-2" data-testid="timeline-list">
      {items.map((item, index) => (
        <TimelineRow key={itemKey(item, index)} item={item} />
      ))}
    </ol>
  );
}

function itemKey(item: TimelineItem, index: number): string {
  if (item.kind === "event") return `event:${item.seq}`;
  if (item.kind === "comment") return `comment:${item.comment.id}`;
  if (item.kind === "approval") return `approval:${item.approval.id}`;
  if (item.kind === "report") return `report:${item.report.id}`;
  if (item.kind === "release") return `release:${item.sha12}`;
  if (item.kind === "delegation") return `delegation:${item.run_id}:${index}`;
  if (item.kind === "integration") return `integration:${item.action}:${index}`;
  if (item.kind === "doc") return `doc:${item.path}`;
  if (item.kind === "knowledge") return `knowledge:${item.run_task_id}:${index}`;
  return `other:${index}`;
}

function TimelineRow({ item }: { item: TimelineItem }) {
  const text = itemText(item);
  return (
    <li className="min-w-0 rounded-md border border-border bg-surface p-3" data-testid={`timeline-${item.kind}`}>
      <p className="flex flex-wrap items-center gap-2 text-label text-muted-foreground">
        <time dateTime={item.at}>{formatAbsolute(item.at)}</time>
        <Badge tone="neutral">{item.kind}</Badge>
      </p>
      <p className="mt-1 whitespace-pre-wrap break-words text-label text-foreground">{text}</p>
    </li>
  );
}

function itemText(item: TimelineItem): string {
  switch (item.kind) {
    case "event":
      return `${item.event.type}${eventSuffix(item)}`;
    case "comment":
      return `${authorLabel(item.comment.author, item.comment.author_kind)}: ${item.comment.body}`;
    case "approval":
      return `認可: ${item.approval.question}${item.approval.decision ? ` → ${item.approval.decision}` : ""}`;
    case "report":
      return `報告: ${item.report.headline}`;
    case "delegation":
      return `委譲: ${item.tasks.map((task) => task.title).join("、")}`;
    case "release":
      return `リリース: ${item.sha12}`;
    case "integration":
      return `取り込み: ${item.action}${item.detail ? ` — ${item.detail}` : ""}`;
    case "doc":
      return `文書: ${item.title}（${item.path}）`;
    case "knowledge":
      return `知識の取り込み（${item.state}）`;
  }
}

function eventSuffix(item: Extract<TimelineItem, { kind: "event" }>): string {
  const event = item.event;
  switch (event.type) {
    case "transitioned":
      return `（${event.from} → ${event.to}）`;
    case "worker_started":
      return `（${event.adapter} / ${event.model}）`;
    case "worker_progress":
      return `（${event.msg}）`;
    case "worker_finished":
      return `（${event.outcome}）`;
    case "checkpoint_saved":
      return `（${event.run_id}）`;
    default:
      return "";
  }
}

function authorLabel(author: string | null | undefined, kind: string): string {
  if (author && kind !== "system") return `${author}（${kind}）`;
  return kind;
}
