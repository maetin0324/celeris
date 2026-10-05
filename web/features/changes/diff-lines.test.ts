import { describe, expect, it } from "vitest";
import { diffLineKind, diffLines, fileStatusView, integrationTone, splitChangedPaths } from "./diff-lines";

describe("diffLines", () => {
  it("行の種類を見分け、末尾の改行で空行を足さない", () => {
    const lines = diffLines("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same\n");
    expect(lines.map((line) => line.kind)).toEqual(["meta", "meta", "hunk", "remove", "add", "context"]);
    expect(lines.at(-1)?.text).toBe(" same");
  });

  it("diff・index・改行なしの印は meta", () => {
    expect(diffLineKind("diff --git a/x b/x")).toBe("meta");
    expect(diffLineKind("index 1..2 100644")).toBe("meta");
    expect(diffLineKind("\\ No newline at end of file")).toBe("meta");
  });
});

describe("fileStatusView", () => {
  it("既知の status は日本語の可視ラベル、未知は原文", () => {
    expect(fileStatusView("M")).toEqual({ label: "変更", tone: "info" });
    expect(fileStatusView("R100")).toEqual({ label: "名前変更", tone: "neutral" });
    expect(fileStatusView("X")).toEqual({ label: "X", tone: "neutral" });
  });
});

describe("integrationTone", () => {
  it("衝突は danger、merge 済みは success", () => {
    expect(integrationTone("conflict")).toBe("danger");
    expect(integrationTone("merged")).toBe("success");
    expect(integrationTone("open")).toBe("info");
  });
});

describe("splitChangedPaths", () => {
  it("2 件以上に共通の dir を取り出し、各行はファイル名と残りの dir に分ける", () => {
    const split = splitChangedPaths(["src/a/b/x.tsx", "src/a/b/c/y.tsx", "src/a/b/z.ts"]);
    expect(split.common).toBe("src/a/b/");
    expect(split.files.map((file) => [file.name, file.dir])).toEqual([
      ["x.tsx", ""],
      ["y.tsx", "c/"],
      ["z.ts", ""],
    ]);
    expect(split.files[1]?.path).toBe("src/a/b/c/y.tsx");
  });

  it("名前の途中では切らず、1 件や共通が無いときは共通 dir を空にする", () => {
    expect(splitChangedPaths(["src/ab/x.ts", "src/ac/y.ts"]).common).toBe("src/");
    expect(splitChangedPaths(["src/a/x.ts"])).toEqual({
      common: "",
      files: [{ path: "src/a/x.ts", dir: "src/a/", name: "x.ts" }],
    });
    expect(splitChangedPaths(["a.ts", "b/c.ts"]).common).toBe("");
  });
});
