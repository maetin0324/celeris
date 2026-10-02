import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
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
