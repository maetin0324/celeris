// 領域の TS 実装を Rust が書いた fixture の全行に通し、比較欄の byte 一致を確かめる（境界 ADR 付記 S2-2・S4-2・S4-16）。
// 使い方: node scripts/compare.mjs <domain> [--fixture <file か dir>]
// fixture が dir なら、その直下の *.jsonl を名前順にすべて読む。
// 一致件数を出す。不一致・TS の throw が 1 件でもあれば最初の数件を出して exit 1。
// Node 24 の型除去で src/domains/<domain>.ts を直接読む。`celeris:host` は src/testing/host-stub.ts へ解決する。
//
// cos_digest（付記 S4-16）は fixtures/cos_digest/ の行を 2 つの形で読む。
// - `{ name, ops, output }`（pass_traces.jsonl など）: fake host が ops から答え、`cosDigestPass(ctx)` を replay する。
//   出力の byte 一致に加え、要求が記録と違う・記録に無い op を呼ぶ・記録の op を呼ばない、のどれも不一致にする。
// - `{ name, input, commands }`（cases.jsonl）: `decide()` の出力を `commands` と比べ、さらに
//   その run だけを持つ pass に組み直して同じく replay する。両方が一致した行だけを一致に数える。
import { readdirSync, readFileSync, statSync } from "node:fs";
import { registerHooks } from "node:module";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SHOW = 5;
const HOST_STUB = pathToFileURL(path.join(pkg, "src/testing/host-stub.ts")).href;

// daemon の V8 の外では celeris:host の実体が無い。試験の差し替え（vitest.config.mjs の alias と同じ file）へ解決する。
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "celeris:host") return { url: HOST_STUB, shortCircuit: true };
    return next(specifier, context);
  },
});

// 領域ごとに、fixture の 1 行から入力 DTO を作る関数・呼ぶ関数・比べる欄（出力 DTO の宣言順）を書く。
// custom: 行の比較を領域ごとの関数で行う（cos_digest の pass replay）。
const DOMAINS = {
  lane_rules: {
    module: "src/domains/lane_rules.ts",
    entry: "laneRules",
    fixture: "fixtures/lane_rules/all-features.jsonl",
    input: (row) => ({ features: row.features }),
    fields: ["rule_id", "proposed", "condition"],
  },
  route_priority: {
    module: "src/domains/route_priority.ts",
    entry: "routePriority",
    fixture: "fixtures/route_priority/cases.jsonl",
    input: (row) => row.input,
    fields: ["basis", "candidates"],
  },
  execution_gate: {
    module: "src/domains/execution_gate.ts",
    entry: "executionGate",
    fixture: "fixtures/execution_gate/cases.jsonl",
    input: (row) => row.input,
    fields: ["mode", "score", "rule_id", "signals"],
  },
  // 参照実装の無い領域（ADR 2026-11-04-project-controller D7）: 行の不変条件を当てる。
  project_controller: {
    module: "src/domains/project_controller.ts",
    fixture: "fixtures/project_controller/invariants.jsonl",
    custom: compareProjectController,
  },
  cos_digest: {
    module: "src/domains/cos_digest.ts",
    fixture: "fixtures/cos_digest",
    custom: compareCosDigest,
  },
};

const REPLAY_CTX = { token: "01M6EVP9GT07K2G01EMW0E6HNC-replay" };

/** fixture の path（file か dir）から、読む *.jsonl の一覧を返す。 */
function fixtureFiles(file) {
  if (!statSync(file).isDirectory()) return [file];
  const files = readdirSync(file)
    .filter((n) => n.endsWith(".jsonl"))
    .sort()
    .map((n) => path.join(file, n));
  if (files.length === 0) throw new Error(`no *.jsonl in fixture dir: ${file}`);
  return files;
}

/** 欄 `key` が行の最後の欄なら、その値の生の byte 列を返す（Rust が書いた直列化そのもの）。 */
function rawTailValue(line, row, key) {
  const value = JSON.stringify(row[key]);
  const tail = `${JSON.stringify(key)}:${value}}`;
  if (!line.endsWith(tail)) return undefined;
  return line.slice(line.length - tail.length + key.length + 3, line.length - 1);
}

// module: 試験だけが使う。領域の実装の写し（規則を壊したもの）で不一致になることを確かめる。
export async function compare(domain, { fixture, module } = {}) {
  const spec = DOMAINS[domain];
  if (spec === undefined) throw new Error(`unknown domain: ${domain} (known: ${Object.keys(DOMAINS).join(", ")})`);
  const mod = await import(pathToFileURL(path.resolve(pkg, module ?? spec.module)).href);
  const target = path.resolve(pkg, fixture ?? spec.fixture);
  const files = fixtureFiles(target);
  const multi = files.length > 1 || target !== files[0];

  const rows = [];
  for (const file of files) {
    const lines = readFileSync(file, "utf8").split("\n");
    if (lines.at(-1) === "") lines.pop();
    if (lines.length === 0) throw new Error(`empty fixture: ${file}`);
    for (const [i, line] of lines.entries()) {
      rows.push({ file: path.relative(pkg, file), line: i + 1, raw: line, row: JSON.parse(line) });
    }
  }
  const where = (r) => (multi ? { file: r.file, line: r.line } : { line: r.line });
  const report = (ok, failures) => ({
    domain,
    fixture: path.relative(pkg, target),
    files: files.map((f) => path.relative(pkg, f)),
    total: rows.length,
    matched: ok.size,
    failures,
  });
  if (spec.custom !== undefined) {
    const { ok, failures } = await spec.custom(mod, rows, where);
    return report(ok, failures);
  }

  const fn = mod[spec.entry];
  const ok = new Set();
  const failures = [];
  for (const r of rows) {
    const input = spec.input(r.row);
    let out;
    try {
      out = await fn(input);
    } catch (e) {
      failures.push({ ...where(r), name: r.row.name, input, error: String(e?.message ?? e) });
      continue;
    }
    // 比較は欄ごとの JSON 直列化の byte 列（値の正規化はしない）。
    const diff = spec.fields.filter((k) => JSON.stringify(out?.[k]) !== JSON.stringify(r.row[k]));
    const extra = Object.keys(out ?? {}).filter((k) => !spec.fields.includes(k));
    if (diff.length === 0 && extra.length === 0) {
      ok.add(r);
    } else {
      const expected = Object.fromEntries(spec.fields.map((k) => [k, r.row[k]]));
      failures.push({ ...where(r), name: r.row.name, input, fields: diff, extra, expected, actual: out });
    }
  }
  return report(ok, failures);
}

/** project_controller の不変条件（runner の project_controller_invariants と同じ）。 */
async function compareProjectController(mod, rows, where) {
  const ok = new Set();
  const failures = [];
  for (const r of rows) {
    const inv = r.row.invariants;
    let out;
    try {
      out = mod.projectController(r.row.input);
    } catch (e) {
      failures.push({ ...where(r), name: r.row.name, error: String(e?.message ?? e) });
      continue;
    }
    const kind = out?.action?.kind;
    const problems = [];
    if (!inv.allowed_kinds.includes(kind)) problems.push(`action ${kind} not in ${inv.allowed_kinds.join(",")}`);
    if (inv.must_not_launch_root && kind === "launch_root") problems.push("launched a root");
    if (inv.wait_phase !== undefined && out.action.phase !== inv.wait_phase) problems.push(`phase must be ${inv.wait_phase}`);
    const b = r.row.input;
    const based = { revision: b.controller.revision, human_epoch: b.controller.human_epoch, request_sha256: b.project.request_sha256 };
    if (JSON.stringify(out.based_on) !== JSON.stringify(based)) problems.push("based_on");
    if (problems.length === 0) ok.add(r);
    else failures.push({ ...where(r), name: r.row.name, problems, actual: out });
  }
  return { ok, failures };
}

/** 1 pass を fake host で replay する。出力の直列化（throw なら `<throw: …>`）と不一致の理由を返す。 */
async function replayPass(mod, stub, replay, trace) {
  const host = replay.fakeHost(trace.ops, stub.HostError, REPLAY_CTX.token);
  stub.setCosDigestListRuns(host.cosDigestListRuns);
  stub.setCosDigestReadRun(host.cosDigestReadRun);
  let output;
  let error;
  try {
    output = JSON.stringify(await mod.cosDigestPass(REPLAY_CTX));
  } catch (e) {
    error = String(e?.message ?? e);
  }
  return { output: output ?? `<throw: ${error}>`, error, problems: host.finish() };
}

/** cos_digest の比較（付記 S4-16）。行ごとに pass を replay し、全行を正順と逆順の 2 回流す。 */
async function compareCosDigest(mod, rows, where) {
  const stub = await import(HOST_STUB);
  const replay = await import(pathToFileURL(path.join(pkg, "src/testing/pass-replay.ts")).href);
  const ok = new Set();
  const failures = [];
  const plans = new Map();
  const outputs = new Map();
  for (const r of rows) {
    const failure = { ...where(r), name: r.row.name, fields: [] };
    let trace;
    let expected;
    if (Object.hasOwn(r.row, "ops")) {
      trace = { name: r.row.name, ops: r.row.ops, output: r.row.output };
      expected = rawTailValue(r.raw, r.row, "output") ?? '<fixture line does not end with a canonical "output">';
    } else {
      // decide の比較（run 1 つ）。期待値は行の生の byte 列。
      const rawCommands =
        rawTailValue(r.raw, r.row, "commands") ?? '<fixture line does not end with a canonical "commands">';
      try {
        const out = mod.decide(mod.parseInput(r.row.input));
        if (JSON.stringify(out.commands) !== rawCommands) failure.fields.push("commands");
      } catch (e) {
        failure.error = String(e?.message ?? e);
      }
      try {
        trace = replay.passFromCase(r.row.name, r.row.input, r.row.commands);
        expected = `{"runs":[{"run_id":${JSON.stringify(r.row.input.run_id)},"commands":${rawCommands}}]}`;
      } catch (e) {
        failure.error ??= String(e?.message ?? e);
      }
    }
    if (trace !== undefined) {
      plans.set(r, trace);
      const got = await replayPass(mod, stub, replay, trace);
      outputs.set(r, got.output);
      if (got.error !== undefined) failure.error ??= got.error;
      else if (got.output !== expected) failure.fields.push("output");
      if (got.problems.length > 0) failure.problems = got.problems;
      if (failure.fields.includes("output")) {
        failure.expected = expected;
        failure.actual = got.output;
      }
    }
    if (failure.fields.length === 0 && failure.error === undefined && failure.problems === undefined) ok.add(r);
    else failures.push(failure);
  }
  for (const r of [...rows].reverse()) {
    const trace = plans.get(r);
    if (trace === undefined || !ok.has(r)) continue;
    const again = await replayPass(mod, stub, replay, trace);
    if (again.output !== outputs.get(r) || again.problems.length > 0) {
      ok.delete(r);
      failures.push({ ...where(r), name: r.row.name, order: "reverse", error: "output differs from the forward pass" });
    }
  }
  return { ok, failures };
}

async function main(argv) {
  const args = [...argv];
  let fixture;
  const at = args.indexOf("--fixture");
  if (at >= 0) {
    fixture = args[at + 1];
    if (fixture === undefined) throw new Error("--fixture needs a path");
    args.splice(at, 2);
  }
  if (args.length !== 1) throw new Error("usage: compare.mjs <domain> [--fixture <file or dir>]");
  const r = await compare(args[0], { fixture });
  console.log(`${r.domain}: ${r.matched}/${r.total} matched (${r.files.join(", ")})`);
  if (r.failures.length > 0) {
    console.error(`${r.failures.length} mismatched; first ${Math.min(SHOW, r.failures.length)}:`);
    for (const f of r.failures.slice(0, SHOW)) console.error(JSON.stringify(f));
    return 1;
  }
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (e) => {
      console.error(String(e?.message ?? e));
      process.exit(2);
    },
  );
}
