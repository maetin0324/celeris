import { existsSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { cleanupTmpDirs, makeTmpDir, removeTmpDir } from "./tmp-dir";

describe("tmp-dir helper", () => {
  it("creates a dir under tmpdir with the prefix and removes it", () => {
    const dir = makeTmpDir("celeris-tmpdir-test-");
    expect(path.dirname(dir)).toBe(path.resolve(tmpdir()));
    expect(path.basename(dir).startsWith("celeris-tmpdir-test-")).toBe(true);
    writeFileSync(path.join(dir, "f"), "x");
    removeTmpDir(dir);
    expect(existsSync(dir)).toBe(false);
  });

  it("cleanupTmpDirs removes every registered dir", () => {
    const a = makeTmpDir("celeris-tmpdir-test-");
    const b = makeTmpDir("celeris-tmpdir-test-");
    expect(a).not.toBe(b);
    cleanupTmpDirs();
    expect(existsSync(a)).toBe(false);
    expect(existsSync(b)).toBe(false);
  });
});
