import { afterEach, describe, expect, it, vi } from "vitest";
import { randomId } from "./random-id";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

describe("randomId", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("returns a v4 UUID", () => {
    expect(randomId()).toMatch(UUID);
  });

  it("works without crypto.randomUUID (insecure context such as LAN http)", () => {
    const real = globalThis.crypto;
    vi.stubGlobal("crypto", { getRandomValues: real.getRandomValues.bind(real) });
    vi.stubGlobal("isSecureContext", false);
    const a = randomId();
    const b = randomId();
    expect(a).toMatch(UUID);
    expect(a).not.toBe(b);
  });
});
