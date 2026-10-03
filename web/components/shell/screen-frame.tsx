import { Link, type LinkOptions } from "@tanstack/react-router";
import { Fragment, type ReactNode } from "react";
import { Panel } from "../ui/panel";

/** パンくずの 1 段。`link` が無い段（通常は現在の画面）は文字だけにする。 */
export type BreadcrumbItem = { label: string; link?: LinkOptions };

export type ScreenFrameProps = {
  title: string;
  route: string;
  children?: ReactNode;
  /** 上位の画面への道筋。最後の段が現在の画面。 */
  breadcrumb?: readonly BreadcrumbItem[];
  /** 見出しの下の 1〜2 文の補足。 */
  description?: ReactNode;
  /** 画面の主操作の slot。狭い幅では見出しの下へ折り返す。 */
  actions?: ReactNode;
};

// 画面の見出しと枠（P2-02）。h1 は tabIndex=-1 で、遷移後の focus 先になる（S4）。
// page header は パンくず → 見出し・説明 → 主操作 の順で、DOM 順と見た目の順を揃える（DESIGN.md「原則」）。
export function ScreenFrame({ title, route, children, breadcrumb, description, actions }: ScreenFrameProps) {
  return (
    <div className="flex min-w-0 flex-col gap-4" data-screen={route}>
      <div data-slot="page-header" className="flex min-w-0 flex-col gap-1">
        {breadcrumb && breadcrumb.length > 0 ? <Breadcrumb items={breadcrumb} /> : null}
        <div className="flex min-w-0 flex-col gap-3 md:flex-row md:items-start md:justify-between">
          <div className="min-w-0">
            <h1
              tabIndex={-1}
              className="break-words text-title font-semibold text-foreground focus:outline-none md:text-title-wide"
            >
              {title}
            </h1>
            {description ? (
              <p data-slot="page-description" className="mt-1 break-words text-body text-muted-foreground">
                {description}
              </p>
            ) : null}
          </div>
          {actions ? (
            <div data-slot="page-actions" className="flex min-w-0 shrink-0 flex-wrap items-center gap-2">
              {actions}
            </div>
          ) : null}
        </div>
      </div>
      {children ?? <Panel title="準備中">この画面の中身は後の Phase で入ります。</Panel>}
    </div>
  );
}

function Breadcrumb({ items }: { items: readonly BreadcrumbItem[] }) {
  const last = items.length - 1;
  return (
    <nav aria-label="パンくず" className="min-w-0">
      <ol className="flex min-w-0 flex-wrap items-center gap-x-1 text-label text-muted-foreground">
        {items.map((item, index) => (
          <Fragment key={`${index}-${item.label}`}>
            <li className="min-w-0 break-words">
              {item.link && index !== last ? (
                <Link
                  {...item.link}
                  className="inline-flex min-h-11 items-center rounded-sm text-primary underline-offset-4 hover:underline"
                >
                  {item.label}
                </Link>
              ) : (
                <span aria-current={index === last ? "page" : undefined} className="inline-flex min-h-11 items-center">
                  {item.label}
                </span>
              )}
            </li>
            {index !== last ? (
              <li aria-hidden="true" className="select-none">
                /
              </li>
            ) : null}
          </Fragment>
        ))}
      </ol>
    </nav>
  );
}
