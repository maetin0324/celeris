import { Link } from "@tanstack/react-router";
import type { ComponentPropsWithoutRef } from "react";
import type { ArtifactView } from "../../api/generated/types";
import { ArtifactPreview } from "../../components/content/artifact-preview";
import { Badge } from "../../components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute } from "../../lib/time";
import { formatBytes } from "../files/task-files-query";

// 成果物の一覧（/artifacts と /tasks/:id?tab=artifacts で共有）。本文は ArtifactPreview（H8: Markdown 以外は download）。
// 360px でも読めるよう、大きさ・記録日時は md 以上でだけ列にし、狭い幅では成果物の欄の下に小さく出す。
// 長い path は折り返し、全文は title 属性に置く。
export type ArtifactTableRow = {
  taskId: string;
  /** 指定した行の一覧はタスクの列を持つ（/artifacts）。1 task の一覧では省く。 */
  taskTitle?: string;
  /** data-artifact の値。parity e2e が行を指す。 */
  marker: string;
  view: ArtifactView;
};

export function ArtifactTable({
  rows,
  label,
  ...props
}: { rows: readonly ArtifactTableRow[]; label: string } & Omit<ComponentPropsWithoutRef<"table">, "children">) {
  const showTask = rows.some((row) => row.taskTitle !== undefined);
  return (
    <Table aria-label={label} className="table-fixed" {...props}>
      <TableHeader>
        <TableRow>
          {showTask ? <TableHead className="w-1/3">タスク</TableHead> : null}
          <TableHead>成果物</TableHead>
          <TableHead className="hidden w-24 text-right md:table-cell">大きさ</TableHead>
          <TableHead className="hidden w-48 md:table-cell">記録</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <ArtifactRowView key={`${row.taskId}/${row.view.idx}`} row={row} showTask={showTask} />
        ))}
      </TableBody>
    </Table>
  );
}

function ArtifactRowView({ row, showTask }: { row: ArtifactTableRow; showTask: boolean }) {
  const { view, taskId, taskTitle } = row;
  const size = view.size != null ? formatBytes(view.size) : "—";
  const recorded = formatAbsolute(view.ts);
  const readable = view.exists && !view.forbidden;
  return (
    <TableRow data-artifact={row.marker}>
      {showTask ? (
        <TableCell className="min-w-0 break-words">
          <Link
            to="/tasks/$id"
            params={{ id: taskId }}
            title={taskId}
            className="inline-flex min-h-11 items-center text-foreground underline"
          >
            {taskTitle || taskId}
          </Link>
        </TableCell>
      ) : null}
      <TableCell className="min-w-0">
        <div className="flex min-w-0 flex-col gap-1">
          {readable ? (
            <ArtifactPreview taskId={taskId} idx={view.idx} name={view.artifact.name} />
          ) : (
            <p className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="min-w-0 break-all">{view.artifact.name}</span>
              <Badge tone={view.forbidden ? "danger" : "warning"}>
                {view.forbidden ? "読めない場所です" : "file がありません"}
              </Badge>
            </p>
          )}
          <p className="min-w-0 break-all font-mono text-label text-muted-foreground" title={view.artifact.path}>
            {view.artifact.path}
          </p>
          <p className="text-label text-muted-foreground tabular-nums md:hidden">
            {size}・{recorded}
          </p>
        </div>
      </TableCell>
      <TableCell
        className="hidden text-right text-muted-foreground tabular-nums md:table-cell"
        title={view.size != null ? `${view.size} B` : undefined}
      >
        {size}
      </TableCell>
      <TableCell className="hidden text-muted-foreground md:table-cell">
        <time dateTime={view.ts}>{recorded}</time>
      </TableCell>
    </TableRow>
  );
}
