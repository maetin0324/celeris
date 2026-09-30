import { describe, expect, it, vi } from "vitest";
import { fetchSession, loginHref, onUnauthenticated, registerProtectedCache, safeNextPath } from "./session";

describe("session boundary", () => {
  it("accepts only same-origin absolute next paths", () => {
    for (const bad of ["//evil.example", "/\\evil", "https://evil.example", "tasks", "", undefined])
      expect(safeNextPath(bad)).toBe("/");
    expect(safeNextPath("/tasks?x=1")).toBe("/tasks?x=1");
    expect(loginHref("/tasks/01A?tab=runs")).toBe("/login?next=%2Ftasks%2F01A%3Ftab%3Druns");
  });

  it("drops protected caches before moving to /login on session loss", () => {
    const cache = new Map([["tasks", [1]]]);
    const unregister = registerProtectedCache(() => cache.clear());
    const go = vi.fn();
    onUnauthenticated("/tasks", go);
    expect(cache.size).toBe(0);
    expect(go).toHaveBeenCalledWith("/login?next=%2Ftasks");
    unregister();
  });

  it("decides only from the gateway session, not daemon health", async () => {
    const calls: string[] = [];
    const fetcher = (async (input: string) => {
      calls.push(input);
      return new Response(JSON.stringify({ authenticated: true, authRequired: true }), { status: 200 });
    }) as typeof fetch;
    expect(await fetchSession(fetcher)).toEqual({ authenticated: true, authRequired: true });
    expect(calls).toEqual(["/api/session"]);
    const down = (async () => new Response("", { status: 502 })) as unknown as typeof fetch;
    expect((await fetchSession(down)).authenticated).toBe(false);
  });
});
