import { describe, expect, it } from "vitest";
import { optionalBoolean, optionalNumber, optionalString } from "./search";

describe("search param parsers", () => {
  it("keeps only meaningful values", () => {
    expect(optionalString("x")).toBe("x");
    expect(optionalString("")).toBeUndefined();
    expect(optionalString(3)).toBe("3");
    expect(optionalString({})).toBeUndefined();
    expect(optionalNumber("2")).toBe(2);
    expect(optionalNumber("a")).toBeUndefined();
    expect(optionalBoolean("1")).toBe(true);
    expect(optionalBoolean("false")).toBe(false);
    expect(optionalBoolean("maybe")).toBeUndefined();
  });
});
