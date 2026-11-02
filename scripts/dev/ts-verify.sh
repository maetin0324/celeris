#!/usr/bin/env bash
# TS だけの変更の高速検証経路（ADR 2026-11-02-ts-fast-verify (1)(2)(5)）。
#   bash scripts/dev/ts-verify.sh --list            変更を ts-only / host-api / rust / none に分け、file ごとの区分と理由、
#                                                   単体試験の選択、走らせる段を出す（何も走らせない。cargo も起動しない）
#   bash scripts/dev/ts-verify.sh [--record <file>] [--no-build]
#                                                   ts-only なら段を順に走らせ、最初の失敗で止まる。ts-only でなければ
#                                                   「従来の経路へ」と出して exit 0（cargo の試験は test-affected.sh が走らせる）
#   bash scripts/dev/ts-verify.sh --grade [--record <file>] [--no-build]
#                                                   ts-only か host-api なら、候補の作業ツリーで 10 段を通す（付記 grade-gate）。
#                                                   単体試験は全部。golden の段は compare.mjs を使わず、cache の golden との cmp と
#                                                   runner の領域ごとの採点で決める。rust・none・追加の禁止の扱いは引数なしと同じ。
#                                                   test-affected.sh が ts-only でない control-ts/・docs/api/script-host/ の変更で呼ぶ
# 基準は ${CELERIS_WU_BASE:-$(git merge-base HEAD main)}。差分は基準との git diff --name-status（rename は両側）と未追跡 file。
# 段: classify → runner-cache → install → typecheck → bundle → unit → golden → safety → integration → activate。
# bundle は毎回 $TMPDIR の使い捨て dir に作り直す（control-ts/dist は読まない・使わない）。
# runner と golden は scripts/dev/script-runner-cache.sh で解決する（この script と同じ dir のもの）。
# 領域の一覧は runner の `domains`（Rust の表）から取る。control-ts/package.json の scripts は呼ばない。
# --record <file>: 段ごとの秒と exit、cargo の起動回数（PATH の先頭に置く数える wrapper）、link_count
#   （CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER を数える wrapper にする）を JSON で書く。
# --no-build: runner の cache が無ければ build せず exit 3。
# exit: 0 = 通った（または ts-only でない。--grade では ts-only・host-api でない）、1 = 段の失敗・追加の禁止に当たった、2 = 使い方の誤り、3 = --no-build で runner が無い。
# この script と runner は検証の道具で、daemon・celerisctl の経路に入らない。
set -euo pipefail

usage() { echo "usage: ts-verify.sh --list | [--grade] [--record <file>] [--no-build]" >&2; exit 2; }
list_only=0
grade=0
record=""
no_build=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --list) list_only=1 ;;
    --record) [ "$#" -ge 2 ] || usage; record="$2"; shift ;;
    --no-build) no_build=1 ;;
    --grade) grade=1 ;;
    *) usage ;;
  esac
  shift
done
[ "$list_only" -eq 0 ] || { [ -z "$record" ] && [ "$grade" -eq 0 ]; } || usage
if [ -n "$record" ]; then
  case "$record" in /*) ;; *) record="$PWD/$record" ;; esac
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(git rev-parse --show-toplevel)"
cd "$root"
base="${CELERIS_WU_BASE:-$(git merge-base HEAD main)}"
cts="$root/control-ts"

work="$(mktemp -d "${TMPDIR:-/tmp}/ts-verify.XXXXXX")"
cleanup() { [ -n "${TS_VERIFY_KEEP_WORK:-}" ] || rm -rf "$work"; }
trap cleanup EXIT

now() { date +%s.%N; }
t_start="$(now)"
: >"$work/stages.tsv"
: >"$work/cargo.log"
: >"$work/link.log"
klass=""
final_exit=0

write_record() {
  [ -n "$record" ] || return 0
  python3 -I - "$work" "$record" "$base" "$klass" "$final_exit" "$t_start" "$(now)" "$grade" <<'PY'
import json, os, sys
work, out, base, klass, code, t0, t1, grade = sys.argv[1:9]
stages = []
for line in open(os.path.join(work, "stages.tsv")):
    name, secs, rc, note = (line.rstrip("\n").split("\t") + [""])[:4]
    stages.append({"name": name, "secs": round(float(secs), 3), "exit": int(rc), **({"note": note} if note else {})})
cargo = [l.rstrip("\n") for l in open(os.path.join(work, "cargo.log")) if l.strip()]
links = [l for l in open(os.path.join(work, "link.log")) if l.strip()]
units = []
p = os.path.join(work, "units.txt")
if os.path.exists(p):
    units = [l.strip() for l in open(p) if l.strip()]
rec = {
    "schema": "celeris.ts-verify-record/1",
    "base": base,
    "class": klass,
    "grade": grade == "1",
    "exit": int(code),
    "total_secs": round(float(t1) - float(t0), 3),
    "stages": stages,
    "cargo_invocations": len(cargo),
    "cargo_calls": cargo,
    # compile・link を伴いうる subcommand（metadata などの読み取りを除く）。
    "cargo_compile_invocations": sum(1 for c in cargo if (c.split() or [""])[0] in
                                     ("build", "b", "run", "r", "test", "t", "check", "c", "nextest", "rustc", "clippy", "doc", "bench", "install")),
    "link_count": len(links),
    "units": units,
}
os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
with open(out, "w") as f:
    json.dump(rec, f, indent=1, ensure_ascii=False)
    f.write("\n")
PY
}

finish() { # code
  final_exit="$1"
  write_record
  exit "$final_exit"
}

# ---- 分類（(1)）。python で file ごとの区分・理由・追加の禁止を決め、$work/classify.json に書く ----
cat >"$work/classify.py" <<'PY'
import json, os, re, subprocess, sys

base, out = sys.argv[1], sys.argv[2]

def git(*args):
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout

# (path, status)。rename・copy は両側の path。旧い側は削除として扱う。
entries = []
for line in git("diff", "--name-status", "-M", base, "--").splitlines():
    parts = line.split("\t")
    st = parts[0]
    if st[:1] in ("R", "C"):
        if st[:1] == "R":
            entries.append((parts[1], "D"))
        entries.append((parts[2], "A"))
    else:
        entries.append((parts[1], st[:1]))
for p in git("ls-files", "--others", "--exclude-standard").splitlines():
    if p:
        entries.append((p, "A"))
seen = {}
for p, st in entries:
    # 同じ path が削除と追加の両方に出たら、削除（強い方）を残す。
    if p not in seen or st == "D":
        seen[p] = st

HOST_CONTRACT = {
    "control-ts/src/host.d.ts",
    "crates/script-runtime/src/ops.rs",
    "crates/script-runtime/src/dto.rs",
    "crates/script-runtime/src/host.js",
    "crates/script-runtime/src/bootstrap.js",
}
TOOLING = {
    "control-ts/package.json": "依存の版・scripts",
    "control-ts/pnpm-lock.yaml": "依存の版（lockfile）",
    "control-ts/pnpm-workspace.yaml": "pnpm の設定",
    "control-ts/tsconfig.json": "型検査の設定",
    "control-ts/vitest.config.mjs": "単体試験の設定",
    "control-ts/bundle.config.json": "manifest の domains・capabilities",
}
RANK = {"none": 0, "ts-only": 1, "host-api": 2, "rust": 3}

def area(path, st):
    if path.startswith("agent-docs/") or (path.startswith("docs/") and not path.startswith("docs/api/")):
        return "neutral", "進捗・ADR・文書（分類に影響しない）"
    if path.startswith("docs/api/script-host/"):
        return "host-api", "TS と Rust の契約（host API の schema）"
    if path in HOST_CONTRACT:
        return "host-api", "TS と Rust の契約（型・native op・bootstrap）"
    if path.startswith("control-ts/fixtures/"):
        return "host-api", "Rust golden（fixture）。合否の基準を変える"
    if path.startswith("control-ts/scripts/"):
        return "host-api", "TS の検証の道具（control-ts/scripts）"
    if path in TOOLING:
        return "host-api", "検証の道具・設定: " + TOOLING[path]
    if path.startswith("control-ts/src/testing/"):
        return "host-api", "試験の土台（host の写しの stub）"
    if path.startswith("control-ts/src/") and path.endswith(".test.ts") and st == "D":
        return "host-api", "単体試験の削除"
    if path.startswith("control-ts/src/") and path.endswith(".ts"):
        if path.endswith(".test.ts"):
            return "ts-only", "業務 TS の単体試験の追加・変更"
        return "ts-only", "業務 TS"
    if path.startswith("control-ts/"):
        return "host-api", "control-ts/ のその他（業務 TS 以外）"
    return "rust", "従来の経路（Rust・scripts・web など）"

files = []
klass = "none"
for p in sorted(seen):
    a, why = area(p, seen[p])
    files.append({"path": p, "status": seen[p], "area": a, "reason": why})
    if a != "neutral" and RANK[a] > RANK[klass]:
        klass = a

# 追加の禁止: 業務 TS の候補 file の追加した行。
FORBID_ALL = re.compile(r"@ts-(nocheck|ignore|expect-error)")
FORBID_TEST = re.compile(r"\.(only|skip|todo)\(")
violations = []
for f in files:
    if f["area"] != "ts-only" or f["status"] == "D":
        continue
    p = f["path"]
    tracked = subprocess.run(["git", "cat-file", "-e", f"{base}:{p}"], capture_output=True).returncode == 0
    if tracked:
        added = [l[1:] for l in git("diff", "-U0", base, "--", p).splitlines() if l.startswith("+") and not l.startswith("+++")]
    else:
        try:
            added = open(p, encoding="utf-8", errors="replace").read().splitlines()
        except OSError:
            added = []
    for line in added:
        if FORBID_ALL.search(line):
            violations.append({"path": p, "line": line.strip(), "rule": "@ts-nocheck・@ts-ignore・@ts-expect-error の追加"})
        if p.endswith(".test.ts") and FORBID_TEST.search(line):
            violations.append({"path": p, "line": line.strip(), "rule": "単体試験の .only( ・ .skip( ・ .todo( の追加"})

# fixture を書く・読む crate（crates/*/{src,examples,tests} で control-ts/fixtures/ を参照する crate）。
fixture_crates = []
if any(f["path"].startswith("control-ts/fixtures/") for f in files):
    r = subprocess.run(["git", "grep", "-l", "-F", "control-ts/fixtures/", "--", ":(glob)crates/*/src/**", ":(glob)crates/*/examples/**", ":(glob)crates/*/tests/**"],
                       capture_output=True, text=True)
    dirs = sorted({l.split("/")[1] for l in r.stdout.splitlines() if l.startswith("crates/")})
    for d in dirs:
        name = d
        try:
            m = re.search(r'^name\s*=\s*"([^"]+)"', open(f"crates/{d}/Cargo.toml").read(), re.M)
            if m:
                name = m.group(1)
        except OSError:
            pass
        fixture_crates.append(name)

# cache key の入力（script-runner-cache.sh）が変わったか。
KEY_PREFIX = ("crates/script-runtime/", "crates/task-core/")
KEY_FILES = {"Cargo.lock", "Cargo.toml", "docs/api/script-host/host-v1.schema.json"}
key_changed = [f["path"] for f in files if f["path"].startswith(KEY_PREFIX) or f["path"] in KEY_FILES]

json.dump({"class": klass, "files": files, "violations": violations, "fixture_crates": fixture_crates,
           "key_changed": key_changed}, open(out, "w"), ensure_ascii=False, indent=1)
PY

# ---- TS の import グラフ（(1)-1）。esbuild の metafile で各入口の読む file を出す ----
cat >"$work/graph.mjs" <<'JS'
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";

const [pkg, out] = process.argv.slice(2);
const require = createRequire(path.join(pkg, "package.json"));
const { build } = require("esbuild");
const walk = (dir) =>
  readdirSync(path.join(pkg, dir), { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)],
  );
const srcFiles = walk("src");
const entries = ["src/main.ts", ...srcFiles.filter((f) => f.endsWith(".test.ts")).sort()];
// 式で組み立てる import(...)（文字列の literal 以外）があればグラフで追えない。
const dynamic = [];
const literal = /^\s*(["'])[^"'`$]*\1\s*\)/;
for (const f of srcFiles.filter((f) => f.endsWith(".ts"))) {
  const text = readFileSync(path.join(pkg, f), "utf8");
  for (const m of text.matchAll(/\bimport\s*\(/g)) {
    if (!literal.test(text.slice(m.index + m[0].length))) dynamic.push(f);
  }
}
const result = { entries: {}, dynamic: [...new Set(dynamic)] };
for (const entry of entries) {
  const r = await build({
    absWorkingDir: pkg,
    entryPoints: [entry],
    bundle: true,
    write: false,
    metafile: true,
    format: "esm",
    platform: "neutral",
    logLevel: "silent",
    external: ["celeris:host", "vitest", "vitest/*", "node:*"],
  });
  result.entries["control-ts/" + entry] = Object.keys(r.metafile.inputs).map((p) => "control-ts/" + p);
}
writeFileSync(out, JSON.stringify(result));
JS

# 単体試験の選択（(1)-1）。危険側の skip をしない: 追えないときは全部。出力: units.txt（選んだ試験）と理由 1 行。
select_units() {
  local graph_ok=0
  if [ -d "$cts/node_modules/esbuild" ] && node "$work/graph.mjs" "$cts" "$work/graph.json" 2>"$work/graph.err"; then
    graph_ok=1
  fi
  python3 -I - "$work/classify.json" "$work/graph.json" "$graph_ok" "$work/units.txt" <<'PY'
import json, sys
cls = json.load(open(sys.argv[1]))
graph_ok = sys.argv[3] == "1"
changed = [f["path"] for f in cls["files"] if f["area"] == "ts-only"]
reason = None
tests = []
if not graph_ok:
    reason = "全部（import グラフの計算が失敗した・node_modules が無い）"
else:
    g = json.load(open(sys.argv[2]))
    tests = sorted(e for e in g["entries"] if e.endswith(".test.ts"))
    if g["dynamic"]:
        reason = "全部（src/ に式で組み立てる import(...) がある: " + ", ".join(g["dynamic"]) + "）"
    else:
        read_by = {f: [e for e, ins in g["entries"].items() if f in ins] for f in changed}
        orphan = [f for f, es in read_by.items() if not es]
        untested = [f for f, es in read_by.items() if es and not any(e.endswith(".test.ts") for e in es)]
        if orphan:
            reason = "全部（どの入口からも読まれない変更: " + ", ".join(orphan) + "）"
        elif untested:
            reason = "全部（試験から読まれない変更: " + ", ".join(untested) + "）"
        else:
            picked = sorted({e for es in read_by.values() for e in es if e.endswith(".test.ts")})
            if picked:
                tests = picked
                reason = "import グラフで選んだ " + str(len(picked)) + " file"
            else:
                reason = "全部（選べる試験が無い）"
if reason.startswith("全部"):
    tests = ["ALL"]
with open(sys.argv[4], "w") as f:
    f.write("\n".join(tests) + "\n")
print(reason)
PY
}

STAGES="classify runner-cache install typecheck bundle unit golden safety integration activate"

print_list() {
  python3 -I - "$work/classify.json" "$base" <<'PY'
import json, sys
c = json.load(open(sys.argv[1]))
print(f"class: {c['class']}  (基準 {sys.argv[2]})")
for f in c["files"]:
    print(f"  {f['area']:<8} {f['status']} {f['path']}  — {f['reason']}")
for v in c["violations"]:
    print(f"  forbidden {v['path']}: {v['rule']}: {v['line']}")
if c["fixture_crates"]:
    print("  fixture を書く・読む crate（cargo の試験の対象に足す）: " + " ".join(c["fixture_crates"]))
if c["key_changed"]:
    print("  runner の cache key の入力が変わった（runner と golden を作り直して TS の段も走らせる）: " + " ".join(c["key_changed"]))
PY
}

python3 -I "$work/classify.py" "$base" "$work/classify.json"
klass="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["class"])' "$work/classify.json")"
nviol="$(python3 -I -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["violations"]))' "$work/classify.json")"

if [ "$list_only" -eq 1 ]; then
  print_list
  if [ "$nviol" -gt 0 ]; then
    echo "stages: classify で失敗（追加の禁止に当たった）"
    exit 1
  fi
  case "$klass" in
    ts-only)
      echo "unit: $(select_units)"
      grep -v '^ALL$' "$work/units.txt" | sed 's/^/  /' || true
      echo "stages: $STAGES（cargo 0 回）" ;;
    host-api|rust)
      echo "stages: classify のあと従来の経路へ（cargo の試験は bash scripts/dev/test-affected.sh）" ;;
    none)
      echo "stages: なし（試験の対象が無い）" ;;
  esac
  exit 0
fi

# ---- 段の実行 ----
# cargo を数える wrapper と linker を数える wrapper。runner の build（ensure）だけが cargo を起動しうる。
mkdir -p "$work/bin"
real_cargo="$(command -v cargo || true)"
cat >"$work/bin/cargo" <<EOF
#!/bin/sh
echo "\$*" >>"$work/cargo.log"
[ -n "$real_cargo" ] || { echo "ts-verify: cargo が無い" >&2; exit 127; }
exec "$real_cargo" "\$@"
EOF
host_linker="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-}"
[ -n "$host_linker" ] || host_linker="$(sed -n '/^\[target.x86_64-unknown-linux-gnu\]/,/^\[/s/^linker *= *"\(.*\)"/\1/p' "$HOME/.cargo/config.toml" 2>/dev/null | head -1 || true)"
[ -n "$host_linker" ] || host_linker=cc
cat >"$work/bin/count-linker" <<EOF
#!/bin/sh
echo link >>"$work/link.log"
exec "$host_linker" "\$@"
EOF
chmod +x "$work/bin/cargo" "$work/bin/count-linker"
export PATH="$work/bin:$PATH"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$work/bin/count-linker"

stage() { # name note cmd...
  local name="$1" note="$2" t0 rc
  shift 2
  echo "ts-verify: [$name] $*" >&2
  t0="$(now)"
  set +e
  "$@"
  rc=$?
  set -e
  printf '%s\t%s\t%s\t%s\n' "$name" "$(python3 -I -c 'import sys; print(float(sys.argv[2]) - float(sys.argv[1]))' "$t0" "$(now)")" "$rc" "$note" >>"$work/stages.tsv"
  if [ "$rc" -ne 0 ]; then
    echo "ts-verify: 段 $name が失敗した（exit $rc）" >&2
    # --no-build で runner が無いときだけ exit 3。他の失敗は exit 1。
    if [ "$name" = runner-cache ] && [ "$rc" -eq 3 ]; then finish 3; fi
    finish 1
  fi
}

do_classify() {
  print_list >&2
  [ "$nviol" -eq 0 ] || { echo "ts-verify: 追加の禁止に当たった" >&2; return 1; }
  if [ "$klass" = ts-only ]; then
    echo "ts-verify: unit の選択: $(select_units)" >&2
  fi
}
stage classify "$klass" do_classify
# --grade は host-api でも段を通す（付記 grade-gate）。rust・none は引数なしと同じく抜ける。
if [ "$grade" -eq 1 ] && [ "$klass" = host-api ]; then
  echo "ts-verify: --grade: host-api の変更。候補の作業ツリーで 10 段を通し、Rust の runner と cache の golden で採点する" >&2
  echo ALL >"$work/units.txt"
elif [ "$klass" != ts-only ]; then
  if [ "$klass" = none ]; then
    echo "ts-verify: 試験の対象が無い（$klass）" >&2
  else
    echo "ts-verify: $klass の変更。従来の経路へ（cargo の試験は bash scripts/dev/test-affected.sh が走らせる）" >&2
  fi
  finish 0
fi

cache="$here/script-runner-cache.sh"
runner=""
do_runner_cache() {
  if runner="$(bash "$cache" path)"; then return 0; fi
  if [ "$no_build" -eq 1 ]; then
    echo "ts-verify: runner の cache が無い（--no-build）" >&2
    return 3
  fi
  runner="$(bash "$cache" ensure)"
}
stage runner-cache "" do_runner_cache
golden="$(dirname "$runner")/golden"
dist="$work/dist"

do_install() {
  [ ! -f "$cts/node_modules/.modules.yaml" ] || return 0
  (cd "$cts" && pnpm install --offline --frozen-lockfile)
}
stage install "" do_install
tsbin="$cts/node_modules/.bin"
stage typecheck "" "$tsbin/tsc" --noEmit -p "$cts/tsconfig.json"

do_bundle() {
  TS_VERIFY_BUILD="$cts/scripts/build-bundle.mjs" TS_VERIFY_OUT="$dist" node --input-type=module -e '
    const { buildBundle } = await import(process.env.TS_VERIFY_BUILD);
    await buildBundle({ outDir: process.env.TS_VERIFY_OUT });
  '
  [ -f "$dist/bundle.mjs" ] && [ -f "$dist/manifest.json" ]
}
stage bundle "" do_bundle

do_unit() {
  local units=()
  mapfile -t units < <(grep -v '^ALL$' "$work/units.txt" | sed 's|^control-ts/||' || true)
  if grep -qx ALL "$work/units.txt"; then units=(); fi
  (cd "$cts" && "$tsbin/vitest" run --config vitest.config.mjs "${units[@]}") || return 1
  # control-ts/scripts/*.test.mjs は全部走らせる。ただし test-affected.test.mjs は除く: これは
  # scripts/dev/test-affected.sh（Rust の経路の道具。ts-only では基準から変わらない）の試験で、fixture の中で
  # cargo metadata を起動する。ts-only の経路は cargo を 1 回も起動しない（ADR (2)・付記 wire-1）。
  # この試験は scripts/dev か control-ts/scripts の変更（rust・host-api）で test-affected.sh の control-ts の段が走らせる。
  local scripts=() f
  for f in "$cts"/scripts/*.test.mjs; do
    case "${f##*/}" in test-affected.test.mjs) ;; *) scripts+=("scripts/${f##*/}") ;; esac
  done
  [ "${#scripts[@]}" -gt 0 ] || { echo "ts-verify: control-ts/scripts/*.test.mjs が無い" >&2; return 1; }
  (cd "$cts" && node --test "${scripts[@]}")
}
if [ "$grade" -eq 1 ]; then echo ALL >"$work/units.txt"; fi
stage unit "$(tr '\n' ' ' <"$work/units.txt")" do_unit

# 領域の一覧は runner の domains（Rust の表）から取る。1 行 = "<domain> <fixture の相対 path>"。
do_domains() {
  "$runner" domains >"$work/domains.json" || return 1
  python3 -I - "$work/domains.json" >"$work/domains.txt" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
items = d["domains"] if isinstance(d, dict) else d
rows = []
for it in items:
    name = it.get("domain", it.get("name"))
    fixture = it["fixture"]
    if not name or not fixture or "/" not in fixture or ".." in fixture.split("/"):
        sys.exit(f"bad domain entry: {it}")
    rows.append(f"{name} {fixture}")
if not rows:
    sys.exit("runner domains is empty")
print("\n".join(rows))
PY
}
do_golden() {
  do_domains || { echo "ts-verify: runner の domains を読めない" >&2; return 1; }
  local domain fixture
  while read -r domain fixture; do
    cmp "$cts/fixtures/$fixture" "$golden/$fixture" || {
      echo "ts-verify: control-ts/fixtures/$fixture が Rust の golden と違う" >&2; return 1; }
    if [ "$grade" -eq 1 ]; then
      # --grade: 候補の control-ts/scripts（compare.mjs）は採点に使わない。runner が領域ごとに cache の golden で採点する。
      # --domain で絞ると complete は false なので ok だけを見る（全領域の complete は段 integration が見る）。
      "$runner" run --bundle-dir "$dist" --fixtures "$golden" --suite golden --domain "$domain" >"$work/golden-$domain.json" || {
        echo "ts-verify: runner の golden の採点が失敗した（$domain）" >&2; return 1; }
      python3 -I -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("ok") is True else 1)' "$work/golden-$domain.json" || {
        echo "ts-verify: runner の出力が ok でない（golden $domain）" >&2; return 1; }
    else
      node "$cts/scripts/compare.mjs" "$domain" --fixture "$golden/$fixture" || return 1
    fi
  done <"$work/domains.txt"
}
stage golden "" do_golden

# runner の出力（JSON 1 個）が ok かつ（run では）complete であること。
runner_ok() { # json file
  python3 -I -c '
import json, sys
d = json.load(open(sys.argv[1]))
ok = d.get("ok") is True and (d.get("command") != "run" or d.get("complete") is True)
sys.exit(0 if ok else 1)' "$1"
}
do_suite() { # suite
  "$runner" run --bundle-dir "$dist" --fixtures "$golden" --suite "$1" >"$work/run-$1.json" || return 1
  runner_ok "$work/run-$1.json" || { echo "ts-verify: runner の出力が ok でない（$1）" >&2; return 1; }
}
stage safety "" do_suite safety
stage integration "" do_suite golden

do_activate() {
  mkdir -p "$work/state"
  "$runner" activate --bundle-dir "$dist" --state-dir "$work/state" >"$work/activate.json" || return 1
  runner_ok "$work/activate.json" || { echo "ts-verify: activate の出力が ok でない" >&2; return 1; }
}
stage activate "" do_activate

echo "ts-verify: $klass の検証が通った（--grade $grade、cargo $(wc -l <"$work/cargo.log") 回、link $(wc -l <"$work/link.log") 回）" >&2
finish 0
