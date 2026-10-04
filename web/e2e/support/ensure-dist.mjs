// playwright の webServer が preview の前に呼ぶ。dist/ が無いか、build の入力が dist/index.html より新しいときだけ
// vite build する。検査が `pnpm build && pnpm e2e` と先に build していれば、ここでは build しない（二重 build をしない）。
import { execFileSync } from "node:child_process";
import { readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
// build の入力ではないもの（試験・gateway・台本・生成物・依存）は見ない。
const IGNORED = new Set([
  "node_modules",
  "dist",
  "release",
  "test-results",
  "playwright-report",
  "e2e",
  "server",
  "scripts",
  "deploy",
  "test",
  "playwright.config.ts",
  "vitest.config.ts",
  "biome.json",
]);

function newestInput(dir) {
  let newest = { mtime: 0, file: "" };
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") || entry.name.endsWith(".tsbuildinfo") || entry.name.endsWith(".md")) continue;
    if (dir === webRoot && IGNORED.has(entry.name)) continue;
    const file = path.join(dir, entry.name);
    const candidate = entry.isDirectory() ? newestInput(file) : { mtime: statSync(file).mtimeMs, file };
    if (candidate.mtime > newest.mtime) newest = candidate;
  }
  return newest;
}

function builtAt() {
  try {
    return statSync(path.join(webRoot, "dist", "index.html")).mtimeMs;
  } catch {
    return 0;
  }
}

const built = builtAt();
const input = newestInput(webRoot);
if (built === 0 || input.mtime > built) {
  process.stderr.write(
    built === 0
      ? "e2e: dist/ が無いので build する\n"
      : `e2e: ${path.relative(webRoot, input.file)} が dist/ より新しいので build する\n`,
  );
  execFileSync(path.join(webRoot, "node_modules", ".bin", "vite"), ["build"], { cwd: webRoot, stdio: "inherit" });
} else {
  process.stderr.write("e2e: dist/ は最新なので build しない\n");
}
