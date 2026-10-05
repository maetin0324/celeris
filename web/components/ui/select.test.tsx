import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Select } from "./select";

describe("Select", () => {
  it("native の select を Input と同じ枠・ring・44px で描く", () => {
    const html = renderToStaticMarkup(
      <Select aria-label="状態" defaultValue="b">
        <option value="a">A</option>
        <option value="b">B</option>
      </Select>,
    );
    expect(html).toMatch(/^<select data-slot="select"/);
    for (const token of ["border-input", "min-h-11", "focus-visible:outline-ring", "enabled:cursor-pointer"])
      expect(html).toContain(token);
    expect(html).toContain('<option value="b" selected="">B</option>');
  });

  it("className と属性を渡せる", () => {
    const html = renderToStaticMarkup(
      <Select name="s" className="w-auto" disabled>
        <option>x</option>
      </Select>,
    );
    expect(html).toContain('name="s"');
    expect(html).toContain("w-auto");
    expect(html).not.toContain("w-full");
    expect(html).toContain("disabled");
  });
});
