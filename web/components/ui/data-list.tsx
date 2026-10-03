import type { ComponentPropsWithoutRef, ReactNode } from "react";
import { cn } from "../../lib/utils";

export type DataListItem = {
  /** React の key。省略時は label が文字列ならそれを使う。 */
  key?: string;
  label: ReactNode;
  value: ReactNode;
};

export type DataListProps = ComponentPropsWithoutRef<"dl"> & {
  /** 指定すると行を組み立てる。省略時は children（DataListRow・DataListTerm・DataListValue）をそのまま置く。 */
  items?: readonly DataListItem[];
};

// key/value の一覧。狭い幅では項目名の下に値を縦に積み、広い幅（md 以上）では項目名と値の 2 列にする。
// 行は HTML が dl の子に許す div で包み、見た目と DOM の順序を揃える。
export function DataList({ items, className, children, ...props }: DataListProps) {
  return (
    <dl className={cn("min-w-0 divide-y divide-border text-label", className)} {...props}>
      {items
        ? items.map((item, i) => (
            <DataListRow key={item.key ?? (typeof item.label === "string" ? item.label : i)}>
              <DataListTerm>{item.label}</DataListTerm>
              <DataListValue>{item.value}</DataListValue>
            </DataListRow>
          ))
        : children}
    </dl>
  );
}

export function DataListRow({ className, ...props }: ComponentPropsWithoutRef<"div">) {
  return (
    <div
      data-slot="data-list-row"
      className={cn("flex min-w-0 flex-col gap-1 py-2 md:flex-row md:gap-4", className)}
      {...props}
    />
  );
}

export function DataListTerm({ className, ...props }: ComponentPropsWithoutRef<"dt">) {
  return <dt className={cn("font-medium text-muted-foreground md:w-1/3 md:shrink-0", className)} {...props} />;
}

export function DataListValue({ className, ...props }: ComponentPropsWithoutRef<"dd">) {
  return <dd className={cn("min-w-0 break-words text-foreground", className)} {...props} />;
}
