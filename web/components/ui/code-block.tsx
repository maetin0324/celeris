import { cva } from "class-variance-authority";
import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "../../lib/utils";

// text-code は color token（--color-code）と衝突するため、同じ 14px の text-label と行高 1.6 相当の leading-relaxed を使う（kbd.tsx と同じ）。
const surface = "min-w-0 max-w-full rounded-sm bg-code p-3 font-mono leading-relaxed text-code-foreground";
// tailwind-merge は text-label と text-code-foreground を同じ色の group と見なして片方を落とすため、cn の外で足す。
const textSize = "text-label";

type SurfaceProps = Omit<HTMLAttributes<HTMLDivElement>, "children"> & {
  /** scroll 領域の accessible name（例「stdout」「設定ファイル」）。keyboard で scroll するため必須。 */
  label: string;
  /** true で長い行を折り返す。既定は原文の改行を保ち、横は内側でだけ scroll する。 */
  wrap?: boolean;
  children: ReactNode;
};

/** code・path・生 JSON の原文表示。横溢れは自分の内側でだけ scroll し、外の layout を広げない。 */
export function CodeBlock({ label, wrap = false, className, children, ...props }: SurfaceProps) {
  return (
    <section
      aria-label={label}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: scroll 領域を keyboard で読めるようにする（WCAG 2.1.1）。
      tabIndex={0}
      data-wrap={wrap ? "true" : "false"}
      className={`${cn(surface, "overflow-x-auto", className)} ${textSize}`}
      {...props}
    >
      <pre className={wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"}>
        <code>{children}</code>
      </pre>
    </section>
  );
}

const logHeight = cva("", {
  variants: {
    size: {
      sm: "max-h-64",
      md: "max-h-96",
      lg: "max-h-screen",
    },
  },
  defaultVariants: { size: "md" },
});

/** 長い生ログ。高さ上限で縦も内側で scroll する。追記の追従や省略行の表示は呼び出し側が持つ。 */
export function LogSurface({
  label,
  wrap = false,
  size: height,
  className,
  children,
  ...props
}: SurfaceProps & { size?: "sm" | "md" | "lg" }) {
  return (
    <section
      aria-label={label}
      // biome-ignore lint/a11y/noNoninteractiveTabindex: scroll 領域を keyboard で読めるようにする（WCAG 2.1.1）。
      tabIndex={0}
      data-wrap={wrap ? "true" : "false"}
      className={`${cn(surface, "overflow-auto overscroll-contain", logHeight({ size: height }), className)} ${textSize}`}
      {...props}
    >
      <pre className={wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre"}>{children}</pre>
    </section>
  );
}
