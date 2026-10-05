import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ChallengeCommand,
  issueOwnerChallenge,
  OwnerSessionNotice,
  ownerApproveCommand,
  ownerNoticeReason,
} from "./owner-session-notice";

afterEach(() => vi.unstubAllGlobals());

describe("ownerNoticeReason", () => {
  it("treats a missing or unavailable owner session as owner_unavailable", () => {
    expect(ownerNoticeReason(undefined)).toBe("owner_unavailable");
    expect(ownerNoticeReason({ available: false, isOwner: false })).toBe("owner_unavailable");
  });

  it("asks a registered-but-not-owner session to register", () => {
    expect(ownerNoticeReason({ available: true, isOwner: false })).toBe("not_owner");
  });

  it("needs no notice once the session is the registered owner", () => {
    expect(ownerNoticeReason({ available: true, isOwner: true })).toBeNull();
  });
});

describe("ownerApproveCommand", () => {
  it("includes the challenge and the web gateway socket flag", () => {
    expect(ownerApproveCommand("ABCDEF123456")).toBe(
      "celerisctl browser owner-session approve ABCDEF123456 --socket <path>",
    );
  });
});

describe("ChallengeCommand", () => {
  it("renders the exact CLI command for the issued challenge", () => {
    const out = renderToStaticMarkup(<ChallengeCommand challenge="ABCDEF123456" />);
    expect(out).toContain("celerisctl browser owner-session approve ABCDEF123456 --socket &lt;path&gt;");
  });
});

describe("issueOwnerChallenge", () => {
  it("returns the challenge on success", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ challenge: "AABBCCDDEEFF" })),
    );
    await expect(issueOwnerChallenge()).resolves.toEqual({ challenge: "AABBCCDDEEFF" });
  });

  it("maps a gateway error code to a fixed message instead of echoing the response", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => Response.json({ code: "csrf_failed" }, { status: 403 })),
    );
    await expect(issueOwnerChallenge()).resolves.toEqual({
      error: "送信元を確認できませんでした。画面を読み込み直してください。",
    });
  });
});

describe("OwnerSessionNotice", () => {
  it("shows the unavailable notice (no button) when owner session is missing", () => {
    const out = renderToStaticMarkup(<OwnerSessionNotice owner={undefined} />);
    expect(out).toContain('data-testid="browser-owner-unavailable"');
    expect(out).not.toContain("<button");
  });

  it("shows the challenge-request notice when registered but not the owner", () => {
    const out = renderToStaticMarkup(<OwnerSessionNotice owner={{ available: true, isOwner: false }} />);
    expect(out).toContain('data-testid="browser-owner-request"');
    expect(out).toContain("本人確認のコードを発行");
    expect(out).not.toContain('data-testid="browser-owner-challenge"');
  });

  it("renders nothing once the session is the registered owner", () => {
    const out = renderToStaticMarkup(<OwnerSessionNotice owner={{ available: true, isOwner: true }} />);
    expect(out).toBe("");
  });
});
