import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const defaultRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

export function runGitCommand(args, cwd) {
  return execFileSync("git", args, { cwd, stdio: ["ignore", "pipe", "pipe"], encoding: "utf8" });
}

function checkCommitAncestry(doneRows, root, runGit, errors) {
  if (doneRows.length === 0) return;
  try {
    runGit(["rev-parse", "--is-inside-work-tree"], root);
  } catch {
    errors.push("git が使えないため commit の到達性を検査できない");
    return;
  }
  for (const { id, sha } of doneRows) {
    try {
      runGit(["cat-file", "-e", `${sha}^{commit}`], root);
    } catch {
      errors.push(`${id}: commit ${sha} not found`);
      continue;
    }
    try {
      runGit(["merge-base", "--is-ancestor", sha, "HEAD"], root);
    } catch {
      errors.push(`${id}: commit ${sha} is not an ancestor of HEAD`);
    }
  }
}

export function checkParity(root = defaultRoot, requirePhase = null, runGit = runGitCommand) {
  const matrix = readFileSync(path.join(root, "agent-docs/web/feature-parity.md"), "utf8");
  const specDir = path.join(root, "web/e2e/parity");
  const specs = readdirSync(specDir)
    .filter((name) => name.endsWith(".spec.ts"))
    .map((name) => readFileSync(path.join(specDir, name), "utf8"))
    .join("\n");
  const errors = [];
  // V3 台帳は gateway の全画面を列挙する。片方だけ増やしたときは検査を落とす。
  const routeSource = readFileSync(path.join(root, "web/server/spa-routes.js"), "utf8");
  const screenSource = readFileSync(path.join(root, "web/e2e/support/screens.ts"), "utf8");
  const routeBlock = routeSource.match(/spaRoutePatterns\s*=\s*\[([\s\S]*?)\]/)?.[1] ?? "";
  const routes = [...routeBlock.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
  const screens = [...screenSource.matchAll(/\{\s*path:\s*"([^"]+)"/g)].map((match) => match[1]);
  for (const route of routes) if (!screens.includes(route)) errors.push(`${route}: missing V3 screen`);
  for (const screen of screens) if (!routes.includes(screen)) errors.push(`${screen}: undeclared V3 screen`);
  if (new Set(screens).size !== screens.length) errors.push("duplicate V3 screen");
  const rows = matrix.split("\n").filter((line) => /^\| (?:R\d\d|X\d+) \|/.test(line));
  if (rows.filter((line) => line.startsWith("| R")).length !== 42) errors.push("expected 42 route rows");
  const doneRows = [];
  for (const row of rows) {
    const cells = row
      .split("|")
      .slice(1, -1)
      .map((cell) => cell.trim());
    const id = cells[0];
    const phaseMatch = cells.at(-3)?.match(/^(\d+)\s*\//);
    const title = cells.at(-2)?.match(/`((?:parity|parity-x): [^`]+)`/)?.[1];
    const doneMatch = cells.at(-1)?.match(/^完了（([0-9a-f]{7,40})）$/);
    const done = Boolean(doneMatch);
    if (requirePhase !== null && phaseMatch && Number(phaseMatch[1]) <= requirePhase && !done)
      errors.push(`${id}: phase ${requirePhase} requires completion`);
    if (done && (!title || !specs.includes(title))) errors.push(`${id}: missing parity test title`);
    if (done) doneRows.push({ id, sha: doneMatch[1] });
  }
  checkCommitAncestry(doneRows, root, runGit, errors);
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
