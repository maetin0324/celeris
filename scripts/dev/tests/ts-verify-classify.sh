#!/usr/bin/env bash
# scripts/dev/ts-verify.sh の分類と段の並びを、小さな git fixture・偽の cargo・偽の node と tsc・vitest・偽の runner で確かめる
# （ADR 2026-11-02-ts-fast-verify (1)(2)）。確かめること:
#   - 業務 TS（control-ts/src/**/*.ts）だけの変更は ts-only、文書だけは none、文書が混ざっても ts-only のまま
#   - fixture・control-ts/scripts・設定・lockfile・src/testing・試験の削除・host の契約は host-api（ts-only にならない）
#   - crates/・scripts/ などは rust
#   - ts-only の実行は 10 段を順に走らせ、cargo を 1 回も呼ばない（記録の cargo_invocations 0・link_count 0）
#   - 追加の禁止（@ts-ignore・.only(）は exit 1、--no-build で runner が無ければ exit 3、runner の失敗は exit 1
# 本物の node・cargo・runner は使わない（本物の runner での試験は wire unit の scripts/dev/test-ts-verify.sh）。
set -euo pipefail

src="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/ts-verify.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/ts-verify-classify.XXXXXX")"
trap 'rm -rf "$work"' EXIT
repo="$work/repo"
fake="$work/fake"
calls="$work/calls"
cache="$work/cache/k"
mkdir -p "$repo" "$fake" "$cache/golden/lane_rules"
export CALLS="$calls"

# 文書の path は doc-links の検査が実在の path と見なさないよう変数で組む。
ad=agent-docs
dd=docs
fail=0
pass=0
check() { # 説明 条件コマンド...
  local what="$1"
  shift
  if "$@"; then pass=$((pass + 1)); else echo "FAIL: $what" >&2; fail=$((fail + 1)); fi
}

# ---- 偽の道具（PATH の先頭）----
cat >"$fake/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >>"$CALLS"
exit 101
EOF
# 偽の node: bundle の段では out dir に 2 file を置く。他は呼ばれたことだけを残す。
cat >"$fake/node" <<'EOF'
#!/bin/sh
echo "node $*" >>"$CALLS"
if [ -n "${TS_VERIFY_OUT:-}" ]; then
  mkdir -p "$TS_VERIFY_OUT"
  : >"$TS_VERIFY_OUT/bundle.mjs"
  echo '{}' >"$TS_VERIFY_OUT/manifest.json"
fi
exit 0
EOF
cat >"$fake/pnpm" <<'EOF'
#!/bin/sh
echo "pnpm $*" >>"$CALLS"
exit 1
EOF
chmod +x "$fake"/*
export PATH="$fake:$PATH"

# 偽の runner と golden（cache の dir は repo の外）。FAKE_RUNNER_FAIL=<suite> でその suite を失敗させる。
cat >"$cache/script-host-runner" <<'EOF'
#!/bin/sh
echo "runner $*" >>"$CALLS"
case "$1" in
  domains) echo '{"domains":[{"domain":"lane_rules","fixture":"lane_rules/all-features.jsonl"}]}' ;;
  run)
    suite=""
    while [ "$#" -gt 0 ]; do [ "$1" = --suite ] && suite="$2"; shift; done
    if [ "${FAKE_RUNNER_FAIL:-}" = "$suite" ]; then echo '{"command":"run","ok":false,"complete":true}'; exit 1; fi
    echo '{"command":"run","ok":true,"complete":true}' ;;
  activate) echo '{"command":"activate","ok":true}' ;;
  *) exit 2 ;;
esac
EOF
chmod +x "$cache/script-host-runner"
echo '{"features":{},"rule_id":"r","proposed":"standard","condition":"c"}' >"$cache/golden/lane_rules/all-features.jsonl"

# ---- fixture の repo ----
cd "$repo"
git init -q -b main .
git config user.email t@example.com
git config user.name t
w() { mkdir -p "$(dirname "$1")"; printf '%s\n' "$2" >"$1"; }
mkdir -p scripts/dev
cp "$src" scripts/dev/ts-verify.sh
# 偽の cache script: path は偽の runner を返す（FAKE_CACHE_MISS=1 で無い扱い）。ensure は呼ばれたら失敗。
cat >scripts/dev/script-runner-cache.sh <<EOF
#!/bin/sh
echo "cache \$*" >>"\$CALLS"
case "\$1" in
  path) [ -z "\${FAKE_CACHE_MISS:-}" ] || exit 1; echo "$cache/script-host-runner" ;;
  *) exit 1 ;;
esac
EOF
w .gitignore "node_modules"
w "$ad/note.md" "# note"
w "$dd/guide.md" "# guide"
w control-ts/package.json '{"scripts":{"typecheck":"tsc --noEmit","test":"vitest run"}}'
w control-ts/pnpm-lock.yaml "lockfileVersion: '9.0'"
w control-ts/tsconfig.json '{}'
w control-ts/bundle.config.json '{"domains":{"lane_rules":[]}}'
w control-ts/vitest.config.mjs 'export default {};'
w control-ts/src/main.ts 'export { laneRules } from "./domains/lane_rules";'
w control-ts/src/host.d.ts 'declare module "celeris:host" {}'
w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => x;'
w control-ts/src/domains/lane_rules.test.ts 'import { test } from "vitest"; test("x", () => {});'
w control-ts/src/testing/host-stub.ts 'export {};'
w control-ts/scripts/compare.mjs 'process.exit(0);'
mkdir -p control-ts/fixtures/lane_rules
cp "$cache/golden/lane_rules/all-features.jsonl" control-ts/fixtures/lane_rules/all-features.jsonl
w docs/api/script-host/host-v1.schema.json '{}'
w crates/script-runtime/src/ops.rs '// ops'
w crates/task-core/src/lib.rs '// lib'
w crates/task-core/examples/dump-lane-fixtures.rs '// writes control-ts/fixtures/lane_rules/all-features.jsonl'
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"
export CELERIS_WU_BASE="$base"
# node_modules は git の外（install の段を飛ばす）。偽の tsc と vitest。
mkdir -p control-ts/node_modules/.bin
: >control-ts/node_modules/.modules.yaml
for t in tsc vitest; do
  printf '#!/bin/sh\necho "%s $*" >>"$CALLS"\nexit 0\n' "$t" >"control-ts/node_modules/.bin/$t"
  chmod +x "control-ts/node_modules/.bin/$t"
done

reset() {
  git reset -q --hard "$base"
  git clean -qfd -e control-ts/node_modules
  rm -f "$calls"
  : >"$calls"
}
list() { bash scripts/dev/ts-verify.sh --list 2>&1; }
class_is() { # want
  local got
  got="$(list | sed -n 's/^class: \([a-z-]*\).*/\1/p')"
  [ "$got" = "$1" ] || { echo "  class: want $1, got $got" >&2; list >&2; return 1; }
}
no_cargo() { ! grep -q '^cargo' "$calls"; }

# ---- 分類 ----
reset; w "$ad/note.md" "# changed"; w "$dd/guide.md" "# changed"
check "文書だけは none" class_is none

reset; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => ({ x });'
check "業務 TS の変更は ts-only" class_is ts-only
check "--list は 10 段と cargo 0 回を出す" bash -c 'bash scripts/dev/ts-verify.sh --list | grep -q "^stages: classify runner-cache install typecheck bundle unit golden safety integration activate（cargo 0 回）"'
check "--list は file ごとの理由を出す" bash -c 'bash scripts/dev/ts-verify.sh --list | grep -q "ts-only  M control-ts/src/domains/lane_rules.ts  — 業務 TS"'
w "$ad/note.md" "# progress"
check "文書が混ざっても ts-only" class_is ts-only
w control-ts/src/domains/new_rule.ts 'export const x = 1;'
check "未追跡の業務 TS も ts-only" class_is ts-only

reset; w control-ts/src/domains/lane_rules.test.ts 'import { test } from "vitest"; test("y", () => {});'
check "単体試験の変更は ts-only" class_is ts-only

host_api_cases=(
  "control-ts/fixtures/lane_rules/all-features.jsonl"
  "control-ts/scripts/compare.mjs"
  "control-ts/package.json"
  "control-ts/pnpm-lock.yaml"
  "control-ts/tsconfig.json"
  "control-ts/vitest.config.mjs"
  "control-ts/bundle.config.json"
  "control-ts/src/testing/host-stub.ts"
  "control-ts/src/host.d.ts"
  "docs/api/script-host/host-v1.schema.json"
  "crates/script-runtime/src/ops.rs"
  "control-ts/README.md"
)
for f in "${host_api_cases[@]}"; do
  reset; echo "// changed" >>"$f"; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => [x];'
  check "$f の変更は（業務 TS と一緒でも）host-api" class_is host-api
done

reset; git rm -q control-ts/src/domains/lane_rules.test.ts
check "単体試験の削除は host-api" class_is host-api
reset; git mv control-ts/src/domains/lane_rules.test.ts control-ts/src/domains/other.test.ts
check "単体試験の rename（旧い側の削除）は host-api" class_is host-api

reset; echo "// x" >>control-ts/fixtures/lane_rules/all-features.jsonl
check "fixture の変更で fixture を書く crate を挙げる" bash -c 'bash scripts/dev/ts-verify.sh --list | grep -q "fixture を書く・読む crate.*task-core"'
reset; echo "{}" >>docs/api/script-host/host-v1.schema.json
check "schema の変更で cache key の入力の変化を挙げる" bash -c 'bash scripts/dev/ts-verify.sh --list | grep -q "cache key の入力が変わった"'

for f in crates/task-core/src/lib.rs scripts/dev/test-affected.sh scripts/dev/other.sh Cargo.lock web/src/a.ts; do
  reset; w "$f" "// changed"; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => [x];'
  check "$f の変更は rust" class_is rust
done

# ---- 追加の禁止 ----
reset; printf '// @ts-ignore\nexport const laneRules = (x: unknown) => x;\n' >control-ts/src/domains/lane_rules.ts
check "@ts-ignore の追加は --list が exit 1" bash -c '! bash scripts/dev/ts-verify.sh --list >/dev/null'
check "@ts-ignore の追加は実行が exit 1" bash -c 'bash scripts/dev/ts-verify.sh >/dev/null 2>&1; [ $? -eq 1 ]'
check "@ts-ignore の追加で cargo を呼ばない" no_cargo
reset; w control-ts/src/domains/lane_rules.test.ts 'import { test } from "vitest"; test.only("x", () => {});'
check ".only( の追加は exit 1" bash -c 'bash scripts/dev/ts-verify.sh >/dev/null 2>&1; [ $? -eq 1 ]'

# ---- ts-only の実行 ----
reset; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => ({ x });'
rec="$work/rec.json"
check "ts-only の実行は exit 0" bash -c "bash scripts/dev/ts-verify.sh --record '$rec' >/dev/null 2>&1"
check "ts-only の実行で cargo を呼ばない" no_cargo
check "記録: class ts-only・cargo 0・link 0・10 段すべて exit 0" python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
names = [s["name"] for s in r["stages"]]
want = "classify runner-cache install typecheck bundle unit golden safety integration activate".split()
assert r["class"] == "ts-only" and r["exit"] == 0, r
assert r["cargo_invocations"] == 0 and r["cargo_compile_invocations"] == 0 and r["link_count"] == 0, r
assert names == want, names
assert all(s["exit"] == 0 for s in r["stages"]), r["stages"]
' "$rec"
check "型検査・単体・runner の 3 suite を呼ぶ" bash -c "
  grep -q '^tsc --noEmit' '$calls' && grep -q '^vitest run' '$calls' && grep -q '^node --test' '$calls' &&
  grep -q '^node .*compare.mjs lane_rules --fixture $cache/golden/lane_rules/all-features.jsonl' '$calls' &&
  grep -q '^runner run .*--suite safety' '$calls' && grep -q '^runner run .*--suite golden' '$calls' &&
  grep -q '^runner activate' '$calls' && grep -q '^runner domains' '$calls'"
check "bundle は control-ts/dist でなく使い捨て dir に作る" bash -c "! grep -q 'control-ts/dist' '$calls' && grep -q 'runner run --bundle-dir ${TMPDIR:-/tmp}/ts-verify\.' '$calls'"
check "install の段は node_modules があれば pnpm を呼ばない" bash -c "! grep -q '^pnpm' '$calls'"

# golden が commit 済みの fixture と違えば golden の段で失敗する。
reset; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => ({ x });'
cp "$cache/golden/lane_rules/all-features.jsonl" "$work/golden.bak"
echo '{"changed":true}' >>"$cache/golden/lane_rules/all-features.jsonl"
check "golden と fixture の不一致は exit 1" bash -c 'bash scripts/dev/ts-verify.sh >/dev/null 2>&1; [ $? -eq 1 ]'
cp "$work/golden.bak" "$cache/golden/lane_rules/all-features.jsonl"

# runner の失敗は exit 1、記録はその段で止まる。
reset; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => ({ x });'
check "runner の safety の失敗は exit 1" bash -c "FAKE_RUNNER_FAIL=safety bash scripts/dev/ts-verify.sh --record '$rec' >/dev/null 2>&1; [ \$? -eq 1 ]"
check "記録は safety の失敗で止まる" python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
assert r["exit"] == 1 and r["stages"][-1]["name"] == "safety" and r["stages"][-1]["exit"] != 0, r
' "$rec"

# --no-build で runner が無ければ exit 3（ensure も cargo も呼ばない）。
reset; w control-ts/src/domains/lane_rules.ts 'export const laneRules = (x: unknown) => ({ x });'
check "--no-build で runner が無ければ exit 3" bash -c 'FAKE_CACHE_MISS=1 bash scripts/dev/ts-verify.sh --no-build >/dev/null 2>&1; [ $? -eq 3 ]'
check "--no-build では ensure と cargo を呼ばない" bash -c "! grep -q '^cache ensure' '$calls' && ! grep -q '^cargo' '$calls'"

# host-api・rust は段を走らせず従来の経路へ（exit 0、cargo も runner も呼ばない）。
reset; echo "// x" >>control-ts/scripts/compare.mjs
check "host-api の実行は exit 0 で従来の経路へ" bash -c 'bash scripts/dev/ts-verify.sh 2>&1 | grep -q "従来の経路へ"'
check "host-api の実行で runner と cargo を呼ばない" bash -c "! grep -qE '^(runner|cargo|tsc)' '$calls'"

echo "ts-verify-classify: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
