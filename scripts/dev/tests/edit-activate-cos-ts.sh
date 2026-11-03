#!/usr/bin/env bash
# scripts/dev/edit-activate-baseline.sh の scenario cos-ts・cos-digest（ADR 2026-11-02-ts-control-layer-boundary 付記 S5-10・S5-11）を
# 確かめる。引数なし: bash scripts/dev/tests/edit-activate-cos-ts.sh
# 本物の計測はしない。git fixture に本物の cos_digest.ts・digest.rs の写しを置き、偽の ts-verify・偽の runner cache・偽の cargo で
# 試行の流れ（anchor が 1 回だけ在る・編集が当たって戻る・--rep-start・env.identity・trial_load・over_300s）と
# --validate・--check-targets の新しい欄の検査を確かめる。本物の cargo の compile・link は起動しない。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
src="$root/scripts/dev/edit-activate-baseline.sh"
real_verify="$root/scripts/dev/ts-verify.sh"
ts_file="$root/control-ts/src/domains/cos_digest.ts"
rs_file="$root/crates/task-core/src/chat/digest.rs"
[ -n "${TMPDIR:-}" ] || { echo "edit-activate-cos-ts: TMPDIR が無い" >&2; exit 2; }
work="$(mktemp -d "$TMPDIR/edit-activate-cos-ts.XXXXXX")"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); }
ng() { fail=$((fail + 1)); echo "FAIL: $*" >&2; }
# 説明 期待（数値か nonzero） コマンド...
expect() {
  local what="$1" want="$2" rc=0
  shift 2
  "$@" >"$work/last.out" 2>"$work/last.err" || rc=$?
  case "$want" in
    nonzero) if [ "$rc" -ne 0 ]; then ok; else ng "$what: exit 0（非 0 を期待）"; fi ;;
    *) if [ "$rc" -eq "$want" ]; then ok; else ng "$what: exit $rc（期待 $want）: $(cut -c1-600 "$work/last.err")"; fi ;;
  esac
}
check_py() { # 説明 file python（sys.argv[1] が file。assert で落ちれば ng）
  local what="$1" file="$2"
  if python3 -I -c "$3" "$file" 2>"$work/py.err"; then ok; else ng "$what: $(cut -c1-600 "$work/py.err")"; fi
}

# ---- 偽の cargo: 版と config の読み取りに答え、build 系は呼ばれた引数と編集の有無を記録する ----
# FAKE_CARGO_OK=1 のときだけ build 系を成功させる（cos-digest の流れ）。FAKE_CARGO_FAIL=<引数> はその引数を含む呼び出しを落とす。
fake="$work/fakebin"
mkdir -p "$fake"
cat >"$fake/cargo" <<EOF
#!/bin/sh
case "\$1" in
  -V|--version) echo "cargo 0.0.0 (fake)"; exit 0 ;;
  config) exit 1 ;;
esac
edit=no
if [ -f crates/task-core/src/chat/digest.rs ] && grep -q 'edit-activate-baseline-' crates/task-core/src/chat/digest.rs; then edit=yes; fi
echo "edit=\$edit target=\${CARGO_TARGET_DIR:-} cargo \$*" >>"$work/fake-cargo.log"
[ "\${FAKE_CARGO_OK:-}" = 1 ] || exit 97
case " \$* " in
  *" \${FAKE_CARGO_FAIL:-no-such-arg} "*) exit 101 ;;
esac
exit 0
EOF
chmod +x "$fake/cargo"
: >"$work/fake-cargo.log"
export PATH="$fake:$PATH"

# ---- 1. --dry-run ----
before="$(sha256sum "$ts_file" "$rs_file")"
expect "cos-ts の dry-run" 0 bash "$src" --dry-run --scenario cos-ts
for word in cos_digest.ts ts-verify.sh typecheck bundle golden safety integration activate isolate cos-digest \
  trial_load over_300s bundle_sha256 host_api runner_sha256 "tree=HEAD:control-ts" "--rep-start=1"; do
  if grep -qF -- "$word" "$work/last.out"; then ok; else ng "cos-ts の dry-run に「$word」が無い"; fi
done
expect "cos-digest の dry-run（--rep-start 11 --series m-cosd-b）" 0 bash "$src" --dry-run --scenario cos-digest --rep-start 11 --series m-cosd-b
for word in digest.rs cos_digest_decide "cargo check" "-p task-core -p task-dispatch" "--no-run" "--release -p celeris" \
  "--rep-start=11" "series=m-cosd-b" "crates/task-core crates/task-dispatch" trial_secs over_300s trial_load; do
  if grep -qF -- "$word" "$work/last.out"; then ok; else ng "cos-digest の dry-run に「$word」が無い"; fi
done
if [ "$before" = "$(sha256sum "$ts_file" "$rs_file")" ] && [ ! -s "$work/fake-cargo.log" ]; then ok; else
  ng "dry-run が file を変えたか cargo を呼んだ"
fi
expect "--rep-start 0 は使い方の誤り" 2 bash "$src" --dry-run --scenario cos-ts --rep-start 0
expect "--rep-start が数でない" 2 bash "$src" --dry-run --scenario cos-digest --rep-start x
expect "空の --series" 2 bash "$src" --dry-run --scenario cos-digest --series ""
for sc in policy cos policy-ts; do
  expect "既存の scenario $sc の dry-run は残る" 0 bash "$src" --dry-run --scenario "$sc"
done

# ---- 2. anchor が本物の file に 1 回だけ在る（編集を当てる script と同じ文字列） ----
ts_anchor='  const { text, cards } = digestMessage(input.run_id, input.run_state, input.reason, input.items);'
rs_anchor='    let (text, cards) = cos_digest_message('
if [ "$(grep -cxF -- "$ts_anchor" "$ts_file")" = 1 ]; then ok; else ng "cos_digest.ts の anchor が 1 回だけでない"; fi
if [ "$(grep -cxF -- "$rs_anchor" "$rs_file")" = 1 ]; then ok; else ng "digest.rs の anchor が 1 回だけでない"; fi
if grep -qF "$ts_anchor" "$src" && grep -qF "$rs_anchor" "$src"; then ok; else ng "script の anchor が試験と違う"; fi

# ---- 3. git fixture ----
fx="$work/repo"
mkdir -p "$fx/scripts/dev" "$fx/control-ts/src/domains" "$fx/crates/task-core/src/chat" "$fx/crates/task-dispatch/src"
cp "$src" "$fx/scripts/dev/edit-activate-baseline.sh"
cp "$ts_file" "$fx/control-ts/src/domains/cos_digest.ts"
cp "$rs_file" "$fx/crates/task-core/src/chat/digest.rs"
echo '// fixture' >"$fx/crates/task-dispatch/src/lib.rs"
printf 'node_modules/\n' >"$fx/.gitignore"
# --list は本物の分類。実行は偽物: 基準が HEAD で、cos-ts の編集（当たらない分岐）が当たっていることを確かめて記録を書く。
cat >"$fx/scripts/dev/ts-verify.sh" <<EOF
#!/usr/bin/env bash
set -eu
if [ "\${1:-}" = --list ]; then exec bash "$real_verify" "\$@"; fi
[ "\$1" = --record ] || exit 2
record="\$2"
echo "base=\$CELERIS_WU_BASE keep=\${TS_VERIFY_KEEP_WORK:-}" >>"$work/verify-calls"
[ "\$CELERIS_WU_BASE" = HEAD ] || exit 2
grep -q 'input.reason.includes("edit-activate-baseline-' control-ts/src/domains/cos_digest.ts || exit 2
grep -q 'return { commands: \[\] };' control-ts/src/domains/cos_digest.ts || exit 2
rc="\${FAKE_VERIFY_RC:-0}"
python3 -I - "\$record" "\$rc" <<'PY'
import json, sys
names = ["classify","runner-cache","install","typecheck","bundle","unit","golden","safety","integration","activate"]
rc = int(sys.argv[2])
stages = [{"name": n, "secs": 0.25 if n == "typecheck" else 0.5 if n == "bundle" else 0.01, "exit": 0} for n in names]
if rc:
    stages = stages[:6] + [{"name": "golden", "secs": 0.1, "exit": 1}]
json.dump({"schema": "celeris.ts-verify-record/1", "class": "ts-only", "exit": rc, "total_secs": 1.0, "stages": stages,
           "cargo_invocations": 0, "cargo_compile_invocations": 0, "cargo_calls": [], "link_count": 0, "units": ["ALL"]},
          open(sys.argv[1], "w"))
PY
if [ "\$rc" = 0 ] && [ -n "\${TS_VERIFY_KEEP_WORK:-}" ]; then
  d="\$(mktemp -d "\$TMPDIR/ts-verify.XXXXXX")"
  echo '{"command":"activate","ok":true,"bundle_sha256":"cafebabe","host_api":1}' >"\$d/activate.json"
fi
exit "\$rc"
EOF
# 偽の runner cache: path は在る file を指す（runner_sha256 を取れる）
echo 'fake runner' >"$work/fake-runner"
cat >"$fx/scripts/dev/script-runner-cache.sh" <<EOF
#!/bin/sh
case "\$1" in ensure|path) echo "$work/fake-runner" ;; *) exit 2 ;; esac
EOF
(
  cd "$fx"
  git init -q -b main .
  git config user.email t@example.com
  git config user.name t
  git add -A
  git commit -q -m base
)
ts_orig="$(sha256sum <"$fx/control-ts/src/domains/cos_digest.ts")"
rs_orig="$(sha256sum <"$fx/crates/task-core/src/chat/digest.rs")"
restored() { [ "$ts_orig" = "$(sha256sum <"$fx/control-ts/src/domains/cos_digest.ts")" ] &&
  [ "$rs_orig" = "$(sha256sum <"$fx/crates/task-core/src/chat/digest.rs")" ]; }
run_fx() { (cd "$fx" && unset CELERIS_WU_BASE && RELEASES_DIR="$work/no-releases" bash scripts/dev/edit-activate-baseline.sh "$@"); }
runner_sha="$(sha256sum "$work/fake-runner" | cut -d' ' -f1)"

# ---- 4. cos-ts の実行（後半の rep 11〜12） ----
: >"$work/verify-calls"
expect "cos-ts warm の実行" 0 run_fx --scenario cos-ts --mode warm --reps 2 --rep-start 11 --series m-ts-warm --out "$work/cos-ts.jsonl"
if restored; then ok; else ng "cos-ts の後に編集が戻っていない"; fi
if [ "$(grep -c 'base=HEAD keep=1' "$work/verify-calls")" = 3 ]; then ok; else ng "ts-verify を prime 1 回・試行 2 回: $(cat "$work/verify-calls")"; fi
expect "cos-ts の記録の validate" 0 bash "$src" --validate "$work/cos-ts.jsonl"
expect "cos-ts の記録の check-targets" 0 bash "$src" --check-targets "$work/cos-ts.jsonl"
check_py "cos-ts の check-targets の集計" "$work/last.out" '
import json, sys
m = json.load(open(sys.argv[1]))["scenarios"]["cos-ts"]["warm"]
assert (m["trials"], m["over_300s"], m["p50_limit"], m["p95_limit"]) == (2, 0, 30.0, 60.0), m'
check_py "cos-ts の記録の中身（rep 11・12、identity、bundle、load）" "$work/cos-ts.jsonl" "
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
trials = [r for r in rows if r['kind'] == 'trial']
primes = [r for r in rows if r['kind'] == 'prime']
assert [p['rep'] for p in primes] == [0, 0], primes
assert [t['rep'] for t in trials] == [11, 12], [t['rep'] for t in trials]
for t in trials:
    i = t['env']['identity']
    assert i['series'] == 'm-ts-warm' and i['clean'] is True and i['target_kind'] == 'none', i
    assert list(i['tree']) == ['control-ts'] and len(i['tree']['control-ts']) == 40, i['tree']
    assert len(i['commit']) == 40 and i['host'], i
    assert i['bundle_sha256'] == 'cafebabe' and i['host_api'] == 1 and i['runner_sha256'] == '$runner_sha', i
    assert t['over_300s'] is False and t['trial_secs'] == t['secs'], t
    tl = t['trial_load']
    assert set(tl) >= {'before', 'after', 'busy'} and isinstance(tl['busy'], bool), tl
    assert t['edit']['file'] == 'control-ts/src/domains/cos_digest.ts' and t['reverted'] is True
assert len({r['edit']['nonce'] for r in rows if 'edit' in r}) == 3
"
expect "空でない --out は拒む（cos-ts）" 2 run_fx --scenario cos-ts --mode warm --reps 1 --out "$work/cos-ts.jsonl"

# 失敗する試行は記録に残り、編集は戻り、判定で落ちる（bundle が無くても validate は失敗の行を通す）
expect "cos-ts の失敗する試行（cold）" 0 env FAKE_VERIFY_RC=1 bash -c \
  'cd "$1" && unset CELERIS_WU_BASE && bash scripts/dev/edit-activate-baseline.sh --scenario cos-ts --mode cold --reps 1 --out "$2"' \
  _ "$fx" "$work/cos-ts-fail.jsonl"
if restored; then ok; else ng "cos-ts の失敗の後に編集が戻っていない"; fi
expect "cos-ts の失敗の記録の validate" 0 bash "$src" --validate "$work/cos-ts-fail.jsonl"
expect "cos-ts の失敗の記録の check-targets" 1 bash "$src" --check-targets "$work/cos-ts-fail.jsonl"

# ---- 5. cos-digest の実行（偽の cargo を成功させる） ----
: >"$work/fake-cargo.log"
expect "cos-digest warm の実行" 0 env FAKE_CARGO_OK=1 CARGO_TARGET_DIR="$work/target" bash -c \
  'cd "$1" && unset CELERIS_WU_BASE && RELEASES_DIR="$2/no-releases" bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 2 --rep-start 11 --series m-cosd-b --out "$3"' \
  _ "$fx" "$work" "$work/cos-digest.jsonl"
if restored; then ok; else ng "cos-digest の後に編集が戻っていない"; fi
build_calls() { grep -vc ' cargo metadata' "$work/fake-cargo.log" || true; }
if [ "$(grep -c '^edit=no ' "$work/fake-cargo.log")" = 6 ] && [ "$(grep -c '^edit=yes ' "$work/fake-cargo.log")" = 12 ] &&
  [ "$(build_calls)" = 18 ]; then ok; else ng "prime 6 段は編集なし・試行 2×6 段は編集あり: $(cat "$work/fake-cargo.log")"; fi
if [ "$(grep -c -- '-p task-core -p task-dispatch' "$work/fake-cargo.log")" = 12 ] &&
  [ "$(grep -c -- 'build --release -p celeris' "$work/fake-cargo.log")" = 3 ]; then ok; else
  ng "check・clippy・test-build・test-run は task-core と task-dispatch: $(cat "$work/fake-cargo.log")"
fi
expect "cos-digest の記録の validate" 0 bash "$src" --validate "$work/cos-digest.jsonl"
expect "cos-digest の記録の check-targets" 0 bash "$src" --check-targets "$work/cos-digest.jsonl"
check_py "cos-digest の check-targets の集計（上限なし）" "$work/last.out" '
import json, sys
m = json.load(open(sys.argv[1]))["scenarios"]["cos-digest"]["warm"]
assert m["trials"] == 2 and m["over_300s"] == 0 and "p50" in m and "p95" in m and "p95_limit" not in m, m'
check_py "cos-digest の記録の中身（rep 11・12、段の和、identity）" "$work/cos-digest.jsonl" "
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
steps = [r for r in rows if r['kind'] == 'step']
assert [r['rep'] for r in rows if r['kind'] == 'prime'] == [0] * 6
assert [r['rep'] for r in steps] == [11] * 6 + [12] * 6, [r['rep'] for r in steps]
for rep in (11, 12):
    rr = [r for r in steps if r['rep'] == rep]
    assert len({r['trial_secs'] for r in rr}) == 1 and abs(rr[0]['trial_secs'] - sum(r['secs'] for r in rr)) < 0.01
    assert len({json.dumps(r['trial_load'], sort_keys=True) for r in rr}) == 1
for r in steps:
    i = r['env']['identity']
    assert i['series'] == 'm-cosd-b' and i['target_kind'] == 'warm-shared' and i['clean'] is True, i
    assert sorted(i['tree']) == ['crates/task-core', 'crates/task-dispatch'], i['tree']
    assert r['over_300s'] is False and r['reverted'] is True and r['exit'] == 0
    assert r['edit']['file'] == 'crates/task-core/src/chat/digest.rs'
"
expect "空でない --out は拒む（cos-digest）" 2 env FAKE_CARGO_OK=1 CARGO_TARGET_DIR="$work/target" bash -c \
  'cd "$1" && unset CELERIS_WU_BASE && bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 1 --out "$2"' \
  _ "$fx" "$work/cos-digest.jsonl"

# 段の失敗は記録に残り、編集は戻り、判定で落ちる
expect "cos-digest の test-run が落ちる試行" 0 env FAKE_CARGO_OK=1 FAKE_CARGO_FAIL=--no-fail-fast CARGO_TARGET_DIR="$work/target" bash -c \
  'cd "$1" && unset CELERIS_WU_BASE && RELEASES_DIR="$2/no-releases" bash scripts/dev/edit-activate-baseline.sh --scenario cos-digest --mode warm --reps 1 --out "$3"' \
  _ "$fx" "$work" "$work/cos-digest-fail.jsonl"
if restored; then ok; else ng "cos-digest の失敗の後に編集が戻っていない"; fi
expect "cos-digest の失敗の記録の validate" 0 bash "$src" --validate "$work/cos-digest-fail.jsonl"
expect "cos-digest の失敗の記録の check-targets" 1 bash "$src" --check-targets "$work/cos-digest-fail.jsonl"

# ---- 6. --validate が新しい欄を検査する ----
mutate() { # in out python-expr（rows を変える）
  python3 -I - "$1" "$2" "$3" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
exec(sys.argv[3])
open(sys.argv[2], "w").write("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
PY
}
T='t = [r for r in rows if r["kind"] == "trial"][0]; '
S='t = [r for r in rows if r["kind"] == "step"][0]; '
for change in \
  "${T}del t['env']['identity']" "${T}del t['env']['identity']['commit']" "${T}t['env']['identity']['commit'] = ''" \
  "${T}del t['env']['identity']['tree']" "${T}t['env']['identity']['tree'] = {}" "${T}del t['env']['identity']['node']" \
  "${T}del t['env']['identity']['series']" "${T}t['env']['identity']['clean'] = 'yes'" \
  "${T}t['env']['identity']['target_kind'] = 'shared'" "${T}del t['env']['identity']['bundle_sha256']" \
  "${T}t['env']['identity']['bundle_sha256'] = None" "${T}del t['env']['identity']['runner_sha256']" \
  "${T}del t['env']['identity']['host_api']" "${T}del t['trial_load']" "${T}t['trial_load'] = None" \
  "${T}del t['trial_load']['before']" "${T}del t['trial_load']['busy']" "${T}del t['over_300s']" \
  "${T}t['over_300s'] = 1" "${T}del t['trial_secs']"; do
  mutate "$work/cos-ts.jsonl" "$work/m.jsonl" "$change"
  expect "validate（cos-ts）: $change を拒む" 1 bash "$src" --validate "$work/m.jsonl"
done
for change in \
  "${S}del t['env']['identity']" "${S}del t['env']['identity']['pnpm']" "${S}del t['trial_secs']" \
  "${S}del t['over_300s']" "${S}del t['trial_load']" "${S}t['trial_load']['after']['other_cargo_rustc'] = 'x'" \
  "p = [r for r in rows if r['kind'] == 'prime'][0]; del p['env']['identity']"; do
  mutate "$work/cos-digest.jsonl" "$work/m.jsonl" "$change"
  expect "validate（cos-digest）: $change を拒む" 1 bash "$src" --validate "$work/m.jsonl"
done
# 識別を持たない古い記録（S0 の policy の行）は従来どおり通る（edit-activate-policy-ts.sh の 7. も見る）
mutate "$work/cos-digest.jsonl" "$work/m.jsonl" "
for r in rows:
    r['scenario'] = 'policy'
    (r.get('env') or {}).pop('identity', None)
    for k in ('trial_secs', 'over_300s', 'trial_load'):
        r.pop(k, None)"
expect "validate: 識別の無い S0 の policy の行は通る" 0 bash "$src" --validate "$work/m.jsonl"

# ---- 7. --check-targets: 識別の混ざり・段の欠け・300 秒超の印 ----
mutate "$work/cos-ts.jsonl" "$work/m.jsonl" "${T}t['env']['identity']['tree']['control-ts'] = '0' * 40"
expect "check-targets: tree の違う cos-ts の行が混ざる" 1 bash "$src" --check-targets "$work/m.jsonl"
mutate "$work/cos-digest.jsonl" "$work/m.jsonl" "
for r in rows:
    if r['kind'] == 'step' and r['rep'] == 12:
        r['env']['identity']['rustc'] = 'rustc 0.0.0'"
expect "check-targets: toolchain の違う cos-digest の rep が混ざる" 1 bash "$src" --check-targets "$work/m.jsonl"
mutate "$work/cos-digest.jsonl" "$work/m.jsonl" "rows = [r for r in rows if not (r['kind'] == 'step' and r['rep'] == 12 and r['step'] == 'clippy')]"
expect "check-targets: 段の欠けた cos-digest の rep" 1 bash "$src" --check-targets "$work/m.jsonl"
mutate "$work/cos-digest.jsonl" "$work/m.jsonl" "
for r in rows:
    if r['kind'] == 'step' and r['rep'] == 12:
        r['secs'] = 60.0; r['trial_secs'] = 360.0; r['over_300s'] = True"
expect "check-targets: 300 秒を超えた cos-digest の試行は捨てずに集計に入る" 0 bash "$src" --check-targets "$work/m.jsonl"
check_py "300 秒超の集計" "$work/last.out" '
import json, sys
m = json.load(open(sys.argv[1]))["scenarios"]["cos-digest"]["warm"]
assert m["trials"] == 2 and m["over_300s"] == 1 and m["p95"] == 360.0, m'
cat "$work/cos-ts.jsonl" "$work/cos-digest.jsonl" >"$work/both.jsonl"
expect "check-targets: cos-ts と cos-digest の 1 file" 0 bash "$src" --check-targets "$work/both.jsonl"
check_py "2 scenario の集計" "$work/last.out" '
import json, sys
d = json.load(open(sys.argv[1]))
assert sorted(d["scenarios"]) == ["cos-digest", "cos-ts"] and "modes" not in d, d'

if restored; then ok; else ng "最後に編集が戻っていない"; fi
if grep -q ' cargo ' "$work/fake-cargo.log" && ! grep -q '^edit=.*target=$' "$work/fake-cargo.log"; then ok; else
  ng "cos-digest の cargo は CARGO_TARGET_DIR 付きで呼ばれる: $(cat "$work/fake-cargo.log")"
fi

echo "edit-activate-cos-ts: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
