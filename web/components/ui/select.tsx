import type { ComponentProps } from "react";
import { cn } from "../../lib/utils";
import { fieldClassName } from "./input";

/**
 * 素の select。スマホの OS の選択 UI と keyboard 操作をそのまま使うため、Radix の Select ではなく native を包む。
 * 枠・focus ring・44px は Input と同じ（fieldClassName）。
 */
export function Select({ className, children, ...props }: ComponentProps<"select">) {
  return (
    <select data-slot="select" className={cn(fieldClassName, "enabled:cursor-pointer", className)} {...props}>
      {children}
    </select>
  );
}
