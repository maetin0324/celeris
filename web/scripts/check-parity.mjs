import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

export function checkParity(root = defaultRoot, requirePhase = null) {
  const matrix = readFileSync(path.join(root, "docs/web/feature-parity.md"), "utf8");
  const specDir = path.join(root, "web/e2e/parity");
  const specs = readdirSync(specDir)
    .filter((name) => name.endsWith(".spec.ts"))
    .map((name) => readFileSync(path.join(specDir, name), "utf8"))
    .join("\n");
  const errors = [];
  const rows = matrix.split("\n").filter((line) => /^\| (?:R\d\d|X\d+) \|/.test(line));
  if (rows.filter((line) => line.startsWith("| R")).length !== 42) errors.push("expected 42 route rows");
  for (const row of rows) {
    const cells = row
      .split("|")
      .slice(1, -1)
      .map((cell) => cell.trim());
    const id = cells[0];
    const phaseMatch = cells.at(-3)?.match(/^(\d+)\s*\//);
    const title = cells.at(-2)?.match(/`((?:parity|parity-x): [^`]+)`/)?.[1];
    const done = /^完了（[0-9a-f]{7,40}）$/.test(cells.at(-1) ?? "");
    if (requirePhase !== null && phaseMatch && Number(phaseMatch[1]) <= requirePhase && !done)
      errors.push(`${id}: phase ${requirePhase} requires completion`);
    if (done && (!title || !specs.includes(title))) errors.push(`${id}: missing parity test title`);
  }
  return errors;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const index = process.argv.indexOf("--require-phase");
  const phase = index < 0 ? null : Number(process.argv[index + 1]);
  if (index >= 0 && (!Number.isInteger(phase) || phase < 1 || phase > 6)) throw new Error("--require-phase needs 1..6");
  const errors = checkParity(defaultRoot, phase);
  if (errors.length) {
    console.error(errors.join("\n"));
    process.exitCode = 1;
  }
}
