import type { ReactNode } from "react";

export type MetaItem = { label: string; value: ReactNode };

// 出典・scope・更新日などの補助情報を、縦に積まずに「項目名 値」を横に並べた小さな行で出す（折り返しは許す）。
export function MetaLine({ items }: { items: readonly MetaItem[] }) {
  return (
    <dl className="flex min-w-0 flex-wrap gap-x-4 gap-y-1 text-label">
      {items.map((item) => (
        <div key={item.label} className="flex min-w-0 gap-1">
          <dt className="shrink-0 text-muted-foreground">{item.label}</dt>
          <dd className="min-w-0 break-all text-foreground">{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}
