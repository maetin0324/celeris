import { beforeEach, describe, expect, it, vi } from "vitest";
import { PREFERENCES_KEY, readPreferences, sanitizePreferences, writePreferences } from "./preferences";

describe("preferences", () => {
  beforeEach(() => {
    const map = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (k: string) => map.get(k) ?? null,
      setItem: (k: string, v: string) => void map.set(k, v),
      removeItem: (k: string) => void map.delete(k),
      key: (i: number) => [...map.keys()][i] ?? null,
      get length() {
        return map.size;
      },
    });
  });

  it("drops unknown fields and invalid values", () => {
    expect(sanitizePreferences({ timeZone: "Asia/Tokyo", theme: "dark", token: "x", body: "y" })).toEqual({
      timeZone: "Asia/Tokyo",
      theme: "dark",
    });
    expect(sanitizePreferences({ timeZone: "Not/AZone", theme: "neon" })).toEqual({});
    expect(sanitizePreferences([1, 2])).toEqual({});
    expect(sanitizePreferences(null)).toEqual({});
  });

  it("stores only allowed fields under a single key", () => {
    writePreferences({ timeZone: "America/New_York", theme: "light", extra: "no" } as never);
    expect(localStorage.length).toBe(1);
    expect(localStorage.key(0)).toBe(PREFERENCES_KEY);
    expect(JSON.parse(localStorage.getItem(PREFERENCES_KEY) ?? "")).toEqual({
      timeZone: "America/New_York",
      theme: "light",
    });
  });

  it("validates on read", () => {
    localStorage.setItem(PREFERENCES_KEY, JSON.stringify({ theme: "dark", secret: "s", timeZone: 5 }));
    expect(readPreferences()).toEqual({ theme: "dark" });
    localStorage.setItem(PREFERENCES_KEY, "{broken");
    expect(readPreferences()).toEqual({});
  });
});
