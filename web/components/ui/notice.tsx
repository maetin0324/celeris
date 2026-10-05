import { cva } from "class-variance-authority";
import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../../lib/utils";
import { Icon } from "./icon";

// 画面の中に置く知らせの帯（shadcn の Alert に相当）。danger は今すぐ読ませる alert、warning は割り込まない status。
// 色の正本は styles.css の @theme の状態色の組で、色だけで意味を伝えないよう icon と見出しの文字を必ず出す。
const noticeVariants = cva("flex min-w-0 max-w-full flex-wrap items-start gap-3 rounded-md border p-3 text-body", {
  variants: {
    tone: {
      danger: "border-danger-foreground bg-danger text-danger-foreground",
      warning: "border-warning-foreground bg-warning text-warning-foreground",
    },
  },
  defaultVariants: { tone: "warning" },
});

export type NoticeTone = "danger" | "warning";

/** tone ごとの role。danger は assertive に読み上げる alert、warning は polite な status。 */
export const noticeRole: Record<NoticeTone, "alert" | "status"> = { danger: "alert", warning: "status" };

export type NoticeProps = Omit<HTMLAttributes<HTMLDivElement>, "title"> & {
  tone?: NoticeTone;
  /** 1 行の見出し（例「保存できませんでした」）。本文は children。 */
  title?: ReactNode;
  /** 任意の操作（Button や Link）。本文の後ろに置き、狭い幅では折り返して下に回る。 */
  action?: ReactNode;
};

export function Notice({ tone = "warning", title, action, className, children, ...props }: NoticeProps) {
  return (
    <div
      role={noticeRole[tone]}
      data-slot="notice"
      data-tone={tone}
      className={cn(noticeVariants({ tone }), className)}
      {...props}
    >
      <Icon name={tone === "danger" ? "alert" : "info"} className="mt-0.5 shrink-0" />
      <div className="min-w-0 flex-1 break-words">
        {title ? <p className="font-semibold">{title}</p> : null}
        {children ? <div className="text-foreground">{children}</div> : null}
      </div>
      {action ? <div className="flex shrink-0 flex-wrap gap-2">{action}</div> : null}
    </div>
  );
}
