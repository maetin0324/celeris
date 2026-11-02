// 領域の TS 実装を Rust が書いた fixture の全行に通し、比較欄の byte 一致を確かめる（境界 ADR 付記 S2-2・S4-2）。
// 使い方: node scripts/compare.mjs <domain> [--fixture <file か dir>]
// fixture が dir なら、その直下の *.jsonl を名前順にすべて読む（cos_digest は fixtures/cos_digest/ の
// cases.jsonl と、後から足される traces.jsonl などを同じ形で読む）。
// 一致件数を出す。不一致・TS の throw が 1 件でもあれば最初の数件を出して exit 1。
// Node 24 の型除去で src/domains/<domain>.ts を直接読む（celeris:host を import しない領域だけ）。
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SHOW = 5;

// 領域ごとに、fixture の 1 行から入力 DTO を作る関数・呼ぶ関数・比べる欄（出力 DTO の宣言順）を書く。
// rawTail: その欄が行の最後の欄なら、期待値を JSON.parse→stringify の写しでなく行の生の byte 列から取る。
// bothOrders: 全行を正順と逆順の 2 回流し、同じ入力に同じ出力を要求する（隠れた入力の検出。付記 S4-6）。
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
  cos_digest: {
    module: "src/domains/cos_digest.ts",
    entry: "cosDigest",
    fixture: "fixtures/cos_digest",
    input: (row) => row.input,
    fields: ["commands"],
    rawTail: "commands",
    bothOrders: true,
  },
};

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

export async function compare(domain, { fixture } = {}) {
  const spec = DOMAINS[domain];
  if (spec === undefined) throw new Error(`unknown domain: ${domain} (known: ${Object.keys(DOMAINS).join(", ")})`);
  const mod = await import(pathToFileURL(path.join(pkg, spec.module)).href);
  const fn = mod[spec.entry];
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

  const ok = new Set();
  const failures = [];
  const outputs = new Map();
  for (const r of rows) {
    const input = spec.input(r.row);
    let out;
    try {
      out = await fn(input);
    } catch (e) {
      failures.push({ ...where(r), name: r.row.name, input, error: String(e?.message ?? e) });
      continue;
    }
    outputs.set(r, JSON.stringify(out));
    // 比較は欄ごとの JSON 直列化の byte 列（値の正規化はしない）。
    const expectedBytes = (k) => {
      if (k === spec.rawTail) {
        const raw = rawTailValue(r.raw, r.row, k);
        // 行の生の byte 列が再直列化と食い違う fixture は、比較できないので不一致として出す。
        return raw ?? `<fixture line does not end with a canonical "${k}">`;
      }
      return JSON.stringify(r.row[k]);
    };
    const diff = spec.fields.filter((k) => JSON.stringify(out?.[k]) !== expectedBytes(k));
    const extra = Object.keys(out ?? {}).filter((k) => !spec.fields.includes(k));
    if (diff.length === 0 && extra.length === 0) {
      ok.add(r);
    } else {
      const expected = Object.fromEntries(spec.fields.map((k) => [k, r.row[k]]));
      failures.push({ ...where(r), name: r.row.name, input, fields: diff, extra, expected, actual: out });
    }
  }
  if (spec.bothOrders) {
    for (const r of [...rows].reverse()) {
      const first = outputs.get(r);
      if (first === undefined) continue;
      let again;
      try {
        again = JSON.stringify(await fn(spec.input(r.row)));
      } catch (e) {
        again = `<throw: ${String(e?.message ?? e)}>`;
      }
      if (again !== first) {
        ok.delete(r);
        failures.push({ ...where(r), name: r.row.name, order: "reverse", error: "output differs from the forward pass" });
      }
    }
  }
  return {
    domain,
    fixture: path.relative(pkg, target),
    files: files.map((f) => path.relative(pkg, f)),
    total: rows.length,
    matched: ok.size,
    failures,
  };
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
