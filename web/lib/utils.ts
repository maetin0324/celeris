import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// styles.css の --text-* を文字サイズとして扱い、text-*-foreground と両立させる。
const merge = extendTailwindMerge({
  extend: { theme: { text: ["title", "title-wide", "section", "body", "label", "code"] } },
});

/** class を結合し、Tailwind の衝突するクラスを後勝ちで解決する（shadcn 由来の共通部品で使う）。 */
export function cn(...inputs: ClassValue[]): string {
  return merge(clsx(inputs));
}
