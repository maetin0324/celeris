import type { ComponentPropsWithoutRef } from "react";
import { cn } from "../../lib/utils";

export type TableProps = ComponentPropsWithoutRef<"table"> & {
  /** 横スクロールの枠の名前。枠に focus でき、キーボードで横にスクロールできる。 */
  "aria-label"?: string;
  /** 横スクロールの枠の tabIndex。名前がある時の既定は 0（名前の無い枠は focus 順に入れない）。 */
  tabIndex?: number;
  wrapperClassName?: string;
};

// shadcn の Table（new-york）を Celeris token と高密度の行で上書きしたもの。
// 横に溢れる分は名前のある枠の中だけでスクロールし、ページ全体を溢れさせない（DESIGN.md「Table と list」）。
export function Table({ className, wrapperClassName, "aria-label": ariaLabel, tabIndex, ...props }: TableProps) {
  const table = (
    <table data-slot="table" className={cn("w-full caption-bottom text-label text-foreground", className)} {...props} />
  );
  const wrapper = cn("relative w-full min-w-0 overflow-x-auto", wrapperClassName);
  // 名前の無い枠は landmark にせず、focus 順にも入れない。
  if (!ariaLabel) {
    return (
      <div data-slot="table-container" tabIndex={tabIndex} className={wrapper}>
        {table}
      </div>
    );
  }
  return (
    // 名前のある section は region landmark になる。focus して矢印キーで横スクロールできる。
    <section data-slot="table-container" aria-label={ariaLabel} tabIndex={tabIndex ?? 0} className={wrapper}>
      {table}
    </section>
  );
}

export function TableHeader({ className, ...props }: ComponentPropsWithoutRef<"thead">) {
  return <thead data-slot="table-header" className={cn("border-b border-border", className)} {...props} />;
}

export function TableBody({ className, ...props }: ComponentPropsWithoutRef<"tbody">) {
  return <tbody data-slot="table-body" className={cn("divide-y divide-border", className)} {...props} />;
}

export function TableFooter({ className, ...props }: ComponentPropsWithoutRef<"tfoot">) {
  return (
    <tfoot
      data-slot="table-footer"
      className={cn("border-t border-border bg-muted font-medium", className)}
      {...props}
    />
  );
}

export function TableRow({ className, ...props }: ComponentPropsWithoutRef<"tr">) {
  return (
    <tr
      data-slot="table-row"
      className={cn("transition-colors hover:bg-accent state-checked:bg-accent", className)}
      {...props}
    />
  );
}

export function TableHead({ className, scope = "col", ...props }: ComponentPropsWithoutRef<"th">) {
  return (
    <th
      data-slot="table-head"
      scope={scope}
      className={cn("px-2 py-2 text-left align-bottom font-medium whitespace-nowrap text-muted-foreground", className)}
      {...props}
    />
  );
}

export function TableCell({ className, ...props }: ComponentPropsWithoutRef<"td">) {
  return <td data-slot="table-cell" className={cn("px-2 py-2 align-top", className)} {...props} />;
}

export function TableCaption({ className, ...props }: ComponentPropsWithoutRef<"caption">) {
  return (
    <caption
      data-slot="table-caption"
      className={cn("mt-2 text-left text-label text-muted-foreground", className)}
      {...props}
    />
  );
}
