import { describe, expect, it } from "vitest";
import { deploymentModelId, reachabilityLabel } from "./sources-screen";

describe("source 表示", () => {
  it("到達性が欠けている状態を不明として表示する", () => {
    expect(reachabilityLabel(true)).toBe("到達可能");
    expect(reachabilityLabel(false)).toBe("到達不可");
    expect(reachabilityLabel(null)).toBe("不明");
  });

  it("deployment ID から model ID を取り出す", () => {
    expect(deploymentModelId("claude_oauth/claude-opus")).toBe("claude-opus");
  });
});
