#!/usr/bin/env bash
# TS だけの変更の高速検証経路（scripts/dev/ts-verify.sh）を本物の runner で通し、意図的な破壊を検出することを確かめる
# （ADR 2026-11-02-ts-fast-verify (5) の試験）。引数なし: bash scripts/dev/test-ts-verify.sh
# 1. 準備: runner を script-runner-cache.sh ensure で用意する（cargo を使ってよいのはここだけ）。
# 2. repo の作業ツリー（未 commit の編集を含む）を $TMPDIR の写しにし、写しで commit した基準からの編集を試す。
#    cargo を失敗させる偽の cargo を PATH の先頭に置き、ts-verify.sh はこの repo の物（写しの物ではない）を走らせる。
# 場合:
#   A 無害な TS の編集 → ts-only、exit 0、記録の cargo 起動 0・link_count 0、10 段すべて exit 0
#   B 規則を 1 件壊した TS の編集 → exit 1。同じ bundle を runner に直に渡しても mismatch で exit 1
#     （TS 側の試験・compare.mjs を通さない Rust の採点でも落ちる）
#   C 規則を壊し compare.mjs を常に成功させる → host-api（ts-only にならない）
#   D fixture を壊れた出力に合わせる → host-api、cache の golden との cmp で違いが出る
#   E package.json の typecheck・test を true にする → host-api
#   F bundle.config.json から領域を消す → host-api、その bundle を runner は exit 3 で拒否する
#   G @ts-nocheck の追加・.only( の追加 → classify で exit 1
#   H Host API（host.d.ts・schema）の変更 → host-api、test-affected.sh --list が celeris-script の cargo 側を選ぶ
#   I TS だけの変更では test-affected.sh --list が ts-verify に委ね、cargo を起動しない
#   J 規則を壊し compare.test.mjs の該当試験を test.skip にする（host-api）→ test-affected.sh --list が ts-verify-grade を選び、
#     ts-verify.sh --grade は段 golden（Rust の runner の採点）で exit 1（引数なしは「従来の経路へ」で exit 0）。
#     無害な host-api の変更（compare.mjs の注記）では ts-verify.sh --grade が 10 段を通して exit 0（ADR 付記 grade-gate）
#   K cos_digest の規則（stopped・interrupted の requeue）を壊す → ts-only で exit 1。同じ bundle を runner に直に渡すと
#     cos_digest の mismatch で exit 1。read_run の答えの検査を外した bundle は --suite safety で exit 1（ADR 付記 grade-cos-digest）
#   L pass_traces.jsonl を書き換える → host-api、cache key が変わる（古い golden で採点しない）、test-affected.sh --list が
#     trace の byte 一致を確かめる task-dispatch の cargo 側を選ぶ
# exit: 0 = すべて通った、1 = どれかが期待と違う、2 = 準備の失敗。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tsv="$here/scripts/dev/ts-verify.sh"
[ -n "${TMPDIR:-}" ] || { echo "test-ts-verify: TMPDIR が無い（写しを /tmp に置かない）" >&2; exit 2; }
work="$(mktemp -d "$TMPDIR/test-ts-verify.XXXXXX")"
trap 'chmod -R u+w "$work" 2>/dev/null || true; rm -rf "$work"' EXIT

pass=0
failed=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
ng() { failed=$((failed + 1)); echo "FAIL $*" >&2; }

# ---- 1. 準備: runner（ここだけ cargo を使う） ----
cd "$here"
runner="$(bash scripts/dev/script-runner-cache.sh ensure)" || { echo "test-ts-verify: runner の ensure が失敗した" >&2; exit 2; }
golden="$(dirname "$runner")/golden"
echo "test-ts-verify: runner $runner" >&2

# ---- 2. 写し ----
copy="$work/repo"
mkdir -p "$copy"
git ls-files -co --exclude-standard -z | python3 -I -c '
import os, sys
sys.stdout.buffer.write(b"".join(p + b"\0" for p in sys.stdin.buffer.read().split(b"\0") if p and os.path.lexists(p)))
' | tar --null -T - -cf - | tar -xf - -C "$copy"
cd "$copy"
git init -q -b main .
git config user.email t@example.com
git config user.name t
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"
export CELERIS_WU_BASE="$base"
reset() { git reset -q --hard "$base"; git clean -qfd; }

fake="$work/fakebin"
mkdir -p "$fake"
cat >"$fake/cargo" <<EOF
#!/bin/sh
echo "cargo \$*" >>"$work/fake-cargo.log"
echo "test-ts-verify: 偽の cargo（ts-only の経路は cargo を起動してはならない）" >&2
exit 97
EOF
chmod +x "$fake/cargo"
: >"$work/fake-cargo.log"
nocargo() { PATH="$fake:$PATH" "$@"; }

rec_get() { # file expr
  python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); print(eval(sys.argv[2], {}, {"r": r}))' "$1" "$2"
}
list_class() { nocargo bash "$tsv" --list 2>/dev/null | sed -n 's/^class: \([a-z-]*\).*/\1/p'; }

# lane_rules の規則表の 1 件（frontier/costly-and-unverifiable）を standard に変える。
break_rule() {
  python3 -I - control-ts/src/domains/lane_rules.ts <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'id: "frontier/costly-and-unverifiable",\n    lane: "frontier",'
assert old in s, "rule not found"
open(p, "w").write(s.replace(old, 'id: "frontier/costly-and-unverifiable",\n    lane: "standard",'))
PY
}
build_bundle() { # outdir
  TV_BUILD="$copy/control-ts/scripts/build-bundle.mjs" TV_OUT="$1" node --input-type=module -e '
    const { buildBundle } = await import(process.env.TV_BUILD);
    await buildBundle({ outDir: process.env.TV_OUT });
  ' >/dev/null
}

# ---- A 無害な TS の編集 ----
reset
printf '\n// 無害な編集（test-ts-verify の場合 A）\nexport const TEST_TS_VERIFY_HARMLESS = 1;\n' >>control-ts/src/domains/lane_rules.ts
rc=0
nocargo bash "$tsv" --record "$work/a.json" >"$work/a.log" 2>&1 || rc=$?
if [ "$rc" -eq 0 ] &&
  [ "$(rec_get "$work/a.json" 'r["class"]')" = ts-only ] &&
  [ "$(rec_get "$work/a.json" 'r["cargo_invocations"]')" = 0 ] &&
  [ "$(rec_get "$work/a.json" 'r["link_count"]')" = 0 ] &&
  [ "$(rec_get "$work/a.json" '",".join(s["name"] for s in r["stages"] if s["exit"] == 0)')" = \
    "classify,runner-cache,install,typecheck,bundle,unit,golden,safety,integration,activate" ] &&
  [ ! -s "$work/fake-cargo.log" ]; then
  ok "A 無害な TS の編集: exit 0、cargo 0 回、link 0 回、10 段 exit 0（$(rec_get "$work/a.json" 'r["total_secs"]') 秒）"
else
  ng "A 無害な TS の編集: exit $rc、記録 $(cat "$work/a.json" 2>/dev/null | tr -d '\n' | cut -c1-600)、偽 cargo $(cat "$work/fake-cargo.log")"
  tail -30 "$work/a.log" >&2
fi
# 2 回目以降の場合で install を繰り返さない（node_modules は ignore されていて reset で残る）。

# ---- B 規則を 1 件壊す ----
reset
break_rule
rc=0
nocargo bash "$tsv" --record "$work/b.json" >"$work/b.log" 2>&1 || rc=$?
failed_stage="$(rec_get "$work/b.json" '",".join(s["name"] for s in r["stages"] if s["exit"] != 0)' 2>/dev/null || true)"
if [ "$rc" -eq 1 ] && [ "$(rec_get "$work/b.json" 'r["class"]')" = ts-only ] && [ -n "$failed_stage" ] &&
  [ "$(rec_get "$work/b.json" 'r["cargo_invocations"]')" = 0 ]; then
  ok "B 壊した規則: exit 1（段 $failed_stage で失敗）、cargo 0 回"
else
  ng "B 壊した規則: exit $rc（期待 1）、失敗の段 [$failed_stage]"
  tail -30 "$work/b.log" >&2
fi
# 同じ壊れた bundle を Rust の採点（runner）に直に渡す。TS 側の試験・compare.mjs を通さなくても落ちる。
build_bundle "$work/b-dist"
rc=0
"$runner" run --bundle-dir "$work/b-dist" --fixtures "$golden" --suite golden >"$work/b-run.json" 2>"$work/b-run.err" || rc=$?
mism="$(rec_get "$work/b-run.json" 'sum(d["mismatch"] for d in r["domains"])' 2>/dev/null || echo "?")"
if [ "$rc" -eq 1 ] && [ "$mism" != "?" ] && [ "$mism" -gt 0 ]; then
  ok "B runner に直に渡した壊れた bundle: exit 1、mismatch $mism 件"
else
  ng "B runner: exit $rc（期待 1）、mismatch $mism"
fi

# ---- C 規則を壊し compare.mjs を常に成功させる ----
reset
break_rule
printf 'process.exit(0);\n' >control-ts/scripts/compare.mjs
c_class="$(list_class)"
if [ "$c_class" = host-api ]; then ok "C compare.mjs の書き換え: host-api（ts-only にならない）"; else ng "C compare.mjs: class [$c_class]"; fi

# ---- D fixture を壊れた出力に合わせる ----
reset
python3 -I - control-ts/fixtures/lane_rules/all-features.jsonl <<'PY'
import sys
p = sys.argv[1]
lines = open(p).read().split("\n")
for i, l in enumerate(lines):
    if '"frontier/costly-and-unverifiable"' in l:
        lines[i] = l.replace('"proposed":"frontier"', '"proposed":"standard"')
open(p, "w").write("\n".join(lines))
PY
d_class="$(list_class)"
if [ "$d_class" = host-api ] && ! cmp -s control-ts/fixtures/lane_rules/all-features.jsonl "$golden/lane_rules/all-features.jsonl"; then
  ok "D fixture の書き換え: host-api、cache の golden と cmp で違う"
else
  ng "D fixture: class [$d_class]（または golden と同じまま）"
fi

# ---- E package.json の scripts を true にする ----
reset
python3 -I - control-ts/package.json <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["scripts"]["typecheck"] = "true"
d["scripts"]["test"] = "true"
open(p, "w").write(json.dumps(d, indent=2) + "\n")
PY
e_class="$(list_class)"
if [ "$e_class" = host-api ]; then ok "E package.json の scripts: host-api"; else ng "E package.json: class [$e_class]"; fi

# ---- F bundle.config.json から領域を消す ----
reset
python3 -I - control-ts/bundle.config.json <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
del d["domains"]["route_priority"]
open(p, "w").write(json.dumps(d, indent=2) + "\n")
PY
f_class="$(list_class)"
rc=0
build_bundle "$work/f-dist" 2>"$work/f-build.err" || rc=$?
if [ "$rc" -eq 0 ]; then
  rc=0
  "$runner" run --bundle-dir "$work/f-dist" --fixtures "$golden" >"$work/f-run.json" 2>/dev/null || rc=$?
else
  rc="build-$rc"
fi
if [ "$f_class" = host-api ] && { [ "$rc" = 3 ] || [ "${rc#build-}" != "$rc" ]; }; then
  ok "F bundle.config.json の領域の削除: host-api、bundle は通らない（runner exit $rc）"
else
  ng "F bundle.config.json: class [$f_class]、runner exit $rc（期待 3）"
fi

# ---- G 追加の禁止 ----
reset
printf '// @ts-nocheck\n' | cat - control-ts/src/domains/lane_rules.ts >"$work/g.ts"
cp "$work/g.ts" control-ts/src/domains/lane_rules.ts
rc=0
nocargo bash "$tsv" >"$work/g1.log" 2>&1 || rc=$?
if [ "$rc" -eq 1 ] && grep -q '追加の禁止' "$work/g1.log"; then ok "G @ts-nocheck の追加: classify で exit 1"; else ng "G @ts-nocheck: exit $rc"; fi
reset
sed -i '0,/\bit(/s//it.only(/' control-ts/src/domains/lane_rules.test.ts
if git diff --quiet; then sed -i '0,/\btest(/s//test.only(/' control-ts/src/domains/lane_rules.test.ts; fi
rc=0
nocargo bash "$tsv" >"$work/g2.log" 2>&1 || rc=$?
if ! git diff --quiet && [ "$rc" -eq 1 ] && grep -q '追加の禁止' "$work/g2.log"; then
  ok "G .only( の追加: classify で exit 1"
else
  ng "G .only(: exit $rc（差分 $(git diff --stat | tail -1)）"
fi

# ---- H Host API の変更 → cargo 側 ----
for f in control-ts/src/host.d.ts docs/api/script-host/host-v1.schema.json; do
  reset
  printf '\n' >>"$f"
  h_class="$(list_class)"
  rc=0
  nocargo bash "$tsv" >"$work/h.log" 2>&1 || rc=$?
  ta="$(bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null || true)"
  if [ "$h_class" = host-api ] && [ "$rc" -eq 0 ] && grep -q '従来の経路へ' "$work/h.log" &&
    printf '%s\n' "$ta" | grep -qx celeris-script && ! printf '%s\n' "$ta" | grep -q '^control-ts: ts-verify '; then
    ok "H $f: host-api、ts-verify は従来の経路へ、test-affected は celeris-script の cargo 側を選ぶ"
  else
    ng "H $f: class [$h_class]、ts-verify exit $rc、test-affected [$(printf '%s' "$ta" | tr '\n' ' ')]"
  fi
done

# ---- I TS だけの変更では test-affected.sh が ts-verify に委ね、cargo を起動しない ----
reset
printf '\n// 無害な編集（test-ts-verify の場合 I）\n' >>control-ts/src/domains/route_priority.ts
: >"$work/fake-cargo.log"
ta="$(nocargo bash "$here/scripts/dev/test-affected.sh" --list 2>"$work/i.err" || echo "exit $?")"
if [ "$ta" = "control-ts: ts-verify (理由: TS だけの変更（control-ts/src の業務 TS）。cargo は走らせない)" ] && [ ! -s "$work/fake-cargo.log" ]; then
  ok "I test-affected.sh --list: TS だけの変更を ts-verify に委ね、cargo 0 回"
else
  ng "I test-affected.sh --list: [$ta] 偽 cargo [$(cat "$work/fake-cargo.log")] $(cat "$work/i.err")"
fi

# ---- J host-api の候補を --grade で採点する（reviewer の破壊: 規則の破壊と test.skip） ----
reset
break_rule
python3 -I - control-ts/scripts/compare.test.mjs <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'test("lane_rules matches every line of the committed fixture"'
assert old in s, "test not found"
open(p, "w").write(s.replace(old, 'test.skip("lane_rules matches every line of the committed fixture"'))
PY
j_class="$(list_class)"
# --list は crate の選択に cargo metadata を使う（H と同じく本物の cargo）。
ta="$(bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null || true)"
rc_plain=0
nocargo bash "$tsv" >"$work/j0.log" 2>&1 || rc_plain=$?
rc=0
nocargo bash "$tsv" --grade --record "$work/j.json" >"$work/j.log" 2>&1 || rc=$?
j_failed="$(rec_get "$work/j.json" '",".join(s["name"] for s in r["stages"] if s["exit"] != 0)' 2>/dev/null || true)"
j_passed="$(rec_get "$work/j.json" '",".join(s["name"] for s in r["stages"] if s["exit"] == 0)' 2>/dev/null || true)"
if [ "$j_class" = host-api ] && printf '%s\n' "$ta" | grep -q '^control-ts: ts-verify-grade (理由: ' &&
  [ "$rc_plain" -eq 0 ] && grep -q '従来の経路へ' "$work/j0.log" &&
  [ "$rc" -eq 1 ] && [ "$(rec_get "$work/j.json" 'r["class"]')" = host-api ] && [ "$(rec_get "$work/j.json" 'r["grade"]')" = True ] &&
  [ "$j_failed" = golden ] && [ "$j_passed" = "classify,runner-cache,install,typecheck,bundle,unit" ]; then
  ok "J 規則の破壊と test.skip: host-api、test-affected は ts-verify-grade を選び、--grade は段 golden（runner）で exit 1（段 unit は通った）"
else
  ng "J 規則の破壊と test.skip: class [$j_class]、引数なし exit $rc_plain、--grade exit $rc、通った段 [$j_passed]、失敗の段 [$j_failed]、test-affected [$(printf '%s' "$ta" | tr '\n' ' ')]"
  tail -30 "$work/j.log" >&2
fi
reset
printf '\n// 無害な編集（test-ts-verify の場合 J）\n' >>control-ts/scripts/compare.mjs
rc=0
nocargo bash "$tsv" --grade --record "$work/j2.json" >"$work/j2.log" 2>&1 || rc=$?
if [ "$rc" -eq 0 ] && [ "$(rec_get "$work/j2.json" 'r["class"]')" = host-api ] &&
  [ "$(rec_get "$work/j2.json" '",".join(s["name"] for s in r["stages"] if s["exit"] == 0)')" = \
    "classify,runner-cache,install,typecheck,bundle,unit,golden,safety,integration,activate" ]; then
  ok "J 無害な host-api の変更: --grade は 10 段 exit 0（cargo $(rec_get "$work/j2.json" 'r["cargo_invocations"]') 回）"
else
  ng "J 無害な host-api の変更: --grade exit $rc、記録 $(tr -d '\n' <"$work/j2.json" 2>/dev/null | cut -c1-600)"
  tail -30 "$work/j2.log" >&2
fi

# ---- K cos_digest の規則を壊す ----
reset
python3 -I - control-ts/src/domains/cos_digest.ts <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'return runState === "stopped" || runState === "interrupted";'
assert old in s, "rule not found"
open(p, "w").write(s.replace(old, 'return runState === "stopped";'))
PY
rc=0
nocargo bash "$tsv" --record "$work/k.json" >"$work/k.log" 2>&1 || rc=$?
k_failed="$(rec_get "$work/k.json" '",".join(s["name"] for s in r["stages"] if s["exit"] != 0)' 2>/dev/null || true)"
if [ "$rc" -eq 1 ] && [ "$(rec_get "$work/k.json" 'r["class"]')" = ts-only ] && [ -n "$k_failed" ]; then
  ok "K 壊した cos_digest の規則: exit 1（段 $k_failed で失敗）"
else
  ng "K 壊した cos_digest の規則: exit $rc（期待 1）、失敗の段 [$k_failed]"
  tail -30 "$work/k.log" >&2
fi
build_bundle "$work/k-dist"
rc=0
"$runner" run --bundle-dir "$work/k-dist" --fixtures "$golden" --suite golden --domain cos_digest >"$work/k-run.json" 2>"$work/k-run.err" || rc=$?
k_mism="$(rec_get "$work/k-run.json" 'r["domains"][0]["mismatch"]' 2>/dev/null || echo "?")"
k_rows="$(rec_get "$work/k-run.json" 'r["domains"][0]["rows"]' 2>/dev/null || echo "?")"
if [ "$rc" -eq 1 ] && [ "$k_mism" != "?" ] && [ "$k_mism" -gt 0 ] && [ "$k_rows" -ge 2086 ]; then
  ok "K runner に直に渡した壊れた bundle: exit 1、cos_digest の mismatch $k_mism 件 / $k_rows 行"
else
  ng "K runner: exit $rc（期待 1）、mismatch $k_mism、rows $k_rows"
fi
reset
python3 -I - control-ts/src/domains/cos_digest.ts <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'if (input.thread !== ref.thread || input.run_id !== ref.run_id) {'
assert old in s, "check not found"
open(p, "w").write(s.replace(old, 'if (false) {'))
PY
build_bundle "$work/k2-dist"
rc=0
"$runner" run --bundle-dir "$work/k2-dist" --fixtures "$golden" --suite safety --domain cos_digest >"$work/k2-run.json" 2>"$work/k2-run.err" || rc=$?
k2_out="$(rec_get "$work/k2-run.json" '[s["outcome"] for s in r["safety"] if s["id"] == "cos_digest/read_reply_other_run"][0]' 2>/dev/null || echo "?")"
if [ "$rc" -eq 1 ] && [ "$k2_out" = returned_output ]; then
  ok "K 壊した op の応答を受ける bundle: --suite safety で exit 1"
else
  ng "K safety: exit $rc（期待 1）、read_reply_other_run の結果 [$k2_out]"
fi

# ---- L pass_traces.jsonl の書き換え ----
reset
key_base="$(bash "$here/scripts/dev/script-runner-cache.sh" key)"
sed -i '1s/人に回した/答えた/' control-ts/fixtures/cos_digest/pass_traces.jsonl
l_class="$(list_class)"
key_new="$(bash "$here/scripts/dev/script-runner-cache.sh" key)"
ta="$(bash "$here/scripts/dev/test-affected.sh" --list 2>/dev/null || true)"
if ! git diff --quiet && [ "$l_class" = host-api ] && [ "$key_base" != "$key_new" ] &&
  printf '%s\n' "$ta" | grep -qx task-dispatch && printf '%s\n' "$ta" | grep -q '^control-ts: ts-verify-grade '; then
  ok "L pass_traces.jsonl の書き換え: host-api、key が変わる、test-affected は task-dispatch と ts-verify-grade を選ぶ"
else
  ng "L pass_traces.jsonl: class [$l_class]、key [$key_base → $key_new]、test-affected [$(printf '%s' "$ta" | tr '\n' ' ')]"
fi

echo "test-ts-verify: $pass passed, $failed failed"
[ "$failed" -eq 0 ]
