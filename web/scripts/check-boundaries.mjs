import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourceExtensions = /\.[cm]?[jt]sx?$/;

function filesAt(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (["node_modules", "dist", ".git", "api", "test-results", "playwright-report"].includes(entry.name)) return [];
    const name = path.join(dir, entry.name);
    return entry.isDirectory()
      ? filesAt(name)
      : sourceExtensions.test(name) && !name.endsWith("routeTree.gen.ts")
        ? [name]
        : [];
  });
}

export function checkBoundaries(root = defaultRoot) {
  const files = filesAt(root);
  const errors = [];
  const sources = new Map(files.map((file) => [file, readFileSync(file, "utf8")]));
  const imports = new Map();
  for (const [file, source] of sources) {
    const relative = path.relative(root, file).replaceAll(path.sep, "/");
    const found = [
      ...source.matchAll(/(?:\b(?:import|export)\s+(?:[^;]*?\s+from\s+)?|\bimport\s*\()\s*["']([^"']+)["']/gs),
    ].map((match) => match[1]);
    imports.set(file, found);
    if (
      !relative.startsWith("test/") &&
      !relative.startsWith("scripts/") &&
      found.some((name) => /(^|\/)gui(\/|$)/.test(name))
    )
      errors.push(`${relative}: gui import`);
    if (/(^|\/)(?:v2|next)(\/|\.|$)/i.test(relative) && !relative.startsWith("api/generated/"))
      errors.push(`${relative}: generation name`);
    if (relative.startsWith("routes/")) {
      if (source.split(/\r?\n/).length > 150) errors.push(`${relative}: route exceeds 150 lines`);
      if (
        /\b(?:loader|beforeLoad)\s*[:=]\s*(?:async\s*)?(?:\([^)]*\)|[a-zA-Z_$][\w$]*)\s*=>[\s\S]{0,1500}?\b(?:await\s+(?:fetch|\w+\.fetch|\w+\.ensureQueryData)|return\s+(?:fetch|\w+\.fetch|\w+\.ensureQueryData))\s*\(/.test(
          source,
        )
      )
        errors.push(`${relative}: blocking loader/beforeLoad`);
    }
    if (
      !relative.startsWith("server/") &&
      !relative.startsWith("scripts/") &&
      !relative.startsWith("test/") &&
      !relative.startsWith("e2e/") &&
      /\b(?:CELERIS_TOKEN|DAEMON_TOKEN|daemonToken|apiToken)\b/.test(source)
    )
      errors.push(`${relative}: token handling in client`);
  }
  function visit(file, seen = new Set()) {
    if (seen.has(file)) return;
    seen.add(file);
    for (const specifier of imports.get(file) ?? []) {
      if (specifier.startsWith("../") || specifier.startsWith("./")) {
        const resolved = path.resolve(path.dirname(file), specifier);
        if (resolved.includes(`${path.sep}gui${path.sep}`)) errors.push(`${path.relative(root, file)}: gui import`);
        if (resolved.includes(`${path.sep}server${path.sep}`))
          errors.push(`${path.relative(root, file)}: client imports server`);
        const next = [
          resolved,
          ...[".ts", ".tsx", ".js", ".mjs"].map((ext) => resolved + ext),
          path.join(resolved, "index.ts"),
        ].find((item) => sources.has(item));
        if (next) visit(next, seen);
      }
    }
  }
  for (const file of files) {
    const rel = path.relative(root, file).replaceAll(path.sep, "/");
    if (
      !rel.startsWith("server/") &&
      !rel.startsWith("scripts/") &&
      !rel.startsWith("e2e/") &&
      !rel.startsWith("test/") &&
      !rel.startsWith("api/generated/")
    )
      visit(file);
  }
  return [...new Set(errors)];
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = checkBoundaries();
  if (errors.length) {
    console.error(errors.join("\n"));
    process.exitCode = 1;
  }
}
