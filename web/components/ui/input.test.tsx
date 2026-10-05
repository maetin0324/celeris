import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { fieldClassName, Input } from "./input";

describe("Input", () => {
  it("枠は border-input、focus-visible の ring、44px 以上", () => {
    const html = renderToStaticMarkup(<Input aria-label="名前" />);
    expect(html).toMatch(/^<input data-slot="input" type="text"/);
    for (const token of [
      "border-input",
      "min-h-11",
      "focus-visible:outline-2",
      "focus-visible:outline-ring",
      "bg-surface",
      "text-body",
    ])
      expect(html).toContain(token);
    expect(fieldClassName).not.toMatch(/(^|\s)focus:outline-none/);
  });

  it("type・属性・className を渡し、className は後勝ちで合成する", () => {
    const html = renderToStaticMarkup(<Input type="search" name="q" disabled className="px-2" />);
    expect(html).toContain('type="search"');
    expect(html).toContain('name="q"');
    expect(html).toContain("disabled");
    expect(html).toContain("px-2");
    expect(html).not.toContain("px-3");
  });
});
