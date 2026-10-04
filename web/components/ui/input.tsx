import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";

// 入力欄の共通 class。枠は --color-input（白地 3.80:1・地 3.53:1、WCAG 1.4.11 の 3:1）、focus は focus-visible の ring、
// 高さは 44px（min-h-11）。無効状態の地は styles.css の base（input:disabled）が持つ。
export const fieldClassName =
  "block min-h-11 w-full min-w-0 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-invalid:border-destructive";

/** shadcn の Input を Celeris token に合わせた素の input。label との対応は呼び出し側が id / htmlFor で持つ。 */
export function Input({ className, type = "text", ...props }: ComponentProps<"input">) {
  return <input data-slot="input" type={type} className={cn(fieldClassName, className)} {...props} />;
}
