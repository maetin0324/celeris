import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";
import { compare } from "./compare.mjs";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const work = mkdtempSync(path.join(process.env.TMPDIR ?? tmpdir(), "control-ts-compare-"));
after(() => rmSync(work, { recursive: true, force: true }));

const committed = readFileSync(path.join(pkg, "fixtures/lane_rules/all-features.jsonl"), "utf8").split("\n");

test("lane_rules matches every line of the committed fixture", async () => {
  const r = await compare("lane_rules");
  assert.equal(r.total, 19683);
  assert.equal(r.matched, 19683);
  assert.deepEqual(r.failures, []);
});

test("a changed field is reported as a mismatch", async () => {
  const row = JSON.parse(committed[0]);
  row.condition = `${row.condition} `;
  const file = path.join(work, "bad.jsonl");
  writeFileSync(file, `${committed[1]}\n${JSON.stringify(row)}\n`);
  const r = await compare("lane_rules", { fixture: file });
  assert.equal(r.matched, 1);
  assert.equal(r.failures.length, 1);
  assert.equal(r.failures[0].line, 2);
  assert.deepEqual(r.failures[0].fields, ["condition"]);
});

test("a TS error is reported as a failure", async () => {
  const row = JSON.parse(committed[0]);
  row.features.judgment = "extreme";
  const file = path.join(work, "err.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("lane_rules", { fixture: file });
  assert.equal(r.matched, 0);
  assert.match(r.failures[0].error, /unknown level for judgment/);
});

test("an unknown domain is rejected", async () => {
  await assert.rejects(compare("nope"), /unknown domain: nope/);
});
