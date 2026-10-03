import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it.each(["title", "title-wide", "section", "body", "label", "code"])(
    "%s の文字サイズと前景色を順序によらず保持する",
    (size) => {
      expect(cn(`text-${size} text-muted-foreground`)).toBe(`text-${size} text-muted-foreground`);
      expect(cn(`text-foreground text-${size}`)).toBe(`text-foreground text-${size}`);
      expect(cn(`text-${size}`, "text-sm")).toBe("text-sm");
      expect(cn("text-sm", `text-${size}`)).toBe(`text-${size}`);
    },
  );

  it("responsive のサイズと色も分け、同じ種類だけ後勝ちにする", () => {
    expect(cn("md:text-body md:text-primary", "md:text-title md:text-danger-foreground")).toBe(
      "md:text-title md:text-danger-foreground",
    );
  });

  it("joins truthy class values", () => {
    expect(cn("a", false && "b", undefined, "c")).toBe("a c");
  });

  it("resolves conflicting tailwind classes by keeping the last one", () => {
    expect(cn("px-2 py-1", "px-4")).toBe("py-1 px-4");
  });

  it("merges conditional object and array inputs", () => {
    expect(cn(["bg-surface", { "text-danger-foreground": true, hidden: false }])).toBe(
      "bg-surface text-danger-foreground",
    );
  });
});

describe("cn の省略経路", () => {
  it("出どころが文字列 1 つなら merge せずにそのまま返し、空は空文字", () => {
    expect(cn("px-2 py-1", undefined, false)).toBe("px-2 py-1");
    expect(cn(undefined, null, false)).toBe("");
  });

  it("文字列以外の入力 1 つや複数の入力は merge する", () => {
    expect(cn(["px-2", "px-4"])).toBe("px-4");
    expect(cn({ "px-2": true }, "px-4")).toBe("px-4");
  });
});
