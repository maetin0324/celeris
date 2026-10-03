import type { HTMLAttributes } from "react";
import { cn } from "../../lib/utils";

export function Kbd({ className, ...props }: HTMLAttributes<HTMLElement>) {
  return (
    <kbd
      // text-code は color token と衝突するため、同じ 14px の text-label を使う。
      className={`${cn("inline-flex items-center rounded-sm border border-border bg-code px-2 py-1 font-mono text-code-foreground", className)} text-label`}
      {...props}
    />
  );
}
