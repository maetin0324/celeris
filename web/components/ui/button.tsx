import type { ButtonHTMLAttributes } from "react";

// 基本のボタン（P2-02）。タップ領域は 44×44 px 以上（S3）。Base UI 系の部品は依存を足す葉で置き換える。
export const buttonClassName =
  "inline-flex min-h-11 min-w-11 items-center justify-center gap-2 rounded border border-neutral-400 px-3 text-base font-medium text-neutral-900 hover:bg-neutral-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-700 disabled:opacity-60";

export function Button({ className, type = "button", ...props }: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button type={type} className={className ? `${buttonClassName} ${className}` : buttonClassName} {...props} />;
}
