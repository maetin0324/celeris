import { describe, expect, it } from "vitest";
import type { InboxItem } from "../../api/generated/types";
import { accountsSecretHref, credentialSecretId, nativeTarget } from "../inbox/inbox-model";
import { credentialPhase, credentialWaitRows, VERIFY_GRACE_MS } from "./credential-wait-model";

const waitItem = (title: string, secret = "jev_api_key"): InboxItem => ({
  id: `credential_wait-${secret}`,
  kind: "credential_wait",
  title,
  options: [{ key: "register", label: "登録する", effect: "", needs_note: false }],
  blocking: {
    tasks: [{ id: "T1", title: "分類", kind: "execute", status: "blocked", actions: [] }],
    units: ["u1", "u2"],
    summary: "2 件の work unit だけ",
  },
  blocked_by: [],
  answer: {
    method: "POST",
    path: "/api/v1/inbox/items/x/answer",
    body_schema: {},
    native: { method: "PUT", path: `/api/v1/secrets/${secret}` },
  },
  created_at: "2026-10-15T00:00:00Z",
  age_secs: 0,
  links: [{ label: "アカウント画面（秘密の登録）", href: "/accounts" }],
});

describe("credential wait view", () => {
  it("secret id を native path から取り、accounts の focus へ誘導する", () => {
    const item = waitItem("API キーの登録が必要: jev");
    expect(credentialSecretId(item)).toBe("jev_api_key");
    expect(nativeTarget(item).href).toBe(accountsSecretHref("jev_api_key"));
    expect(accountsSecretHref("a b")).toBe("/accounts?secret=a%20b");
  });

  it("登録待ち → 検証中 → 再登録待ちを見分ける", () => {
    const fresh = waitItem("API キーの登録が必要: jev");
    const rejected = waitItem("API キーを更新してください: jev");
    expect(credentialPhase({ item: fresh, secret: { fingerprint: null }, savedAgoMs: undefined })).toBe("awaiting_key");
    expect(credentialPhase({ item: fresh, secret: { fingerprint: "ab12" }, savedAgoMs: undefined })).toBe("verifying");
    expect(credentialPhase({ item: fresh, secret: { fingerprint: null }, savedAgoMs: 1000 })).toBe("verifying");
    expect(credentialPhase({ item: rejected, secret: { fingerprint: "ab12" }, savedAgoMs: undefined })).toBe(
      "reregister",
    );
    // 誤キーを更新した直後は検証中、猶予を過ぎても更新要求のままなら再登録待ちに戻る。
    expect(credentialPhase({ item: rejected, secret: { fingerprint: "cd34" }, savedAgoMs: 1000 })).toBe("verifying");
    expect(credentialPhase({ item: rejected, secret: { fingerprint: "cd34" }, savedAgoMs: VERIFY_GRACE_MS })).toBe(
      "reregister",
    );
  });

  it("待っている task と unit を行にする", () => {
    const rows = credentialWaitRows([waitItem("API キーの登録が必要: jev")], [], {});
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ secretId: "jev_api_key", phase: "awaiting_key", units: ["u1", "u2"] });
    expect(rows[0]?.tasks[0]?.id).toBe("T1");
  });
});
