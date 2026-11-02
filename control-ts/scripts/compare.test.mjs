import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

const routeLines = readFileSync(path.join(pkg, "fixtures/route_priority/cases.jsonl"), "utf8").split("\n");
routeLines.pop();

test("route_priority matches every line of the committed fixture", async () => {
  const r = await compare("route_priority");
  assert.equal(r.total, routeLines.length);
  assert.equal(r.matched, routeLines.length);
  assert.deepEqual(r.failures, []);
});

test("route_priority: a changed candidate order is reported as a mismatch", async () => {
  const row = JSON.parse(routeLines.find((l) => JSON.parse(l).candidates.length >= 2));
  row.candidates.reverse();
  const file = path.join(work, "route-bad.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("route_priority", { fixture: file });
  assert.equal(r.matched, 0);
  assert.deepEqual(r.failures[0].fields, ["candidates"]);
});

test("route_priority: an extra input field is reported as a TS error", async () => {
  const row = JSON.parse(routeLines[0]);
  row.input.extra = 1;
  const file = path.join(work, "route-err.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("route_priority", { fixture: file });
  assert.equal(r.matched, 0);
  assert.match(r.failures[0].error, /unknown field in input: extra/);
});

const digestLines = readFileSync(path.join(pkg, "fixtures/cos_digest/cases.jsonl"), "utf8").split("\n");
digestLines.pop();

test("cos_digest reads every *.jsonl of the fixture dir and matches every line", async () => {
  const r = await compare("cos_digest");
  assert.ok(r.files.includes("fixtures/cos_digest/cases.jsonl"));
  assert.ok(r.total >= digestLines.length);
  assert.equal(r.matched, r.total);
  assert.deepEqual(r.failures, []);
});

test("cos_digest: a later traces.jsonl in the same dir is read the same way", async () => {
  const dir = path.join(work, "digest-dir");
  mkdirSync(dir);
  writeFileSync(path.join(dir, "cases.jsonl"), `${digestLines[0]}\n`);
  const row = JSON.parse(digestLines[4]);
  row.commands[0].text += " ";
  writeFileSync(path.join(dir, "traces.jsonl"), `${digestLines[1]}\n${JSON.stringify(row)}\n`);
  writeFileSync(path.join(dir, "notes.txt"), "not a fixture\n");
  const r = await compare("cos_digest", { fixture: dir });
  assert.deepEqual(r.files.map((f) => path.basename(f)), ["cases.jsonl", "traces.jsonl"]);
  assert.equal(r.total, 3);
  assert.equal(r.matched, 2);
  assert.equal(r.failures.length, 1);
  assert.equal(path.basename(r.failures[0].file), "traces.jsonl");
  assert.equal(r.failures[0].line, 2);
  assert.deepEqual(r.failures[0].fields, ["commands"]);
});

test("cos_digest: a dropped requeue_run is a mismatch", async () => {
  const row = JSON.parse(digestLines.find((l) => JSON.parse(l).commands.length === 2));
  row.commands.pop();
  const file = path.join(work, "digest-bad.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.deepEqual(r.failures[0].fields, ["commands"]);
});

test("cos_digest: an extra input field is reported as a TS error", async () => {
  const row = JSON.parse(digestLines[0]);
  row.input.items[0].extra = 1;
  const file = path.join(work, "digest-err.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.match(r.failures[0].error, /unknown field in input.items\[0\]: extra/);
});

test("cos_digest: a fixture line whose commands are not in canonical bytes is a mismatch", async () => {
  // 同じ値でも Rust が書いた byte 列（余白なし）と違えば一致にしない。
  const row = JSON.parse(digestLines[0]);
  const line = `${JSON.stringify({ name: row.name, input: row.input }).slice(0, -1)},"commands": ${JSON.stringify(row.commands)}}`;
  const file = path.join(work, "digest-spaced.jsonl");
  writeFileSync(file, `${line}\n`);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.deepEqual(r.failures[0].fields, ["commands"]);
});

test("cos_digest: the CLI exits non-zero and prints the first mismatches", () => {
  const row = JSON.parse(digestLines[0]);
  row.commands[0].run_id = "other";
  const file = path.join(work, "digest-cli.jsonl");
  writeFileSync(file, `${digestLines[1]}\n${JSON.stringify(row)}\n`);
  const r = spawnSync(process.execPath, [path.join(pkg, "scripts/compare.mjs"), "cos_digest", "--fixture", file], {
    encoding: "utf8",
  });
  assert.equal(r.status, 1);
  assert.match(r.stdout, /cos_digest: 1\/2 matched/);
  assert.match(r.stderr, /1 mismatched; first 1:/);
});
