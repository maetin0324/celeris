import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AUTO_INTEGRATE_ACTION_ID, AutoIntegrateToggle, autoIntegrateAction } from "./auto-integrate";

describe("自動取り込みの切替", () => {
  it("PATCH /api/projects/{id} に auto_integrate だけを送る", () => {
    expect(autoIntegrateAction("01ABC", true)).toEqual({
      id: AUTO_INTEGRATE_ACTION_ID,
      method: "PATCH",
      path: "/api/projects/01ABC",
      body: { auto_integrate: true },
    });
    expect(autoIntegrateAction("a/b", false).path).toBe("/api/projects/a%2Fb");
    expect(autoIntegrateAction("01ABC", false).body).toEqual({ auto_integrate: false });
  });

  it("現在の値をチェックに映し、送信中は触れない", () => {
    const on = renderToStaticMarkup(<AutoIntegrateToggle enabled pending={false} onToggle={() => {}} />);
    expect(on).toContain('type="checkbox"');
    expect(on).toContain("checked");
    expect(on).not.toContain("disabled");
    const off = renderToStaticMarkup(<AutoIntegrateToggle enabled={false} pending onToggle={() => {}} />);
    expect(off).not.toContain("checked");
    expect(off).toContain("disabled");
  });
});
