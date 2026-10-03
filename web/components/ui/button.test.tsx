import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Button, buttonClassName, buttonVariants } from "./button";

describe("Button", () => {
  it("既定は副操作で type=button、既存 className と属性を渡せる", () => {
    const markup = renderToStaticMarkup(
      <Button className="self-start" aria-label="再取得">
        再取得
      </Button>,
    );
    expect(markup).toContain('type="button"');
    expect(markup).toContain('aria-label="再取得"');
    expect(markup).toContain("self-start");
    expect(markup).toContain("border-input");
    expect(markup).toContain("min-h-11 min-w-11");
    expect(buttonClassName).toBe(buttonVariants({ variant: "secondary" }));
  });

  it.each([
    ["primary", "bg-primary", "text-primary-foreground"],
    ["secondary", "bg-secondary", "border-input"],
    ["ghost", "hover:bg-accent", "text-foreground"],
    ["destructive", "bg-destructive", "text-destructive-foreground"],
  ] as const)("%s variant に対応した token を使う", (variant, background, foreground) => {
    const markup = renderToStaticMarkup(<Button variant={variant}>操作</Button>);
    expect(markup).toContain(background);
    expect(markup).toContain(foreground);
    expect(markup).toContain("focus-visible:outline-ring");
  });

  it.each(["default", "sm", "lg", "icon"] as const)("%s size も 44px target を保つ", (size) => {
    const markup = renderToStaticMarkup(
      <Button size={size} aria-label="操作">
        操作
      </Button>,
    );
    expect(markup).toContain("min-h-11 min-w-11");
  });

  it("明示した type と disabled を保持する", () => {
    const markup = renderToStaticMarkup(
      <Button type="submit" disabled>
        保存
      </Button>,
    );
    expect(markup).toContain('type="submit"');
    expect(markup).toContain('disabled=""');
    expect(markup).toContain("disabled:bg-neutral");
  });
});
