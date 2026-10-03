import type { HTMLAttributes } from "react";
import { cn } from "../../lib/utils";

export function Kbd({ className, ...props }: HTMLAttributes<HTMLElement>) {
  return (
    <kbd
      className={cn(
        "inline-flex items-center rounded-sm border border-border bg-code px-2 py-1 font-mono text-code text-code-foreground",
        className,
      )}
      {...props}
    />
  );
}
