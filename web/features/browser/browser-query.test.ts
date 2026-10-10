import { afterEach, describe, expect, it, vi } from "vitest";
import {
  BrowserActionGate,
  BrowserGatewayError,
  browserControlQuery,
  browserRunsQuery,
  sendControl,
} from "./browser-query";

afterEach(() => vi.unstubAllGlobals());

describe("browser gateway requests", () => {
  it("rejects a second mutation while the first is unresolved", async () => {
    const gate = new BrowserActionGate();
    let finish!: (value: number) => void;
    const first = gate.run(() => new Promise<number>((resolve) => (finish = resolve)));
    await expect(gate.run(async () => 2)).rejects.toThrow("browser_action_pending");
    finish(1);
    await expect(first).resolves.toBe(1);
    expect(gate.busy).toBe(false);
  });

  it("sends CSRF, current version and a UUID through the control gateway", async () => {
    const fetcher = vi.fn(async () => Response.json({ ok: true, status: { phase: "paused", version: 2 } }));
    vi.stubGlobal("fetch", fetcher);
    await sendControl({ taskId: "T1", runId: "R1", sessionId: "S1" }, { kind: "pause" }, 1, "csrf-token");
    const [path, init] = fetcher.mock.calls[0] as unknown as [string, RequestInit];
    expect(path).toBe("/browser/control/T1/R1/S1");
    expect(init.method).toBe("POST");
    expect(init.credentials).toBe("same-origin");
    expect(JSON.parse(String(init.body))).toMatchObject({
      csrf: "csrf-token",
      expected_version: 1,
      command: { kind: "pause" },
      idempotency_key: expect.stringMatching(/^[0-9a-f-]{36}$/),
    });
  });

  it("never builds a browser run URL from a path-like task id", () => {
    expect(() => browserRunsQuery("../other")).toThrow(TypeError);
  });

  it("stops control polling for every non-running run and 404 responses", () => {
    const terminal = browserControlQuery("T1", "R1", "S1", "COMPLETED").refetchInterval;
    const active = browserControlQuery("T1", "R1", "S1", "RUNNING").refetchInterval;
    expect(typeof terminal).toBe("function");
    expect((terminal as (query: { state: { error: unknown } }) => number | false)({ state: { error: null } })).toBe(
      false,
    );
    for (const state of ["WAITING_FOR_AUTH", "WAITING_FOR_HUMAN", "WAITING_FOR_APPROVAL", "FAILED", "other"]) {
      const interval = browserControlQuery("T1", "R1", "S1", state).refetchInterval;
      expect((interval as (query: { state: { error: unknown } }) => number | false)({ state: { error: null } })).toBe(
        false,
      );
    }
    expect((active as (query: { state: { error: unknown } }) => number | false)({ state: { error: null } })).toBe(2000);
    expect(
      (active as (query: { state: { error: unknown } }) => number | false)({
        state: { error: new BrowserGatewayError(404, "not_found") },
      }),
    ).toBe(false);
  });
});
