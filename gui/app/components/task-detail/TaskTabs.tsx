import { Link } from "react-router";
import { TASK_TABS, type TaskTab, taskTabLabel } from "~/lib/labels";
import { cn } from "~/lib/utils";

/** タブの見出し（ADR-0044 D5）。`?tab=` だけを差し替え、他の検索パラメータ（`types` 等）は残す。 */
export function TaskTabs({
  current,
  searchParams,
  counts,
}: {
  current: TaskTab;
  searchParams: URLSearchParams;
  counts: { timeline: number; artifacts: number };
}) {
  return (
    // フェーズ 71（ADR-0055 D2 ラウンド 3）: タブは横スクロールするピル行（`overflow-x-auto`、折り返さない）。
    // 5 つのタブ名が並んでも 393px に収まらないことがあるため、切れた分はスクロールで見せる（D1-1/D1-6 の
    // 「overflow-x-auto の箱」と同じ扱い。`lg:` は元の折り返し行のまま）。
    // Phase 95（目視点検の所見、重さ「高」）: 5 つ目の「成果物」が右端で文字の途中で切れているのに、
    // 横にまだ続きがあると気付ける手がかりが無かった。右端に `pointer-events-none` のフェードを重ね、
    // タップ領域・DOM 構造・スクロール自体は変えずに「まだ右にある」ことだけを示す（`lg:hidden`。
    // デスクトップは折り返すのでフェード不要）。
    // フェードの右端をスクロール箱の実際の右端（`-mx-4` で画面端まで伸びた見た目上の境界）に合わせるため、
    // `relative` と `-mx-4`（bleed）は `nav` 側に置く（`ul` は `px-4` だけ残す）。`ul` にだけ `relative` を
    // 付けると、bleed していない `nav` の内側 16px 分だけフェードがずれて中途半端な位置に出てしまう。
    <nav aria-label="タスクの内訳" data-testid="task-tabs" className="relative -mx-4 lg:mx-0">
      <ul className="flex gap-1 overflow-x-auto border-b border-border px-4 lg:flex-wrap lg:overflow-visible lg:px-0">
        {TASK_TABS.map((t) => {
          const params = new URLSearchParams(searchParams);
          params.set("tab", t);
          const active = t === current;
          const count = t === "timeline" ? counts.timeline : t === "artifacts" ? counts.artifacts : null;
          return (
            <li key={t} className="shrink-0">
              <Link
                to={`?${params.toString()}`}
                replace
                data-testid={`task-tab-${t}`}
                data-active={active ? "true" : undefined}
                aria-current={active ? "page" : undefined}
                className={cn(
                  // ADR-0055 D1-2: タップ領域 44×44 以上。
                  // celeris ADR-0079 D14（Phase R4b）: 1 文字の「木」でも幅 44 を満たすよう min-w-11 と中央寄せ。
                  "-mb-px inline-flex min-h-11 min-w-11 items-center justify-center gap-1.5 rounded-t-lg border-b-2 px-3.5 py-2 text-sm font-medium no-underline transition-colors",
                  active
                    ? "border-primary text-primary"
                    : "border-transparent text-fg-muted hover:border-border-strong hover:text-fg",
                )}
              >
                {taskTabLabel(t)}
                {count !== null && count > 0 && (
                  // ADR-0055 D1-4: モバイルは text-sm、デスクトップは lg: で元の text-xs のまま。
                  <span className="text-sm tabular-nums text-fg-subtle lg:text-xs">{count}</span>
                )}
              </Link>
            </li>
          );
        })}
      </ul>
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-y-0 right-0 w-8 bg-gradient-to-l from-bg to-transparent lg:hidden"
      />
    </nav>
  );
}
