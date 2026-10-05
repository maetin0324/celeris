import { type HTMLAttributes, useCallback, useEffect, useRef, useState } from "react";
import { cn } from "../../lib/utils";

export type ScrollMetrics = { scrollLeft: number; scrollWidth: number; clientWidth: number };
export type EdgeFade = { start: boolean; end: boolean };

/** 端から 1px 以内は端に着いたとみなす（小数の scrollLeft と拡大率の丸め）。RTL の負の scrollLeft も扱う。 */
export function edgeFade({ scrollLeft, scrollWidth, clientWidth }: ScrollMetrics): EdgeFade {
  const overflow = scrollWidth - clientWidth;
  if (overflow <= 1) return { start: false, end: false };
  const offset = Math.abs(scrollLeft);
  return { start: offset > 1, end: offset < overflow - 1 };
}

const fadeBase =
  "pointer-events-none absolute inset-y-0 w-8 from-transparent transition-opacity duration-150 motion-reduce:transition-none";

const fadeTo = { background: "to-background", surface: "to-surface" } as const;

export type ScrollTabsProps = HTMLAttributes<HTMLDivElement> & {
  /** fade の終わりの色。置く面の地に合わせる（既定は画面の地）。 */
  surface?: keyof typeof fadeTo;
};

/**
 * 横 scroll する tab 列の外枠。続きがある側の端だけを地の色へ fade させ、溢れていることを見せる。
 * tab の意味（tablist・nav の link 列）と roving は children 側が持つ。fade は装飾なので aria-hidden。
 */
export function ScrollTabs({ surface = "background", className, children, onScroll, ...props }: ScrollTabsProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState<EdgeFade>({ start: false, end: false });
  const update = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const next = edgeFade(el);
    setFade((prev) => (prev.start === next.start && prev.end === next.end ? prev : next));
  }, []);
  useEffect(() => {
    update();
    const el = ref.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(update);
    observer.observe(el);
    for (const child of Array.from(el.children)) observer.observe(child);
    return () => observer.disconnect();
  }, [update]);
  return (
    <div data-slot="scroll-tabs" className="relative min-w-0 max-w-full">
      <div
        ref={ref}
        onScroll={(event) => {
          update();
          onScroll?.(event);
        }}
        className={cn("min-w-0 overflow-x-auto overscroll-x-contain", className)}
        {...props}
      >
        {children}
      </div>
      <span
        aria-hidden="true"
        data-edge="start"
        data-visible={fade.start ? "true" : "false"}
        className={cn(fadeBase, "left-0 bg-linear-to-l", fadeTo[surface], fade.start ? "opacity-100" : "opacity-0")}
      />
      <span
        aria-hidden="true"
        data-edge="end"
        data-visible={fade.end ? "true" : "false"}
        className={cn(fadeBase, "right-0 bg-linear-to-r", fadeTo[surface], fade.end ? "opacity-100" : "opacity-0")}
      />
    </div>
  );
}
