import { describe, expect, it } from "vitest";
import { BrowserGatewayError } from "./browser-query";
import {
  canRestoreIdentity,
  canRevokeIdentity,
  formatIdentityExpiry,
  identityErrorMessage,
  identityStateLabel,
  identityStateTone,
  parseIdentityStateJson,
} from "./identities-screen";

describe("identityStateLabel / identityStateTone", () => {
  it("labels active as 有効 with a success tone", () => {
    expect(identityStateLabel("active")).toBe("有効");
    expect(identityStateTone("active")).toBe("success");
  });

  it("labels revoked as 失効済み with a neutral tone", () => {
    expect(identityStateLabel("revoked")).toBe("失効済み");
    expect(identityStateTone("revoked")).toBe("neutral");
  });

  it("falls back to the raw state for an unknown value", () => {
    expect(identityStateLabel("deleted")).toBe("deleted");
  });
});

describe("canRevokeIdentity / canRestoreIdentity", () => {
  it("allows revoke and restore only while active", () => {
    expect(canRevokeIdentity({ state: "active" })).toBe(true);
    expect(canRestoreIdentity({ state: "active" })).toBe(true);
  });

  it("disallows revoke and restore once revoked or deleted", () => {
    expect(canRevokeIdentity({ state: "revoked" })).toBe(false);
    expect(canRestoreIdentity({ state: "revoked" })).toBe(false);
    expect(canRevokeIdentity({ state: "deleted" })).toBe(false);
    expect(canRestoreIdentity({ state: "deleted" })).toBe(false);
  });
});

describe("formatIdentityExpiry", () => {
  it("formats epoch seconds independent of the local timezone", () => {
    expect(formatIdentityExpiry(1790604800)).toBe("2026-09-28 14:13:20Z");
  });

  it("formats an ISO string the same way", () => {
    expect(formatIdentityExpiry("2026-09-28T14:13:20Z")).toBe("2026-09-28 14:13:20Z");
  });

  it("falls back to the raw value when it cannot parse", () => {
    expect(formatIdentityExpiry("not-a-date")).toBe("not-a-date");
  });
});

describe("parseIdentityStateJson", () => {
  it("accepts an object with an entries array", () => {
    const input = '{"entries":[{"origin":"https://billing.example.com","kind":"cookie","name":"s","value":"v"}]}';
    const result = parseIdentityStateJson(input);
    expect(result.ok).toBe(true);
    expect(result.ok && result.state.entries).toHaveLength(1);
  });

  it("rejects text that is not JSON", () => {
    const result = parseIdentityStateJson("{not json");
    expect(result).toEqual({ ok: false, error: "JSON の形式が正しくありません。" });
  });

  it("rejects JSON that has no entries array", () => {
    const result = parseIdentityStateJson('{"entries":"nope"}');
    expect(result).toEqual({ ok: false, error: "entries の配列を含む JSON にしてください。" });
  });

  it("rejects a bare JSON array or primitive", () => {
    expect(parseIdentityStateJson("[]").ok).toBe(false);
    expect(parseIdentityStateJson("42").ok).toBe(false);
  });
});

describe("identityErrorMessage", () => {
  it("maps a known gateway code to its fixed text", () => {
    expect(identityErrorMessage(new BrowserGatewayError(409, "identity_not_restorable"))).toBe(
      "いまは復元できません。状態を確認してください。",
    );
  });

  it("falls back to a generic message for an unknown code or non-gateway error", () => {
    expect(identityErrorMessage(new BrowserGatewayError(500, "something_new"))).toBe("操作に失敗しました。");
    expect(identityErrorMessage(new TypeError("boom"))).toBe("操作に失敗しました。");
  });
});
