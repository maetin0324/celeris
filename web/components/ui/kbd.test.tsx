import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Kbd } from "./kbd";

describe("Kbd", () => {
  it("kbd 要素に token と等幅書体を適用し、属性も渡す", () => {
    const markup = renderToStaticMarkup(<Kbd title="ショートカット">Ctrl</Kbd>);
    expect(markup).toContain("<kbd");
    expect(markup).toContain("bg-code");
    expect(markup).toContain("border-border");
    expect(markup).toContain("font-mono");
    expect(markup).toContain('title="ショートカット"');
    expect(markup).toContain("Ctrl</kbd>");
  });
});
