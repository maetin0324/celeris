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
  assert.deepEqual(r.failures[0].fields, ["commands", "output"]);
});

test("cos_digest: a dropped requeue_run is a mismatch", async () => {
  const row = JSON.parse(digestLines.find((l) => JSON.parse(l).commands.length === 2));
  row.commands.pop();
  const file = path.join(work, "digest-bad.jsonl");
  writeFileSync(file, `${JSON.stringify(row)}\n`);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.deepEqual(r.failures[0].fields, ["commands", "output"]);
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
  assert.deepEqual(r.failures[0].fields, ["commands", "output"]);
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

// pass trace（付記 S4-16）。期待値は手書きせず、Rust が書いた cases.jsonl の行（commands の生の byte 列）から組む。
const caseRows = digestLines.map((l) => ({ line: l, row: JSON.parse(l) }));
const rawCommands = (c) => c.line.slice(c.line.indexOf(',"commands":') + ',"commands":'.length, -1);
const ref = (input) => ({ thread: input.thread, run_id: input.run_id, run_state: input.run_state });
const listOp = (...inputs) => ({ op: "cos_digest:list_runs", request: {}, reply: { kind: "ok", value: { runs: inputs.map(ref) } } });
const readOp = (input, reply = { kind: "ok", value: input }) => ({
  op: "cos_digest:read_run",
  request: { thread: input.thread, run_id: input.run_id },
  reply,
});
const passLine = (name, ops, cases) => {
  const runs = cases.map((c) => `{"run_id":${JSON.stringify(c.row.input.run_id)},"commands":${rawCommands(c)}}`);
  return `${JSON.stringify({ name, ops })}`.slice(0, -1) + `,"output":{"runs":[${runs.join(",")}]}}`;
};
// cases.jsonl の run_id は全行で同じなので、2 つ目の run は行の run_id を機械的に置き換えて作る
// （decide の出力は run_id を冪等 key・文面・欄にそのまま埋めるだけなので、置き換えた行も Rust の出力と同じ）。
const withRunId = (c, runId) => {
  const line = c.line.split(c.row.input.run_id).join(runId);
  return { line, row: JSON.parse(line) };
};
const twoRuns = () => {
  const a = caseRows.find((c) => c.row.commands.length === 2);
  const b = withRunId(caseRows.find((c) => c.row.commands.length === 1), "01M6PASSREPLAY0000000000RB");
  assert.notEqual(a.row.input.run_id, b.row.input.run_id);
  return [a, b];
};
const writePasses = (name, lines) => {
  const file = path.join(work, `${name}.jsonl`);
  writeFileSync(file, `${lines.join("\n")}\n`);
  return file;
};

test("cos_digest: a pass trace with two runs replays through the fake host in both orders", async () => {
  const [a, b] = twoRuns();
  const file = writePasses("pass-two", [
    passLine("two", [listOp(a.row.input, b.row.input), readOp(a.row.input), readOp(b.row.input)], [a, b]),
    passLine("empty", [listOp()], []),
  ]);
  const r = await compare("cos_digest", { fixture: file });
  assert.deepEqual(r.failures, []);
  assert.equal(r.matched, 2);
});

test("cos_digest: a run whose read_run fails with exception is skipped, as the Rust pass does", async () => {
  const [a, b] = twoRuns();
  const failed = { kind: "error", error: { code: "exception", message: "store read failed" } };
  const file = writePasses("pass-skip", [
    passLine("skip", [listOp(a.row.input, b.row.input), readOp(a.row.input, failed), readOp(b.row.input)], [b]),
  ]);
  const r = await compare("cos_digest", { fixture: file });
  assert.deepEqual(r.failures, []);
  assert.equal(r.matched, 1);
});

test("cos_digest: a read_run request that differs from the record is a mismatch", async () => {
  const [a] = twoRuns();
  const op = readOp(a.row.input);
  op.request = { thread: `${a.row.input.thread}x`, run_id: a.row.input.run_id };
  const file = writePasses("pass-req", [passLine("req", [listOp(a.row.input), op], [a])]);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.ok(r.failures[0].problems.some((p) => p.startsWith("unrecorded op: cos_digest:read_run")));
  assert.ok(r.failures[0].problems.some((p) => p.startsWith("recorded op not called: cos_digest:read_run")));
});

test("cos_digest: a recorded op that the pass never calls is a mismatch", async () => {
  const [a, b] = twoRuns();
  // b は一覧に無いので TS は読まない（S4-13）。記録に残る read は呼ばれず、不一致になる。
  const file = writePasses("pass-unused", [passLine("unused", [listOp(a.row.input), readOp(a.row.input), readOp(b.row.input)], [a])]);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.deepEqual(r.failures[0].fields, []);
  assert.deepEqual(r.failures[0].problems, [`recorded op not called: cos_digest:read_run ${JSON.stringify(readOp(b.row.input).request)}`]);
});

test("cos_digest: a denied list_runs propagates as a TS error", async () => {
  const file = writePasses("pass-denied", [
    passLine("denied", [{ op: "cos_digest:list_runs", request: {}, reply: { kind: "error", error: { code: "denied", message: "no" } } }], []),
  ]);
  const r = await compare("cos_digest", { fixture: file });
  assert.equal(r.matched, 0);
  assert.equal(r.failures[0].error, "no");
});

test("cos_digest: a copy of the TS with one requeue rule broken does not match the fixture", async () => {
  const src = readFileSync(path.join(pkg, "src/domains/cos_digest.ts"), "utf8");
  const broken = src.replace('return runState === "stopped" || runState === "interrupted";', 'return runState === "stopped";');
  assert.notEqual(broken, src);
  const module = path.join(work, "cos_digest_broken.ts");
  writeFileSync(module, broken);
  const r = await compare("cos_digest", { module });
  assert.ok(r.matched < r.total);
  assert.ok(r.failures.length > 0);
});

test("cos_digest: the CLI exits non-zero when a pass trace does not match", () => {
  const [a] = twoRuns();
  const file = writePasses("pass-cli", [passLine("ok", [listOp(a.row.input), readOp(a.row.input)], [a]), passLine("dup", [listOp(a.row.input), readOp(a.row.input), readOp(a.row.input)], [a])]);
  const r = spawnSync(process.execPath, [path.join(pkg, "scripts/compare.mjs"), "cos_digest", "--fixture", file], {
    encoding: "utf8",
  });
  assert.equal(r.status, 1);
  assert.match(r.stdout, /cos_digest: 1\/2 matched/);
  assert.match(r.stderr, /recorded op not called: cos_digest:read_run/);
});
