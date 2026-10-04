import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { edgeFade, ScrollTabs } from "./scroll-tabs";

describe("edgeFade", () => {
  it("溢れていなければ両端とも出さない", () => {
    expect(edgeFade({ scrollLeft: 0, scrollWidth: 300, clientWidth: 300 })).toEqual({ start: false, end: false });
    expect(edgeFade({ scrollLeft: 0, scrollWidth: 301, clientWidth: 300 })).toEqual({ start: false, end: false });
  });

  it("scroll 位置で左右を出し分ける", () => {
    const m = { scrollWidth: 600, clientWidth: 300 };
    expect(edgeFade({ ...m, scrollLeft: 0 })).toEqual({ start: false, end: true });
    expect(edgeFade({ ...m, scrollLeft: 150 })).toEqual({ start: true, end: true });
    expect(edgeFade({ ...m, scrollLeft: 300 })).toEqual({ start: true, end: false });
    // 小数の丸め（端から 1px 以内）は端とみなす
    expect(edgeFade({ ...m, scrollLeft: 299.5 })).toEqual({ start: true, end: false });
    expect(edgeFade({ ...m, scrollLeft: 0.5 })).toEqual({ start: false, end: true });
  });

  it("RTL の負の scrollLeft も同じ向きで扱う", () => {
    expect(edgeFade({ scrollLeft: -300, scrollWidth: 600, clientWidth: 300 })).toEqual({ start: true, end: false });
  });
});

describe("ScrollTabs", () => {
  it("横 scroll の中身と、装飾の fade 2 つ（初期は隠す）を描く", () => {
    const html = renderToStaticMarkup(
      <ScrollTabs aria-label="タブ" className="flex gap-2">
        <a href="#a">A</a>
      </ScrollTabs>,
    );
    expect(html).toContain('data-slot="scroll-tabs"');
    expect(html).toContain("overflow-x-auto");
    expect(html).toContain('aria-label="タブ"');
    expect(html).toContain("flex gap-2");
    expect(html.match(/aria-hidden="true"/g)?.length).toBe(2);
    expect(html).toContain('data-edge="start" data-visible="false"');
    expect(html).toContain('data-edge="end" data-visible="false"');
    expect(html).toContain("to-background");
    expect(html).toContain("motion-reduce:transition-none");
    expect(html).toContain("pointer-events-none");
  });

  it("surface で fade の色を面に合わせる", () => {
    const html = renderToStaticMarkup(<ScrollTabs surface="surface">x</ScrollTabs>);
    expect(html).toContain("to-surface");
    expect(html).not.toContain("to-background");
  });
});
