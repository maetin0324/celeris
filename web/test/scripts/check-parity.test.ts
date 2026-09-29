import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, it } from "vitest";
// @ts-expect-error Local .mjs checker has no declaration.
import { checkParity } from "../../scripts/check-parity.mjs";

it("requires completed rows to have a matching parity title and phase completion", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    const routes = Array.from(
      { length: 42 },
      (_, index) =>
        `| R${String(index + 1).padStart(2, "0")} | /route | 1 / gateway | \`parity: route ${index}\` | 未着手 |`,
    );
    writeFileSync(
      path.join(root, "docs/web/feature-parity.md"),
      `${routes.join("\n")}\n| X16 | 型 | 1 / gateway | \`parity-x: 型検査\` | 完了（abcdef0） |\n`,
    );
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), 'test("parity-x: 型検査", () => {});');
    expect(checkParity(root)).toEqual([]);
    expect(checkParity(root, 1)).toContain("R01: phase 1 requires completion");
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), "");
    expect(checkParity(root)).toContain("X16: missing parity test title");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
