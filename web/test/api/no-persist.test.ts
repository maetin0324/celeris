import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const PERSIST =
  /query-(sync-storage|async-storage)-persister|query-persist-client|persistQueryClient|PersistQueryClient/;

describe("H4: Query を永続化しない", () => {
  it("persist 系の package が依存と lockfile に無い", () => {
    const pkg = readFileSync(new URL("../../package.json", import.meta.url), "utf8");
    const lock = readFileSync(new URL("../../pnpm-lock.yaml", import.meta.url), "utf8");
    for (const text of [pkg, lock]) {
      expect(text).not.toMatch(PERSIST);
    }
  });

  it("api/ と main.tsx が persist の API を使っていない", () => {
    for (const file of ["api/query-client.ts", "api/provider.tsx", "main.tsx"]) {
      expect(readFileSync(new URL(`../../${file}`, import.meta.url), "utf8")).not.toMatch(PERSIST);
    }
  });
});
