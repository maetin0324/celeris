import { describe, expect, it } from "vitest";
import { diffLineKind, diffLines, fileStatusView, integrationTone } from "./diff-lines";

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
