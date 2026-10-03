import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Badge, badgeTones } from "./badge";

describe("Badge", () => {
  it("6 tone がそれぞれ token の背景と前景の組を使う", () => {
    expect(badgeTones).toEqual(["success", "warning", "danger", "info", "neutral", "running"]);
    for (const tone of badgeTones) {
      const html = renderToStaticMarkup(<Badge tone={tone}>ラベル</Badge>);
      expect(html).toContain(`data-tone="${tone}"`);
      expect(html).toContain(`bg-${tone} `);
      expect(html).toContain(`text-${tone}-foreground`);
      expect(html).toContain(">ラベル</span>");
      // 色の class と文字の大きさの class が両方残る（tailwind-merge に消されない）
      expect(html).toContain("text-label");
    }
  });

  it("tone の既定は neutral で、pill にしない", () => {
    const html = renderToStaticMarkup(<Badge>既定</Badge>);
    expect(html).toContain('data-slot="badge"');
    expect(html).toContain('data-tone="neutral"');
    expect(html).toContain("bg-neutral");
    expect(html).toContain("rounded-sm");
    expect(html).not.toContain("rounded-full");
  });

  it("className を足せて、asChild で子の要素を使う", () => {
    const html = renderToStaticMarkup(
      <Badge tone="info" asChild>
        <a href="/tasks">一覧</a>
      </Badge>,
    );
    expect(html).toMatch(/^<a /);
    expect(html).toContain('href="/tasks"');
    expect(html).toContain("bg-info");
  });
});
