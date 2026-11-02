// 領域の TS 実装を Rust が書いた fixture の全行に通し、比較欄の byte 一致を確かめる（境界 ADR 付記 S2-2）。
// 使い方: node scripts/compare.mjs <domain> [--fixture <path>]
// 一致件数を出す。不一致・TS の throw が 1 件でもあれば最初の数件を出して exit 1。
// Node 24 の型除去で src/domains/<domain>.ts を直接読む（celeris:host を import しない領域だけ）。
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const pkg = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SHOW = 5;

// 領域ごとに、fixture の 1 行から入力 DTO を作る関数・呼ぶ関数・比べる欄（出力 DTO の宣言順）を書く。
const DOMAINS = {
  lane_rules: {
    module: "src/domains/lane_rules.ts",
    entry: "laneRules",
    fixture: "fixtures/lane_rules/all-features.jsonl",
    input: (row) => ({ features: row.features }),
    fields: ["rule_id", "proposed", "condition"],
  },
};

export async function compare(domain, { fixture } = {}) {
  const spec = DOMAINS[domain];
  if (spec === undefined) throw new Error(`unknown domain: ${domain} (known: ${Object.keys(DOMAINS).join(", ")})`);
  const mod = await import(pathToFileURL(path.join(pkg, spec.module)).href);
  const fn = mod[spec.entry];
  const file = path.resolve(pkg, fixture ?? spec.fixture);
  const lines = readFileSync(file, "utf8").split("\n");
  if (lines.at(-1) === "") lines.pop();
  if (lines.length === 0) throw new Error(`empty fixture: ${file}`);

  let matched = 0;
  const failures = [];
  for (const [i, line] of lines.entries()) {
    const row = JSON.parse(line);
    const input = spec.input(row);
    let out;
    try {
      out = await fn(input);
    } catch (e) {
      failures.push({ line: i + 1, input, error: String(e?.message ?? e) });
      continue;
    }
    // 比較は欄ごとの JSON 直列化の byte 列（値の正規化はしない）。
    const diff = spec.fields.filter((k) => JSON.stringify(out?.[k]) !== JSON.stringify(row[k]));
    const extra = Object.keys(out ?? {}).filter((k) => !spec.fields.includes(k));
    if (diff.length === 0 && extra.length === 0) {
      matched += 1;
    } else {
      const expected = Object.fromEntries(spec.fields.map((k) => [k, row[k]]));
      failures.push({ line: i + 1, input, fields: diff, extra, expected, actual: out });
    }
  }
  return { domain, fixture: path.relative(pkg, file), total: lines.length, matched, failures };
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
  if (args.length !== 1) throw new Error("usage: compare.mjs <domain> [--fixture <path>]");
  const r = await compare(args[0], { fixture });
  console.log(`${r.domain}: ${r.matched}/${r.total} matched (${r.fixture})`);
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
