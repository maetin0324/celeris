import type { ActualWriteSetView, BehindTarget, BehindTargetRepo, TaskDetail } from "~/celeris/types";
import { Badge } from "~/components/ui/badge";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { EmptyState, Mono } from "~/components/ui/misc";
import { formatDuration } from "~/lib/time-delta";
import { cn } from "~/lib/utils";

/** docs/adr/0130 D4: 遅れを「N commits / 経過時間」で読めるようにする。欠落は「計測不可」。 */
function behindLabel(commits: number | null | undefined, ageSeconds: number | null | undefined): string {
  if (commits == null) return "target から遅れ: 計測不可";
  const age = ageSeconds != null ? `${formatDuration(ageSeconds)}経過` : "経過時間: 計測不可";
  return `target から遅れ: ${commits} commits / ${age}`;
}

/** 0 commits は既定色、計測不可は中立、正の値は stale を強調する警告色（ADR-0130 D4/D5）。 */
function behindTone(commits: number | null | undefined): "neutral" | "success" | "warning" {
  if (commits == null) return "neutral";
  return commits > 0 ? "warning" : "success";
}

function BehindRow({ label, value }: { label: string; value: BehindTarget | BehindTargetRepo }) {
  const commits = value.behind_target_commits;
  return (
    <li className="flex flex-wrap items-center gap-2 rounded-lg border border-border p-2.5 text-sm">
      <span className="font-medium text-fg">{label}</span>
      <Badge tone={behindTone(commits)} data-testid="behind-target-badge">
        {behindLabel(commits, value.behind_target_age_seconds)}
      </Badge>
      {value.behind_target_observed_at && (
        <span className="text-fg-subtle">（観測: {value.behind_target_observed_at}）</span>
      )}
    </li>
  );
}

function WriteSetList({ title, items }: { title: string; items: ActualWriteSetView[] }) {
  return (
    <div data-testid="actual-write-set-group">
      <p className="text-sm font-semibold uppercase tracking-wide text-fg-subtle lg:text-xs">{title}</p>
      {items.length === 0 ? (
        <p className="mt-1 text-sm text-fg-subtle">ありません。</p>
      ) : (
        <ul className="mt-1.5 space-y-1.5">
          {items.map((item) => (
            <li
              key={`${item.repo_id}-${item.owner_id}-${item.recorded_at}`}
              data-testid="actual-write-set-item"
              className="rounded-lg border border-border p-2.5 text-sm"
            >
              <p className="flex flex-wrap items-center gap-1.5">
                <Mono>{item.repo_id}</Mono>
                <Badge tone={item.status === "unavailable" ? "warning" : "neutral"}>{item.status}</Badge>
                {item.base_sha && item.head_sha && (
                  <Mono>
                    {item.base_sha.slice(0, 8)}..{item.head_sha.slice(0, 8)}
                  </Mono>
                )}
                <span className="text-fg-subtle">{item.recorded_at}</span>
              </p>
              {item.reason && <p className="mt-1 text-fg-muted">{item.reason}</p>}
              {item.paths.length > 0 && (
                <ul className="mt-1.5 flex flex-wrap gap-1.5">
                  {item.paths.map((path) => (
                    <li key={path}>
                      <Mono className={cn("rounded border border-border bg-surface-2/40 px-1.5 py-0.5")}>{path}</Mono>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * タスク詳細の write-set / behind 節（docs/adr/0130 D1/D2/D4）。
 * expected_write_paths（粗い hint）・actual_run_write_sets/actual_work_unit_write_sets（Git diff から
 * の確定実績）・behind_target（target からの遅れと計測時刻）を表示する。GUI は celeris の判定を
 * 再実装せず、API が返した値をそのまま出す。
 */
export function WriteSetSection({ detail }: { detail: TaskDetail }) {
  const expected = detail.expected_write_paths ?? [];
  const behind = detail.behind_target;
  const repos = behind?.repos ?? [];

  return (
    <section aria-labelledby="write-set-heading" data-testid="write-set-section">
      <Card>
        <CardHeader
          icon="target"
          tone="info"
          title={
            <h2 id="write-set-heading" className="text-[0.95rem] font-semibold text-fg">
              write-set と behind
            </h2>
          }
        />
        <CardBody className="space-y-5">
          <div data-testid="expected-write-paths">
            <p className="text-sm font-semibold uppercase tracking-wide text-fg-subtle lg:text-xs">
              expected_write_paths
            </p>
            {expected.length === 0 ? (
              <p className="mt-1 text-sm text-fg-subtle">指定なし。</p>
            ) : (
              <ul className="mt-1.5 flex flex-wrap gap-1.5">
                {expected.map((path) => (
                  <li key={path}>
                    <Mono className="rounded border border-border bg-surface-2/40 px-1.5 py-0.5">{path}</Mono>
                  </li>
                ))}
              </ul>
            )}
          </div>

          <div data-testid="behind-target">
            <p className="text-sm font-semibold uppercase tracking-wide text-fg-subtle lg:text-xs">target からの遅れ</p>
            {!behind || (behind.behind_target_commits == null && repos.length === 0) ? (
              <EmptyState icon="target" title="計測されていません。" compact />
            ) : (
              <ul className="mt-1.5 space-y-1.5">
                <BehindRow label="代表値" value={behind} />
                {repos.map((repo) => (
                  <BehindRow key={repo.repo_id} label={repo.repo_id} value={repo} />
                ))}
              </ul>
            )}
          </div>

          <WriteSetList title="actual_run_write_sets" items={detail.actual_run_write_sets} />
          <WriteSetList title="actual_work_unit_write_sets" items={detail.actual_work_unit_write_sets} />
        </CardBody>
      </Card>
    </section>
  );
}
