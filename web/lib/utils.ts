import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/** class を結合し、Tailwind の衝突するクラスを後勝ちで解決する（shadcn 由来の共通部品で使う）。 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
