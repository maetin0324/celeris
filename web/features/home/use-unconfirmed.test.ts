import { describe, expect, it } from "vitest";
import { stickyUnconfirmed } from "./use-unconfirmed";

describe("stickyUnconfirmed", () => {
  it("再接続中・未接続の後は、再試行の『接続を確認中』でも未確認のまま", () => {
    let unconfirmed = stickyUnconfirmed(false, "connecting");
    expect(unconfirmed).toBe(false);
    for (const state of ["reconnecting", "connecting", "reconnecting", "connecting"] as const) {
      unconfirmed = stickyUnconfirmed(unconfirmed, state);
      expect(unconfirmed).toBe(true);
    }
    expect(stickyUnconfirmed(true, "closed")).toBe(true);
  });

  it("繋がるか認証が要るときは未確認を解く", () => {
    expect(stickyUnconfirmed(true, "open")).toBe(false);
    expect(stickyUnconfirmed(true, "unauthorized")).toBe(false);
    expect(stickyUnconfirmed(false, "open")).toBe(false);
  });
});
