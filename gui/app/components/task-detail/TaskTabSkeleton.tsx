import { Skeleton } from "~/components/ui/skeleton";

/**
 * タブの中身の読み込み中プレースホルダ（Phase 77、ADR-0055 D3「体感速度」）。チャンク待ち（`Suspense`）と
 * タブ切り替えのナビゲーション待ち（`useNavigation`）の両方で使う。高さは実際のタブの中身（見出し 1 行 +
 * カード数枚）に近い概算。
 */
export function TaskTabSkeleton() {
  return (
    <div className="space-y-4" aria-hidden="true" data-testid="task-tab-skeleton">
      <Skeleton className="h-5 w-24" />
      <div className="space-y-3 rounded-xl border border-border bg-surface p-4">
        <Skeleton className="h-4 w-1/3" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-4 w-2/3" />
      </div>
    </div>
  );
}
