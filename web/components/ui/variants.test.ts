import { describe, expect, it } from "vitest";
import { cn, mergeClasses } from "../../lib/utils";
import { badgeTones, badgeVariants } from "./badge";
import { buttonClassName, buttonVariants } from "./button";

// cn は class の出どころが文字列 1 つなら tailwind-merge を省く（lib/utils.ts）。
// 部品の定義（cva の全 variant）が単独で衝突せず、merge しても同じ文字列になることを確かめる。
describe("部品の class 定義は単独で衝突しない", () => {
  const variants = ["primary", "secondary", "ghost", "destructive"] as const;
  const sizes = ["default", "sm", "lg", "icon"] as const;

  it("Button の全 variant × size", () => {
    for (const variant of variants) {
      for (const size of sizes) {
        const classes = buttonVariants({ variant, size });
        expect(cn(classes)).toBe(mergeClasses(classes));
      }
    }
    expect(cn(buttonClassName)).toBe(mergeClasses(buttonClassName));
  });

  it("Badge の全 tone", () => {
    for (const tone of badgeTones) {
      const classes = badgeVariants({ tone });
      expect(cn(classes)).toBe(mergeClasses(classes));
    }
  });

  it("className を重ねたときは従来どおり後勝ちで解決する", () => {
    expect(cn(buttonVariants({ size: "sm" }), "px-6")).toBe(mergeClasses(`${buttonVariants({ size: "sm" })} px-6`));
    expect(cn(badgeVariants({ tone: "info" }), "px-3")).not.toContain("px-2");
  });
});
