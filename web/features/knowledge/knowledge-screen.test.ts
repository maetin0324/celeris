import { describe, expect, it } from "vitest";
import { withoutLeadingTitle } from "./knowledge-screen";

describe("withoutLeadingTitle", () => {
  it("title と同じ先頭の見出しを外す", () => {
    expect(withoutLeadingTitle("# Demo knowledge\n\nKnowledge body", "Demo knowledge")).toBe("\nKnowledge body");
    expect(withoutLeadingTitle("# New knowledge", "New knowledge")).toBe("");
  });
  it("front matter は残して、その後の見出しを外す", () => {
    expect(withoutLeadingTitle("---\nscope: a\n---\n## Title ##\nbody", "Title")).toBe("---\nscope: a\n---\nbody");
  });
  it("title と違う見出しや途中の見出しは残す", () => {
    expect(withoutLeadingTitle("# Other\nbody", "Title")).toBe("# Other\nbody");
    expect(withoutLeadingTitle("intro\n# Title", "Title")).toBe("intro\n# Title");
  });
});
