import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, expect, it } from "vitest";
// @ts-expect-error Local .mjs checker has no declaration.
import { checkBoundaries } from "../../scripts/check-boundaries.mjs";

let root = "";
afterEach(() => {
  if (root) rmSync(root, { recursive: true, force: true });
});
function fixture(file: string, source: string) {
  root = mkdtempSync(path.join(tmpdir(), "web-boundaries-"));
  const target = path.join(root, file);
  mkdirSync(path.dirname(target), { recursive: true });
  writeFileSync(target, source);
  return checkBoundaries(root);
}

it.each([
  ["gui import", "main.tsx", 'import x from "../gui/app/root";', "gui import"],
  ["server import", "main.tsx", 'import x from "./server/token";', "client imports server"],
  [
    "blocking loader",
    "routes/tasks.tsx",
    'export const Route = createFileRoute("/tasks")({ loader: async () => { await fetch("/api/v1/tasks"); } });',
    "blocking loader",
  ],
  [
    "blocking beforeLoad",
    "routes/tasks.tsx",
    'export const Route = createFileRoute("/tasks")({ beforeLoad: async () => { return fetch("/api/v1/tasks"); } });',
    "blocking loader",
  ],
  ["generation name", "routes/next.tsx", "export const x = 1;", "generation name"],
  ["large route", "routes/tasks.tsx", `${"const x = 1;\n".repeat(151)}`, "150 lines"],
])("detects %s", (_name, file, source, expected) => {
  expect(fixture(file, source).join("\n")).toContain(expected);
});
