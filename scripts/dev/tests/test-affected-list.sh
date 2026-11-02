#!/bin/sh
# scripts/dev/test-affected.sh --list を小さな git fixture（最小 cargo workspace）で確かめる。
# 確かめること: (a) 変更 crate+逆依存 crate、(b) workspace 共通 file で全 crate + 警告、
# (c) web のみ・docs のみは crate を出さない、(d) 統合時の基準が空 diff にならない、
# (e) web の型検査の選択（ADR 2026-10-23-test-affected-web-typecheck）: task-api とその通常依存（event kind の
# task-core）・docs/api・web/ の変更で選ばれ、無関係な crate・task-api の dev 依存だけでは選ばれない、
# (f) 実行時: node_modules が無ければ pnpm install、型検査が落ちたら test-affected.sh も落ちる（cargo より先）。
# (g) TS だけの変更（ADR 2026-11-02-ts-fast-verify (5)(6)）: control-ts の検査を ts-verify.sh に委ね、cargo を起動しない
# （cargo を失敗させる偽の cargo を PATH の先頭に置く）。Host API・検証の道具・golden・Rust の変更では cargo 側
# （celeris-script と逆依存・fixture の crate）を選び、runner の cache key の入力の変更では test-ts-verify も選ぶ。
# cargo の試験は走らせない（--list と、web の偽 toolchain・docs のみの実行）。
# 必須の health gate（scripts/dev/check-health.sh）は fixture に最小版を置き、test-affected.sh が
# 実行時に必ず呼ぶこと（--list では呼ばないこと）を記録 file で確かめる。本物の gate は repo 自身の
# test-affected.sh 実行で走る。
set -eu

here="$(cd "$(dirname "$0")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/test-affected-list.XXXXXX")"
mark="$(mktemp -d "${TMPDIR:-/tmp}/test-affected-mark.XXXXXX")/health"
trap 'rm -rf "$work" "$(dirname "$mark")"' EXIT
export HEALTH_MARK="$mark"
cd "$work"
git init -q -b main .
git config user.email t@example.com
git config user.name t
d=docs
api="$d/api"  # 生成型の元の schema の置き場（doc-links の検査が実在の path と見なさないよう変数で組む）
mkdir -p crates/a/src crates/b/src crates/c/src crates/task-core/src crates/task-api/src crates/script-runtime/src web "$d"/api "$d"
printf '[workspace]\nmembers = ["crates/a", "crates/b", "crates/c", "crates/task-core", "crates/task-api", "crates/script-runtime"]\nresolver = "2"\n' >Cargo.toml
mk() { # dir [dep] [package name]
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' "${3:-$1}" >"crates/$1/Cargo.toml"
  [ -z "${2:-}" ] || printf '%s = { path = "../%s" }\n' "$2" "$2" >>"crates/$1/Cargo.toml"
  echo "" >"crates/$1/src/lib.rs"
}
mk a
mk b a
mk c
mk task-core
mk task-api task-core
mk script-runtime "" celeris-script
# fixture を読む crate（a の試験が control-ts/fixtures/ を参照する）。
mkdir -p crates/a/tests control-ts/src/domains control-ts/fixtures/x control-ts/scripts
printf '// reads control-ts/fixtures/x/cases.jsonl\n' >crates/a/tests/fx.rs
echo '{}' >control-ts/fixtures/x/cases.jsonl
echo 'export const x = 1;' >control-ts/src/domains/x.ts
echo 'export {};' >control-ts/src/host.d.ts
echo '' >control-ts/scripts/compare.mjs
echo '{"scripts":{"typecheck":"tsc --noEmit","test":"vitest run"},"packageManager":"pnpm@1.0.0"}' >control-ts/package.json
mkdir -p "$api"/script-host
echo '{}' >"$api"/script-host/host-v1.schema.json
# c は task-api の dev 依存（schema の型には入らない）。
printf '\n[dev-dependencies]\nc = { path = "../c" }\n' >>crates/task-api/Cargo.toml
echo '{"scripts":{"typecheck":"tsc -b","test":"vitest run"},"packageManager":"pnpm@1.0.0"}' >web/package.json
echo '{}' >"$api"/schema.json
printf 'node_modules\nerr\n' >.gitignore
echo x >"$d"/x.md
mkdir -p scripts/dev
# fixture 用の最小 health gate: 呼ばれた記録を残す。
printf '#!/usr/bin/env bash\nset -eu\necho called >>"$HEALTH_MARK"\n' >scripts/dev/check-health.sh
# fixture 用の偽の ts-verify.sh: 呼ばれた記録を残し、--record の file に TSV_CLASS の class を書く。
cat >scripts/dev/ts-verify.sh <<'SH'
#!/bin/sh
echo "ts-verify $1" >>"$TSV_CALLS"
printf '{"class":"%s","exit":0,"stages":[{},{}]}' "${TSV_CLASS:-}" >"$2"
exit "${TSV_RC:-0}"
SH
export TSV_CALLS="$(dirname "$mark")/tsv"
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"

fail() { echo "FAIL: $*" >&2; exit 1; }
run() { CELERIS_WU_BASE="$base" bash "$here/scripts/dev/test-affected.sh" --list 2>"$work/err"; }

# (a) a を変更 → a と逆依存 b。c は出ない。未追跡 file も数える。
echo "// x" >>crates/a/src/lib.rs
[ "$(run)" = "a
b" ] || fail "(a) changed a: got [$(run)]"
git checkout -q -- .
echo "// new" >crates/c/src/new.rs
[ "$(run)" = "c
task-api" ] || fail "(a) untracked c: got [$(run)]"
rm crates/c/src/new.rs

# (b) Cargo.lock → 全 crate + 警告
echo "# x" >Cargo.lock
[ "$(run | grep -v '^\(web\|ts-verify\): ')" = "a
b
c
celeris-script
task-api
task-core" ] || fail "(b) all: got [$(run)]"
run | grep -qx 'ts-verify: test-ts-verify (理由: runner の cache key の入力の変更)' || fail "(b) Cargo.lock must select test-ts-verify: got [$(run)]"
run | grep -q '^web: typecheck+unit (理由: .*workspace 共通 file' || fail "(b) all must select web typecheck: got [$(run)]"
grep -q '全 crate' "$work/err" || fail "(b) no warning"
rm Cargo.lock

# (c) web のみ → crate は無く web の検査だけ。docs のみ → 空
echo "// w" >web/a.ts
[ "$(run)" = "web: typecheck+unit (理由: web/ の変更)" ] || fail "(c) web only: got [$(run)]"
rm web/a.ts
echo y >>"$d"/x.md
[ -z "$(run)" ] || fail "(c) docs only not empty"
[ ! -e "$mark" ] || fail "(c) --list must not run the health gate"
CELERIS_WU_BASE="$base" bash "$here/scripts/dev/test-affected.sh" >/dev/null 2>"$work/err" || fail "(c) docs only exit: $(cat "$work/err")"
[ "$(cat "$mark" 2>/dev/null)" = "called" ] || fail "(c) health gate was not run"
# health gate が落ちたら test-affected.sh も落ちる（必須検査を飛ばさない）。
printf '#!/usr/bin/env bash\nexit 3\n' >scripts/dev/check-health.sh
if CELERIS_WU_BASE="$base" bash "$here/scripts/dev/test-affected.sh" >/dev/null 2>&1; then fail "(c) failing health gate must fail test-affected"; fi

# (d) 統合の検査（CELERIS_WU_BASE なし。Task の worktree で env 無しに流れる）: 基準は merge-base(HEAD, main)。
# commit 済みの Task ブランチの変更が空 diff にならず、main へ取り込んだ後の HEAD==main だけが空になる。
git checkout -q -- .
git checkout -q -b task
echo "// t" >>crates/a/src/lib.rs
git commit -q -am "task change"
integ() { env -u CELERIS_WU_BASE bash "$here/scripts/dev/test-affected.sh" --list 2>"$work/err"; }
[ "$(integ)" = "a
b" ] || fail "(d) integration base must not be an empty diff: got [$(integ)]"
# 基準を HEAD にした呼び出し（過去の退行: base が HEAD 自身）は空になる。基準の既定がそうならないことを上で確かめた。
[ -z "$(CELERIS_WU_BASE="$(git rev-parse HEAD)" bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null)" ] || fail "(d) explicit HEAD base should be empty"

# (e) web の型検査の選択。
git checkout -q main
echo "// new event kind" >>crates/task-core/src/lib.rs
[ "$(run)" = "task-api
task-core
web: typecheck+unit (理由: web の型の元になる crate の変更: task-core)
ts-verify: test-ts-verify (理由: runner の cache key の入力の変更)" ] || fail "(e) task-core event kind: got [$(run)]"
git checkout -q -- .
echo "// route" >>crates/task-api/src/lib.rs
run | grep -qx 'web: typecheck+unit (理由: web の型の元になる crate の変更: task-api)' || fail "(e) task-api: got [$(run)]"
git checkout -q -- .
echo '{"x":1}' >"$api"/schema.json
[ "$(run)" = "web: typecheck+unit (理由: docs/api/ の変更（生成型の元の schema）)" ] || fail "(e) docs/api: got [$(run)]"
git checkout -q -- .
# 無関係な crate（a・b）と task-api の dev 依存（c。逆依存の task-api は試験対象になるが型は変わらない）では選ばない。
echo "// x" >>crates/a/src/lib.rs
echo "// x" >>crates/c/src/lib.rs
run | grep -q '^web:' && fail "(e) unrelated crates must not select web: got [$(run)]"
git checkout -q -- .

# (f) 実行時。偽の pnpm が node_modules に偽の tsc・vitest を置く。呼ばれた順を記録する。
fake="$(dirname "$mark")/bin"
calls="$(dirname "$mark")/calls"
mkdir -p "$fake"
cat >"$fake/pnpm" <<'SH'
#!/bin/sh
echo "pnpm $*" >>"$WEB_CALLS"
mkdir -p node_modules/.bin
: >node_modules/.modules.yaml
printf '#!/bin/sh\necho "tsc $*" >>"$WEB_CALLS"\nexit "${FAKE_TSC_RC:-0}"\n' >node_modules/.bin/tsc
printf '#!/bin/sh\necho "vitest $*" >>"$WEB_CALLS"\n' >node_modules/.bin/vitest
chmod +x node_modules/.bin/tsc node_modules/.bin/vitest
SH
chmod +x "$fake/pnpm"
export WEB_CALLS="$calls"
exec_run() { CELERIS_WU_BASE="$base" PATH="$fake:$PATH" bash "$here/scripts/dev/test-affected.sh" >/dev/null 2>"$work/err"; }
echo '{"x":1}' >"$api"/schema.json
exec_run || fail "(f) web checks must pass: $(cat "$work/err")"
[ "$(cat "$calls")" = "pnpm install --offline --frozen-lockfile
tsc -b
vitest run" ] || fail "(f) install+typecheck+unit: got [$(cat "$calls")]"
# node_modules があれば install しない。
rm "$calls"
exec_run || fail "(f) second run: $(cat "$work/err")"
[ "$(cat "$calls")" = "tsc -b
vitest run" ] || fail "(f) no reinstall: got [$(cat "$calls")]"
git checkout -q -- .
# Rust の event kind だけの変更で型の不一致 → test-affected.sh が落ちる。cargo の試験より先に止まる。
rm "$calls"
echo "// new event kind" >>crates/task-core/src/lib.rs
if FAKE_TSC_RC=2 exec_run; then fail "(f) typecheck failure must fail test-affected"; fi
[ "$(cat "$calls")" = "tsc -b" ] || fail "(f) typecheck failure must stop before unit/cargo: got [$(cat "$calls")]"
grep -q 'web typecheck が失敗した (exit 2)' "$work/err" || fail "(f) no failure message: $(cat "$work/err")"
grep -q 'crates:' "$work/err" && fail "(f) cargo must not run after typecheck failure"
git checkout -q -- .

# (g) TS だけの変更と Host API・道具・golden の変更。
# cargo を失敗させる偽の cargo。TS だけの変更の --list と実行はこれを 1 回も呼ばない。
nocargo="$(dirname "$mark")/nocargo"
mkdir -p "$nocargo"
printf '#!/bin/sh\necho "cargo $*" >>"%s/calls"\nexit 97\n' "$nocargo" >"$nocargo/cargo"
chmod +x "$nocargo/cargo"
nc_run() { CELERIS_WU_BASE="$base" PATH="$nocargo:$PATH" bash "$here/scripts/dev/test-affected.sh" "$@" 2>"$work/err"; }
echo '// harmless' >>control-ts/src/domains/x.ts
echo "// new test" >control-ts/src/domains/x.test.ts
echo y >>"$d"/x.md
[ "$(nc_run --list)" = "control-ts: ts-verify (理由: TS だけの変更（control-ts/src の業務 TS）。cargo は走らせない)" ] ||
  fail "(g) ts-only --list: got [$(nc_run --list)] $(git status --short) $(cat "$work/err")"
[ ! -e "$nocargo/calls" ] || fail "(g) ts-only --list must not run cargo: $(cat "$nocargo/calls")"
# 実行: base に置いた偽の ts-verify.sh（記録を書く）に委ね、cargo を起動しない。
TSV_CLASS=ts-only nc_run >/dev/null || fail "(g) ts-only run: $(cat "$work/err")"
[ "$(cat "$TSV_CALLS")" = "ts-verify --record" ] || fail "(g) ts-only run must call ts-verify --record: got [$(cat "$TSV_CALLS")]"
[ ! -e "$nocargo/calls" ] || fail "(g) ts-only run must not run cargo: $(cat "$nocargo/calls")"
# ts-verify が ts-only と判断しなかった（何も走らせずに抜けた）・落ちたときは test-affected.sh も落ちる。
if TSV_CLASS=rust nc_run >/dev/null; then fail "(g) a ts-verify record without ts-only must fail"; fi
if TSV_CLASS=ts-only TSV_RC=1 nc_run >/dev/null; then fail "(g) a failing ts-verify must fail test-affected"; fi
rm -f control-ts/src/domains/x.test.ts
git checkout -q -- .
# 試験の削除・host.d.ts・control-ts/scripts・package.json・schema は TS だけにならず celeris-script の cargo 側を選ぶ。
for f in control-ts/src/host.d.ts control-ts/scripts/compare.mjs control-ts/package.json "$api"/script-host/host-v1.schema.json; do
  echo '// x' >>"$f"
  out="$(run)"
  printf '%s\n' "$out" | grep -qx celeris-script || fail "(g) $f must select celeris-script: got [$out]"
  printf '%s\n' "$out" | grep -q '^control-ts: typecheck+test ' || fail "(g) $f must select control-ts typecheck+test: got [$out]"
  printf '%s\n' "$out" | grep -q '^control-ts: ts-verify ' && fail "(g) $f must not be ts-only: got [$out]"
  git checkout -q -- .
done
echo 'export {};' >control-ts/src/domains/gone.test.ts
git add control-ts/src/domains/gone.test.ts
git commit -q -m "add test"
base2="$(git rev-parse HEAD)"
git rm -q control-ts/src/domains/gone.test.ts
out="$(CELERIS_WU_BASE="$base2" bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null)"
printf '%s\n' "$out" | grep -qx celeris-script || fail "(g) deleting a test must not be ts-only: got [$out]"
git reset -q --hard "$base2"
# golden（fixture）の変更は fixture を読む crate（a と逆依存 b）も選ぶ。
echo '{"x":1}' >>control-ts/fixtures/x/cases.jsonl
[ "$(CELERIS_WU_BASE="$base2" bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null | grep -v '^control-ts: ')" = "a
b
celeris-script" ] || fail "(g) fixture: got [$(CELERIS_WU_BASE="$base2" bash "$here/scripts/dev/test-affected.sh" --list 2>&1)]"
git checkout -q -- .
# 業務 TS と Rust の混在は Rust の経路（ts-verify に委ねない）。
echo '// y' >>control-ts/src/domains/x.ts
echo '// y' >>crates/c/src/lib.rs
out="$(CELERIS_WU_BASE="$base2" bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null)"
printf '%s\n' "$out" | grep -qx c || fail "(g) mixed must select crate c: got [$out]"
printf '%s\n' "$out" | grep -q '^control-ts: ts-verify ' && fail "(g) mixed must not be ts-only: got [$out]"
git checkout -q -- .
# 検証の道具の変更は test-ts-verify と celeris-script を選ぶ。
echo '# x' >scripts/dev/test-ts-verify.sh
out="$(CELERIS_WU_BASE="$base2" bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null)"
[ "$out" = "celeris-script
ts-verify: test-ts-verify (理由: 検証の道具の変更)" ] || fail "(g) grader change: got [$out]"
rm scripts/dev/test-ts-verify.sh

echo "test-affected-list: ok"
