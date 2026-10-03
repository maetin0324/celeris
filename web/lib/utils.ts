import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// styles.css の --text-* を文字サイズとして扱い、text-*-foreground と両立させる。
const merge = extendTailwindMerge({
  extend: { theme: { text: ["title", "title-wide", "section", "body", "label", "code"] } },
});

/** tailwind-merge の解決だけを行う（cn の省略経路が merge と同じ結果になることの試験で使う）。 */
export const mergeClasses = merge;

/** class を結合し、Tailwind の衝突するクラスを後勝ちで解決する（shadcn 由来の共通部品で使う）。 */
export function cn(...inputs: ClassValue[]): string {
  // 衝突は部品の定義に呼び出し側の className を重ねたときに起きる。出どころが文字列 1 つだけなら
  // merge を省く。tailwind-merge は最初の呼び出しで class の対応表を作り、起動直後の main thread を
  // 塞ぐ（CPU 6x で約 35ms）。className を渡さない shell の Badge などはこれを払わなくてよい。
  // 部品の定義そのものが衝突しないことは utils.test.ts で全 variant について確かめる。
  let sources = 0;
  let onlyString = true;
  for (const input of inputs) {
    if (!input) continue;
    sources += 1;
    if (typeof input !== "string") onlyString = false;
  }
  if (sources === 0) return "";
  if (sources === 1 && onlyString) return clsx(inputs);
  return merge(clsx(inputs));
}
