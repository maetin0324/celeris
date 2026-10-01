import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, it } from "vitest";
// @ts-expect-error Local .mjs checker has no declaration.
import { checkParity } from "../../scripts/check-parity.mjs";

const okGit = () => "";

function makeGit(options: { available?: boolean; existingShas?: Set<string>; ancestorShas?: Set<string> }) {
  return (args: string[]) => {
    if (options.available === false) throw new Error("spawnSync git ENOENT");
    if (args[0] === "rev-parse") return "";
    if (args[0] === "cat-file") {
      const sha = String(args[2]).replace(/\^\{commit\}$/, "");
      if (!options.existingShas?.has(sha)) throw new Error(`fatal: Not a valid object name ${sha}`);
      return "";
    }
    if (args[0] === "merge-base") {
      const sha = args[2];
      if (!options.ancestorShas?.has(String(sha))) throw new Error("fatal: Not an ancestor");
      return "";
    }
    throw new Error(`unexpected git args: ${args.join(" ")}`);
  };
}

it("requires completed rows to have a matching parity title and phase completion", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/support"), { recursive: true });
    mkdirSync(path.join(root, "web/server"), { recursive: true });
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    writeFileSync(path.join(root, "web/e2e/support/screens.ts"), 'export const screens = [{ path: "/tasks" }];');
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
    expect(checkParity(root, null, okGit)).toEqual([]);
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks", "/inbox"];');
    expect(checkParity(root, null, okGit)).toContain("/inbox: missing V3 screen");
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    expect(checkParity(root, 1, okGit)).toContain("R01: phase 1 requires completion");
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), "");
    expect(checkParity(root, null, okGit)).toContain("X16: missing parity test title");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

it("rejects a completed commit that does not exist", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/support"), { recursive: true });
    mkdirSync(path.join(root, "web/server"), { recursive: true });
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    writeFileSync(path.join(root, "web/e2e/support/screens.ts"), 'export const screens = [{ path: "/tasks" }];');
    const routes = Array.from(
      { length: 42 },
      (_, index) =>
        `| R${String(index + 1).padStart(2, "0")} | /route | 1 / gateway | \`parity: route ${index}\` | 未着手 |`,
    );
    writeFileSync(
      path.join(root, "docs/web/feature-parity.md"),
      `${routes.join("\n")}\n| X16 | 型 | 1 / gateway | \`parity-x: 型検査\` | 完了（1111111） |\n`,
    );
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), 'test("parity-x: 型検査", () => {});');
    const git = makeGit({ existingShas: new Set(), ancestorShas: new Set() });
    expect(checkParity(root, null, git)).toContain("X16: commit 1111111 not found");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

it("rejects a completed commit that is not an ancestor of HEAD", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/support"), { recursive: true });
    mkdirSync(path.join(root, "web/server"), { recursive: true });
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    writeFileSync(path.join(root, "web/e2e/support/screens.ts"), 'export const screens = [{ path: "/tasks" }];');
    const routes = Array.from(
      { length: 42 },
      (_, index) =>
        `| R${String(index + 1).padStart(2, "0")} | /route | 1 / gateway | \`parity: route ${index}\` | 未着手 |`,
    );
    writeFileSync(
      path.join(root, "docs/web/feature-parity.md"),
      `${routes.join("\n")}\n| X16 | 型 | 1 / gateway | \`parity-x: 型検査\` | 完了（2222222） |\n`,
    );
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), 'test("parity-x: 型検査", () => {});');
    const git = makeGit({ existingShas: new Set(["2222222"]), ancestorShas: new Set() });
    expect(checkParity(root, null, git)).toContain("X16: commit 2222222 is not an ancestor of HEAD");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

it("accepts a completed commit that exists and is an ancestor of HEAD", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/support"), { recursive: true });
    mkdirSync(path.join(root, "web/server"), { recursive: true });
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    writeFileSync(path.join(root, "web/e2e/support/screens.ts"), 'export const screens = [{ path: "/tasks" }];');
    const routes = Array.from(
      { length: 42 },
      (_, index) =>
        `| R${String(index + 1).padStart(2, "0")} | /route | 1 / gateway | \`parity: route ${index}\` | 未着手 |`,
    );
    writeFileSync(
      path.join(root, "docs/web/feature-parity.md"),
      `${routes.join("\n")}\n| X16 | 型 | 1 / gateway | \`parity-x: 型検査\` | 完了（3333333） |\n`,
    );
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), 'test("parity-x: 型検査", () => {});');
    const git = makeGit({ existingShas: new Set(["3333333"]), ancestorShas: new Set(["3333333"]) });
    expect(checkParity(root, null, git)).toEqual([]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

it("fails with an explicit error when git cannot be run", () => {
  const root = mkdtempSync(path.join(tmpdir(), "web-parity-"));
  try {
    mkdirSync(path.join(root, "docs/web"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/parity"), { recursive: true });
    mkdirSync(path.join(root, "web/e2e/support"), { recursive: true });
    mkdirSync(path.join(root, "web/server"), { recursive: true });
    writeFileSync(path.join(root, "web/server/spa-routes.js"), 'export const spaRoutePatterns = ["/tasks"];');
    writeFileSync(path.join(root, "web/e2e/support/screens.ts"), 'export const screens = [{ path: "/tasks" }];');
    const routes = Array.from(
      { length: 42 },
      (_, index) =>
        `| R${String(index + 1).padStart(2, "0")} | /route | 1 / gateway | \`parity: route ${index}\` | 未着手 |`,
    );
    writeFileSync(
      path.join(root, "docs/web/feature-parity.md"),
      `${routes.join("\n")}\n| X16 | 型 | 1 / gateway | \`parity-x: 型検査\` | 完了（4444444） |\n`,
    );
    writeFileSync(path.join(root, "web/e2e/parity/gateway.spec.ts"), 'test("parity-x: 型検査", () => {});');
    const git = makeGit({ available: false });
    expect(checkParity(root, null, git)).toEqual(["git が使えないため commit の到達性を検査できない"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
