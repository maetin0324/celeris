import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Icon, IconOnlyButton } from "./icon";

describe("Icon", () => {
  it("lucide glyph を token の 20px で線幅 2、装飾扱いにする", () => {
    const markup = renderToStaticMarkup(<Icon name="refresh" />);
    expect(markup).toContain('width="var(--spacing-icon)"');
    expect(markup).toContain('height="var(--spacing-icon)"');
    expect(markup).toContain('stroke-width="2"');
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).toContain('focusable="false"');
    expect(markup).toContain('stroke="currentColor"');
  });

  it("行内 icon は 16px token を使う", () => {
    expect(renderToStaticMarkup(<Icon name="check" size="sm" />)).toContain('width="var(--spacing-icon-sm)"');
  });

  it("icon-only 操作は accessible name と 44px target を持つ", () => {
    const markup = renderToStaticMarkup(<IconOnlyButton name="close" label="閉じる" />);
    expect(markup).toContain('aria-label="閉じる"');
    expect(markup).toContain("min-h-11 min-w-11");
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).toContain('type="button"');
  });
});
