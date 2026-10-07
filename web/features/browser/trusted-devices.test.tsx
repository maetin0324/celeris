import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BrowserGatewayError,
  fetchOwnerSession,
  registerTrustedDevice,
  resetOwnerResumeAttempt,
  revokeTrustedDevice,
  type TrustedDevice,
} from "./browser-query";
import { formatIdentityExpiry } from "./identities-screen";
import { OwnerSessionNotice, resumeErrorMessage } from "./owner-session-notice";
import {
  defaultDeviceName,
  TrustDeviceForm,
  TrustedDeviceOffer,
  TrustedDevicesPanel,
  trustDeviceMode,
  trustedDeviceErrorMessage,
  trustedDeviceState,
  trustedDeviceStateLabel,
} from "./trusted-devices";

// ADR 2026-10-07-browser-trusted-devices: 自動復帰（owner でなく端末 cookie があれば resume を 1 度だけ）、
// 登録・失効の要求、一覧の状態の判定。

const ID = "01K00000000000000000000001";
const device = (patch: Partial<TrustedDevice> = {}): TrustedDevice => ({
  id: ID,
  name: "手元のノート",
  method: "cookie",
  created_at: 1_790_000_000,
  last_used_at: null,
  expires_at: 1_790_000_000 + 90 * 86400,
  absolute_expires_at: null,
  revoked_at: null,
  revoked_reason: null,
  ...patch,
});
const notOwner = { available: true, isOwner: false, csrfToken: null, trustedDevice: true, resumable: true };
const owner = {
  available: true,
  isOwner: true,
  csrfToken: "csrf",
  trustedDevice: true,
  resumable: false,
  deviceId: ID,
};

function stubGateway(replies: Array<[string, number, unknown]>) {
  const calls: Array<[string, string]> = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init: RequestInit) => {
      calls.push([init.method ?? "GET", path]);
      const next = replies.shift();
      if (!next || next[0] !== `${init.method ?? "GET"} ${path}`) throw new Error(`unexpected ${init.method} ${path}`);
      return Response.json(next[2], { status: next[1] });
    }),
  );
  return calls;
}

beforeEach(() => resetOwnerResumeAttempt());
afterEach(() => vi.unstubAllGlobals());

describe("trusted_device: automatic resume", () => {
  it("resumes once with the device cookie and returns the owner session", async () => {
    const calls = stubGateway([
      ["GET /browser/owner-session", 200, notOwner],
      ["POST /browser/owner-session/resume", 200, { ok: true, isOwner: true, csrfToken: "csrf", deviceId: ID }],
      ["GET /browser/owner-session", 200, owner],
    ]);
    await expect(fetchOwnerSession()).resolves.toMatchObject({ isOwner: true, deviceId: ID });
    expect(calls.map(([m]) => m)).toEqual(["GET", "POST", "GET"]);
  });

  it("does not call resume for an owner or a session without a device cookie", async () => {
    const calls = stubGateway([
      ["GET /browser/owner-session", 200, owner],
      ["GET /browser/owner-session", 200, { ...notOwner, trustedDevice: false, resumable: false }],
    ]);
    await fetchOwnerSession();
    await expect(fetchOwnerSession()).resolves.toMatchObject({ isOwner: false });
    expect(calls.every(([m]) => m === "GET")).toBe(true);
  });

  it("marks a rejected resume and does not retry it until the page reloads", async () => {
    const calls = stubGateway([
      ["GET /browser/owner-session", 200, notOwner],
      ["POST /browser/owner-session/resume", 403, { code: "device_rejected" }],
      ["GET /browser/owner-session", 200, { ...notOwner, trustedDevice: false, resumable: false }],
      ["GET /browser/owner-session", 200, notOwner],
    ]);
    await expect(fetchOwnerSession()).resolves.toMatchObject({ isOwner: false, resumeError: "device_rejected" });
    await expect(fetchOwnerSession()).resolves.not.toHaveProperty("resumeError");
    expect(calls.filter(([m]) => m === "POST")).toHaveLength(1);
  });

  it("shares one in-flight resume between concurrent readers", async () => {
    const calls = stubGateway([
      ["GET /browser/owner-session", 200, notOwner],
      ["GET /browser/owner-session", 200, notOwner],
      ["POST /browser/owner-session/resume", 200, { ok: true, isOwner: true, csrfToken: "csrf", deviceId: ID }],
      ["GET /browser/owner-session", 200, owner],
    ]);
    const [a, b] = await Promise.all([fetchOwnerSession(), fetchOwnerSession()]);
    expect(a.isOwner && b.isOwner).toBe(true);
    expect(calls.filter(([m]) => m === "POST")).toHaveLength(1);
  });
});

describe("trusted_device: register and revoke requests", () => {
  it("registers this device with a name and CSRF through the owner-session path", async () => {
    const fetcher = vi.fn(async () => Response.json({ ok: true, device: device() }, { status: 201 }));
    vi.stubGlobal("fetch", fetcher);
    await registerTrustedDevice("手元のノート", "csrf-token");
    const [path, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/browser/owner-session/device");
    expect(init.method).toBe("POST");
    expect(JSON.parse(String(init.body))).toEqual({ name: "手元のノート", csrf: "csrf-token" });
  });

  it("revokes by id and refuses a path-like id", async () => {
    const fetcher = vi.fn(async () => Response.json({ ok: true, revoked: true, device: device() }));
    vi.stubGlobal("fetch", fetcher);
    await revokeTrustedDevice(ID, "csrf-token");
    expect((fetcher.mock.calls[0] as unknown as [string])[0]).toBe(`/browser/trusted-devices/${ID}/revoke`);
    expect(() => revokeTrustedDevice("../x", "csrf-token")).toThrow(TypeError);
  });

  it("maps the gateway codes to fixed messages", () => {
    expect(trustedDeviceErrorMessage(new BrowserGatewayError(409, "device_limit"))).toContain("上限");
    expect(trustedDeviceErrorMessage(new BrowserGatewayError(500, "whatever"))).toBe("操作に失敗しました。");
  });
});

describe("trusted_device: list states", () => {
  const now = 1_790_000_000 + 10;
  it("orders revoked before expired and labels reuse", () => {
    expect(trustedDeviceState(device(), now)).toBe("active");
    expect(trustedDeviceState(device({ expires_at: now }), now)).toBe("expired");
    expect(trustedDeviceState(device({ absolute_expires_at: now - 1 }), now)).toBe("expired");
    expect(trustedDeviceState(device({ expires_at: now - 1, revoked_at: now - 5 }), now)).toBe("revoked");
    expect(trustedDeviceStateLabel(device({ revoked_at: now, revoked_reason: "reuse" }), now)).toContain("使い回し");
  });

  it("renders name, dates, state and the revoke button only for active devices", () => {
    const html = renderToStaticMarkup(
      <TrustedDevicesPanel
        devices={[device(), device({ id: "01K00000000000000000000002", name: "古い端末", revoked_at: now })]}
        now={now}
        limit={5}
        currentDeviceId={ID}
        csrf="csrf"
      />,
    );
    expect(html).toContain("手元のノート");
    expect(html).toContain("この端末");
    expect(html).toContain(formatIdentityExpiry(1_790_000_000));
    expect(html).toContain("未使用");
    expect(html).toContain("失効済み");
    expect(html).toContain("有効な端末 1 台（上限 5 台）");
    expect(html.match(/を失効/g)).toHaveLength(1);
  });

  it("offers the trust form only to a CLI-approved owner without a device cookie", () => {
    const cli = { ...owner, trustedDevice: false, deviceId: null };
    expect(trustDeviceMode(cli)).toBe("form");
    expect(trustDeviceMode(owner)).toBe("trusted");
    expect(trustDeviceMode({ ...cli, trustedDevice: true })).toBe("trusted");
    expect(trustDeviceMode(notOwner)).toBeNull();
    expect(renderToStaticMarkup(<TrustedDeviceOffer owner={notOwner} />)).toBe("");
    const form = renderToStaticMarkup(<TrustDeviceForm csrf="csrf" />);
    expect(form).toContain("端末の名前");
    expect(form).toContain("この端末を信頼する");
  });

  it("suggests a device name from the user agent", () => {
    expect(defaultDeviceName("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit Version/17 Safari/605")).toBe(
      "Mac の Safari",
    );
    expect(defaultDeviceName("")).toBe("端末");
  });

  it("explains a failed automatic resume in the owner notice", () => {
    expect(resumeErrorMessage(undefined)).toBeNull();
    const html = renderToStaticMarkup(
      <OwnerSessionNotice owner={{ available: true, isOwner: false, resumeError: "device_rejected" }} />,
    );
    expect(html).toContain("登録した端末として確認できませんでした");
  });
});
