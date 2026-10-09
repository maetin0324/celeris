import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BrowserWait } from "../../api/generated/types";
import { BrowserGatewayError } from "./browser-query";
import {
  BrowserWaitsList,
  submitCredential,
  submitDecision,
  TrustedLoginSummary,
  waitActionMessage,
} from "./browser-waits-panel";

afterEach(() => vi.unstubAllGlobals());

const decisionWait = (over: Partial<BrowserWait> = {}): BrowserWait => ({
  created_at: "2026-10-05T00:00:00Z",
  deadline: "2026-10-05T00:05:00Z",
  origin: "https://billing.example.com",
  policy_hash: "abcdef0123456789",
  policy_revision: 3,
  purpose: "請求書の承認",
  reason: "waiting_for_approval",
  resume_key: "resume-1",
  run_id: "R1",
  session_id: "S1",
  state: "pending",
  task_id: "T1",
  version: 1,
  wait_id: "W1",
  operation: { action: "click_submit", args_digest: "digest-1", intent_id: "I1" },
  ...over,
});

const credentialWait = (over: Partial<BrowserWait> = {}): BrowserWait => ({
  ...decisionWait(over),
  reason: "waiting_for_auth",
  wait_id: "W2",
  operation: undefined,
  ...over,
});

const owner = { available: true, isOwner: true, csrfToken: "csrf-token" };

describe("waitActionMessage", () => {
  it("maps known codes to fixed Japanese text and falls back for unknown codes", () => {
    expect(waitActionMessage("approved")).toBe("一回だけ承認しました。");
    expect(waitActionMessage("totally-unknown-code")).toBe("操作に失敗しました。");
  });
});

describe("submitDecision", () => {
  it("reports the mapped message for a successful send", async () => {
    await expect(submitDecision(async () => ({ ok: true, code: "approved" }))).resolves.toEqual({
      ok: true,
      message: "一回だけ承認しました。",
    });
  });

  it("maps a gateway error code to a fixed message instead of surfacing the raw error", async () => {
    await expect(
      submitDecision(async () => {
        throw new BrowserGatewayError(409, "version_conflict");
      }),
    ).resolves.toEqual({ ok: false, message: "依頼の状態が変わりました。画面を読み込み直してください。" });
  });
});

describe("submitCredential", () => {
  it("always clears username and password on success, and never echoes them", async () => {
    const send = vi.fn(async () => ({ ok: true as const, code: "registered" }));
    const result = await submitCredential(send, "alice", "s3cret");
    expect(send).toHaveBeenCalledWith("alice", "s3cret");
    expect(result.username).toBe("");
    expect(result.password).toBe("");
    expect(result.outcome).toEqual({ ok: true, message: "登録しました。使う前に改めて承認を求めます。" });
  });

  it("still clears username and password when the send fails", async () => {
    const send = vi.fn(async () => {
      throw new BrowserGatewayError(422, "invalid_input");
    });
    const result = await submitCredential(send, "alice", "s3cret");
    expect(result.username).toBe("");
    expect(result.password).toBe("");
    expect(result.outcome).toEqual({ ok: false, message: "入力を確認してください。" });
  });
});

describe("BrowserWaitsList", () => {
  it("renders nothing for an empty wait list", () => {
    expect(renderToStaticMarkup(<BrowserWaitsList waits={[]} owner={owner} />)).toBe("");
  });

  it("shows the decision form with operation action, args digest, origin and policy revision", () => {
    const out = renderToStaticMarkup(<BrowserWaitsList waits={[decisionWait()]} owner={owner} />);
    expect(out).toContain('data-testid="browser-decision-form"');
    expect(out).not.toContain('data-testid="browser-credential-form"');
    expect(out).toContain("click_submit");
    expect(out).toContain("digest-1");
    expect(out).toContain("billing.example.com");
    expect(out).toContain("3");
    expect(out).toContain("一回だけ承認");
    expect(out).toContain("拒否");
  });

  it("shows the credential form with labeled, autocomplete-off username and password fields", () => {
    const out = renderToStaticMarkup(<BrowserWaitsList waits={[credentialWait()]} owner={owner} />);
    expect(out).toContain('data-testid="browser-credential-form"');
    expect(out).not.toContain('data-testid="browser-decision-form"');
    expect(out).toContain("ユーザー名");
    expect(out).toContain("パスワード");
    expect(out).toContain('type="password"');
    expect(out.match(/autoComplete="off"/gi)?.length).toBeGreaterThanOrEqual(2);
  });

  it("shows the owner-unavailable notice instead of any form when owner session is missing", () => {
    const out = renderToStaticMarkup(
      <BrowserWaitsList
        waits={[decisionWait(), credentialWait()]}
        owner={{ available: false, isOwner: false, csrfToken: null }}
      />,
    );
    expect(out).toContain('data-testid="browser-owner-unavailable"');
    expect(out).not.toContain('data-testid="browser-decision-form"');
    expect(out).not.toContain('data-testid="browser-credential-form"');
  });

  it("shows the register-as-owner notice instead of any form for a non-owner session", () => {
    const out = renderToStaticMarkup(
      <BrowserWaitsList waits={[credentialWait()]} owner={{ available: true, isOwner: false, csrfToken: null }} />,
    );
    expect(out).toContain('data-testid="browser-owner-request"');
    expect(out).not.toContain('data-testid="browser-credential-form"');
  });

  it("renders no form and no notice while the owner session is still loading", () => {
    const out = renderToStaticMarkup(<BrowserWaitsList waits={[decisionWait()]} owner={undefined} />);
    expect(out).not.toContain('data-testid="browser-decision-form"');
    expect(out).not.toContain('data-testid="browser-owner-unavailable"');
    expect(out).not.toContain('data-testid="browser-owner-request"');
  });

  it("does not offer a form for an already-resolved wait, even for the owner", () => {
    const out = renderToStaticMarkup(<BrowserWaitsList waits={[decisionWait({ state: "approved" })]} owner={owner} />);
    expect(out).not.toContain('data-testid="browser-decision-form"');
    expect(out).not.toContain('data-testid="browser-credential-form"');
  });
});

describe("TrustedLoginSummary", () => {
  const login = {
    policy_id: "manaba",
    revision: 1,
    login_url: "https://idp.example.ac.jp/idp/profile/SAML2/Unsolicited/SSO?providerId=lms",
    password_selector: 'input[name="j_password"]',
    submit_selector: 'button[name="_eventId_proceed"]',
  };
  it("names both input fields and states the post-login read on every credential_use approval", () => {
    const html = renderToStaticMarkup(
      <TrustedLoginSummary
        wait={decisionWait({
          operation: { action: "credential_use", intent_id: "I9" },
          trusted_login: {
            ...login,
            username_selector: 'input[name="j_username"]',
            post_login: { read_origins: ["https://lms.example.ac.jp"], actions: ["snapshot", "extract", "click"] },
          },
        })}
      />,
    );
    expect(html).toContain("j_username");
    expect(html).toContain("（username）");
    expect(html).toContain("https://lms.example.ac.jp（snapshot, extract, click）");
    expect(html).toContain("LLM");
    expect(html).toContain("password 欄のある頁は読み取りません");
  });
  it("says nothing is read after login without an opt-in, and renders nothing without a pinned login", () => {
    const html = renderToStaticMarkup(
      <TrustedLoginSummary
        wait={decisionWait({ operation: { action: "credential_use", intent_id: "I9" }, trusted_login: login })}
      />,
    );
    expect(html).toContain("しない（session の終わりまで頁を読まない）");
    expect(html).not.toContain("LLM");
    expect(renderToStaticMarkup(<TrustedLoginSummary wait={decisionWait()} />)).toBe("");
  });
});
