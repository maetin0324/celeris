import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("~/browser-owner.server", () => ({
  checkOwner: vi.fn(async () => ({ ok: true, config: {}, sessionHash: "owner" })),
  exactSameOrigin: vi.fn(() => true),
  verifyOwnerCsrfToken: vi.fn(() => true),
}));
vi.mock("~/browser-attestation.server", () => ({
  signLiveAssertion: vi.fn(() => "assertion"),
}));
vi.mock("~/celeris/browser", () => ({
  loadBrowserRuns: vi.fn(async () => [{ task_id: "t1", run_id: "r1", session_id: "s1" }]),
}));

import { runControlRequest } from "~/celeris/browser-control.server";
import type { CelerisClient } from "~/celeris/client.server";
import { CelerisError } from "~/celeris/errors";

const ids = { taskId: "t1", runId: "r1", sessionId: "s1" };

function status(authSection: boolean) {
  return { phase: "paused", version: 3, lease_expires_at: null, in_flight: 0, auth_section: authSection };
}

function fakeClient(authSection: boolean, post?: () => Promise<unknown>) {
  const posted: unknown[] = [];
  const client = {
    get: vi.fn(async () => status(authSection)),
    post: vi.fn(async (_path: string, body: unknown) => {
      posted.push(body);
      return post ? post() : status(authSection);
    }),
  } as unknown as CelerisClient;
  return { client, posted };
}

function post(kind: "takeover" | "renew") {
  return new Request("https://gui.example/tasks/t1/browser/control/r1/s1", {
    method: "POST",
    headers: { "content-type": "application/json", origin: "https://gui.example" },
    body: JSON.stringify({ csrf: "c", command: { kind }, expected_version: 3, idempotency_key: "k1" }),
  });
}

describe("browser-control auth_section (ADR-0080 H3 / ADR-0081)", () => {
  beforeEach(() => vi.clearAllMocks());

  for (const kind of ["takeover", "renew"] as const) {
    it(`refuses ${kind} while auth_section is true without forwarding the command`, async () => {
      const { client, posted } = fakeClient(true);
      const res = await runControlRequest(client, post(kind), ids);
      expect(res.status).toBe(409);
      expect(await res.json()).toEqual({ ok: false, code: "auth_section_active" });
      expect(posted).toHaveLength(0);
    });
  }

  it("maps the task-api auth_section_active refusal (race after the GET) to 409", async () => {
    const { client } = fakeClient(false, async () => {
      throw new CelerisError({ status: 409, code: "auth_section_active", detail: "auth section" });
    });
    const res = await runControlRequest(client, post("takeover"), ids);
    expect(res.status).toBe(409);
    expect(await res.json()).toEqual({ ok: false, code: "auth_section_active" });
  });

  it("forwards takeover once auth_section is false", async () => {
    const { client, posted } = fakeClient(false);
    const res = await runControlRequest(client, post("takeover"), ids);
    expect(res.status).toBe(200);
    expect(posted).toHaveLength(1);
  });
});
