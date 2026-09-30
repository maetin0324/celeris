import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// file-based routing の生成物に /login と __root が載っていることを確かめる（build 前の typecheck で生成される）。
describe("route tree", () => {
  it("declares /login under __root", () => {
    const tree = readFileSync(new URL("../routeTree.gen.ts", import.meta.url), "utf8");
    expect(tree).toMatch(/["']\/login["']/);
    expect(tree).toContain("__root");
  });
});
