import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";
import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

// shadcn（new-york）の Badge を写し、Celeris token で上書きした読み取り専用の部品（DESIGN.md §Badge）。
// 14px・weight 500・左右 8px 上下 4px・角丸 4px・影なし。pill button に見せないので rounded-full にしない。
// 色の正本は styles.css の @theme の状態色の組（背景 / 前景）で、ここでは名前だけを参照する。
export const badgeVariants = cva(
  "inline-flex w-fit shrink-0 items-center gap-1 whitespace-nowrap rounded-sm px-2 py-1 font-medium",
  {
    variants: {
      tone: {
        success: "bg-success text-success-foreground",
        warning: "bg-warning text-warning-foreground",
        danger: "bg-danger text-danger-foreground",
        info: "bg-info text-info-foreground",
        neutral: "bg-neutral text-neutral-foreground",
        running: "bg-running text-running-foreground",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

// tailwind-merge は Celeris の文字 token（text-label）を色の class と同じ組と見なし、後の text-*-foreground で
// 消してしまう。lib/utils の cn() を直すまで、文字の大きさは cn() を通さずに足す。
const badgeTextSize = "text-label";

export type BadgeTone = NonNullable<VariantProps<typeof badgeVariants>["tone"]>;

export const badgeTones: readonly BadgeTone[] = ["success", "warning", "danger", "info", "neutral", "running"];

export function Badge({
  className,
  tone,
  asChild = false,
  ...props
}: ComponentProps<"span"> & VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span";
  return (
    <Comp
      data-slot="badge"
      data-tone={tone ?? "neutral"}
      className={`${cn(badgeVariants({ tone }), className)} ${badgeTextSize}`}
      {...props}
    />
  );
}
