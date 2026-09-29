import { execFileSync } from "node:child_process";
import { test } from "@playwright/test";

test("parity-x: 型の再生成差分ゼロ・gui import なし", () => {
  execFileSync(process.execPath, ["scripts/gen-types.mjs", "--check"]);
  execFileSync(process.execPath, ["scripts/check-boundaries.mjs"]);
});
