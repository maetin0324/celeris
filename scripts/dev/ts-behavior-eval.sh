#!/usr/bin/env bash
# 方針改善（behavior change）の独立した評価（ADR 2026-11-03-ts-intended-behavior-change D3）。cargo を起動しない。
#   bash scripts/dev/ts-behavior-eval.sh --out <dir> [--patch <file>] [--label <name>] [--changes <dir>]
#     HEAD の使い捨ての作業ツリー（$TMPDIR の git worktree --detach）に候補の patch を当て、control-ts の
#     src/domains/cos_digest.ts を esbuild で 1 file にし、次の 2 つを数える:
#     (1) 旧挙動の fixture（control-ts/fixtures/cos_digest/cases.jsonl、Rust golden）の全行で TS が Rust と同じか
#     (2) 期待差分 fixture（control-ts/behavior-changes/cos_digest/*.cases.jsonl）の全行で TS の出力が `expected` と同じか
#         （`expect: explained` は改善した出力、`expect: equal` は Rust と同じ出力）
#     結果を <dir>/<label>.json に書き、1 行の raw record を <dir>/records.jsonl に足す。
#     --changes: 期待差分 fixture の dir（`<名>.json` と `<名>.cases.jsonl` の組。既定は作業ツリーの
#     control-ts/behavior-changes/cos_digest）。P2-G の課題（scripts/dev/llm-improve/tasks/<課題>）を渡す（ADR 2026-11-03-llm-self-improve-trial）。
# 合否: (1) の差分 0 件かつ (2) の食い違い 0 件で ok。期待差分 fixture の file の sha256 が仕様の cases_sha256 と違えば失敗。
# 比べ方: JSON.stringify の byte 列で比べる（値の正規化はしない。期待差分 fixture は key の名前順で書くので、TS の出力も key だけ並べ替える）。説明の判定（仕様の述語）は Rust（task-core）だけが持ち、
# 期待差分 fixture の `expected` は Rust の試験が参照実装と仕様から作る。この script は TS の出力をそれと比べるだけ。
# 守ること: この作業ツリーと本番に触れない。LLM・外部ネットワークを使わない（pnpm install --offline）。
# exit: 0 = ok、1 = 評価に落ちた・段の失敗、2 = 使い方の誤り。
set -euo pipefail

usage() { echo "usage: ts-behavior-eval.sh --out <dir> [--patch <file>] [--label <name>] [--changes <dir>]" >&2; exit 2; }
out=""
patch=""
label=""
changes=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out) [ "$#" -ge 2 ] || usage; out="$2"; shift ;;
    --patch) [ "$#" -ge 2 ] || usage; patch="$2"; shift ;;
    --label) [ "$#" -ge 2 ] || usage; label="$2"; shift ;;
    --changes) [ "$#" -ge 2 ] || usage; changes="$2"; shift ;;
    *) usage ;;
  esac
  shift
done
[ -n "$out" ] || usage
mkdir -p "$out"
out="$(cd "$out" && pwd)"
if [ -n "$patch" ]; then
  [ -f "$patch" ] || { echo "ts-behavior-eval: patch が無い: $patch" >&2; exit 2; }
  patch="$(cd "$(dirname "$patch")" && pwd)/$(basename "$patch")"
fi
if [ -n "$changes" ]; then
  [ -d "$changes" ] || { echo "ts-behavior-eval: --changes の dir が無い: $changes" >&2; exit 2; }
  changes="$(cd "$changes" && pwd)"
fi
[ -n "$label" ] || label="$( [ -n "$patch" ] && basename "$patch" .patch || echo head )"

root="$(git rev-parse --show-toplevel)"
commit="$(git -C "$root" rev-parse HEAD)"
work="$(mktemp -d "${TMPDIR:?TMPDIR が要る}/ts-behavior-eval.XXXXXX")"
tree="$work/tree"
cleanup() {
  git -C "$root" worktree remove --force "$tree" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
now() { date +%s.%N; }
t0="$(now)"

git -C "$root" worktree add --detach "$tree" HEAD >/dev/null 2>&1
patch_sha=""
if [ -n "$patch" ]; then
  git -C "$tree" apply "$patch"
  patch_sha="$(sha256sum "$patch" | cut -d' ' -f1)"
fi
t_install="$(now)"
(cd "$tree/control-ts" && pnpm install --offline --frozen-lockfile) >"$work/install.log" 2>&1 || {
  tail -20 "$work/install.log" >&2; echo "ts-behavior-eval: pnpm install --offline が失敗した" >&2; exit 1; }
t_build="$(now)"
cat >"$work/build.mjs" <<'JS'
import { createRequire } from "node:module";
const [esbuildPath, input, outfile] = process.argv.slice(2);
const esbuild = createRequire(esbuildPath)("esbuild");
const stub = { name: "host-stub", setup(b) {
  b.onResolve({ filter: /^celeris:host$/ }, () => ({ path: "host", namespace: "stub" }));
  b.onLoad({ filter: /.*/, namespace: "stub" }, () => ({ contents: "export const cosDigestListRuns = () => { throw new Error('stub'); }; export const cosDigestReadRun = cosDigestListRuns;", loader: "js" }));
} };
await esbuild.build({ entryPoints: [input], outfile, bundle: true, format: "esm", platform: "node", plugins: [stub], logLevel: "error" });
JS
node "$work/build.mjs" "$tree/control-ts/node_modules/esbuild/package.json" \
  "$tree/control-ts/src/domains/cos_digest.ts" "$work/cand.mjs"
t_eval="$(now)"
cat >"$work/eval.mjs" <<'JS'
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
const [mod, fixtures, changes] = process.argv.slice(2);
const ts = await import(mod);
// 旧挙動の fixture は compare.mjs と同じく JSON.stringify の byte 列で比べる（TS の出力の key の順は Rust の DTO の宣言順）。
const canon = (v) => JSON.stringify(v);
// 期待差分 fixture は key を名前順にして書いてある（Rust の試験の `sorted`）。TS の出力も同じ順にしてから比べる。値は変えない。
const sorted = (v) => Array.isArray(v) ? v.map(sorted)
  : v !== null && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sorted(v[k])])) : v;
const run = (input) => { try { return ts.decide(ts.parseInput(input)).commands; } catch (e) { return { error: String(e.message) }; } };
const lines = (f) => readFileSync(f, "utf8").split("\n").filter((l) => l.trim());
let oldCases = 0, oldDiffs = 0;
const oldExamples = [];
for (const l of lines(path.join(fixtures, "cases.jsonl"))) {
  const c = JSON.parse(l);
  oldCases++;
  if (canon(run(c.input)) !== canon(c.commands)) { oldDiffs++; if (oldExamples.length < 3) oldExamples.push(c.name); }
}
const specs = [];
for (const f of readdirSync(changes).filter((n) => n.endsWith(".json")).sort()) {
  const spec = JSON.parse(readFileSync(path.join(changes, f), "utf8"));
  const casesFile = path.join(changes, f.replace(/\.json$/, ".cases.jsonl"));
  const bytes = readFileSync(casesFile);
  const casesSha = createHash("sha256").update(bytes).digest("hex");
  const r = { file: f, id: spec.id, version: spec.version, cases_sha256_ok: casesSha === spec.cases_sha256,
    cases: 0, explained_ok: 0, equal_ok: 0, wrong: 0, pilot_explained_ok: 0, wrong_examples: [] };
  for (const l of lines(casesFile)) {
    const c = JSON.parse(l);
    r.cases++;
    const got = canon(sorted(run(c.input)));
    const ok = got === canon(c.expected);
    if (ok && c.expect === "explained") { r.explained_ok++; if (c.name.startsWith("pilot/")) r.pilot_explained_ok++; }
    else if (ok && c.expect === "equal") r.equal_ok++;
    else {
      r.wrong++;
      if (r.wrong_examples.length < 3) {
        const m = got.match(/run は失敗: ([^)]*)\)/);
        r.wrong_examples.push({ name: c.name, expect: c.expect, ts_heading_reason_chars: m ? Array.from(m[1]).length : null });
      }
    }
  }
  specs.push(r);
}
const ok = oldDiffs === 0 && specs.length > 0 && specs.every((s) => s.cases_sha256_ok && s.wrong === 0);
console.log(JSON.stringify({ old_fixture_cases: oldCases, old_fixture_diffs: oldDiffs, old_fixture_diff_examples: oldExamples, specs, ok }));
JS
result="$(node "$work/eval.mjs" "$work/cand.mjs" "$tree/control-ts/fixtures/cos_digest" \
  "${changes:-$tree/control-ts/behavior-changes/cos_digest}")"
t_end="$(now)"
python3 -I - "$out" "$label" "$commit" "$patch_sha" "$result" "$t0" "$t_install" "$t_build" "$t_eval" "$t_end" <<'PY'
import json, os, sys
from datetime import datetime, timezone
out, label, commit, patch_sha, result, t0, ti, tb, te, t1 = sys.argv[1:11]
r = json.loads(result)
f = lambda a, b: round(float(b) - float(a), 3)
rec = {
    "schema": "celeris.ts-behavior-eval/1",
    "label": label,
    "commit": commit,
    "patch_sha256": patch_sha or None,
    "domain": "cos_digest",
    "at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
    "secs": {"total": f(t0, t1), "worktree": f(t0, ti), "install": f(ti, tb), "build": f(tb, te), "eval": f(te, t1)},
    "cargo_invocations": 0,
    **r,
}
json.dump(rec, open(os.path.join(out, f"{label}.json"), "w"), ensure_ascii=False, indent=1)
with open(os.path.join(out, "records.jsonl"), "a", encoding="utf-8") as fh:
    fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
s = r["specs"]
print(f"ts-behavior-eval: {label}: old fixture {r['old_fixture_cases']} 件の差分 {r['old_fixture_diffs']}、"
      + "、".join(f"{x['id']} v{x['version']}: explained {x['explained_ok']}（pilot {x['pilot_explained_ok']}）・equal {x['equal_ok']}・食い違い {x['wrong']}" for x in s)
      + f"、ok={r['ok']}、{rec['secs']['total']} 秒")
sys.exit(0 if r["ok"] else 1)
PY
