#!/usr/bin/env bash
# scripts/dev/edit-activate-baseline.sh の scenario policy-ts と --validate・--check-targets を小さな記録で確かめる
# （ADR 2026-11-02-ts-fast-verify (7)）。引数なし: bash scripts/dev/tests/edit-activate-policy-ts.sh
# 本物の計測はしない。実行の場合は git fixture と偽の ts-verify・偽の runner cache で、本物の lane_rules.ts の写しに編集を当てる。
# cargo の compile・link は起動しない（偽の cargo を PATH の先頭に置き、build 系の subcommand が呼ばれたら落とす）。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
src="$root/scripts/dev/edit-activate-baseline.sh"
real_verify="$root/scripts/dev/ts-verify.sh"
lane="$root/control-ts/src/domains/lane_rules.ts"
[ -n "${TMPDIR:-}" ] || { echo "edit-activate-policy-ts: TMPDIR が無い" >&2; exit 2; }
work="$(mktemp -d "$TMPDIR/edit-activate-policy-ts.XXXXXX")"
trap 'rm -rf "$work"' EXIT

pass=0
fail=0
ok() { pass=$((pass + 1)); }
ng() { fail=$((fail + 1)); echo "FAIL: $*" >&2; }
# 説明 期待（0 か nonzero か数値） コマンド...
expect() {
  local what="$1" want="$2" rc=0
  shift 2
  "$@" >"$work/last.out" 2>"$work/last.err" || rc=$?
  case "$want" in
    nonzero) if [ "$rc" -ne 0 ]; then ok; else ng "$what: exit 0（非 0 を期待）"; fi ;;
    *) if [ "$rc" -eq "$want" ]; then ok; else ng "$what: exit $rc（期待 $want）: $(cut -c1-400 "$work/last.err")"; fi ;;
  esac
}
stderr_has() { # 説明 語
  if grep -qF -- "$2" "$work/last.err"; then ok; else ng "$1: stderr に「$2」が無い: $(cut -c1-400 "$work/last.err")"; fi
}

# ---- 偽の cargo: 版と config の読み取りだけ答え、compile・link を伴う subcommand は失敗にして記録する ----
fake="$work/fakebin"
mkdir -p "$fake"
cat >"$fake/cargo" <<EOF
#!/bin/sh
case "\$1" in
  -V|--version) echo "cargo 0.0.0 (fake)"; exit 0 ;;
  config) exit 1 ;;
esac
echo "cargo \$*" >>"$work/fake-cargo.log"
exit 97
EOF
chmod +x "$fake/cargo"
: >"$work/fake-cargo.log"
export PATH="$fake:$PATH"

# ---- 小さな記録を作る helper ----
cat >"$work/gen.py" <<'PY'
"""gen.py <out> <mode> <secs,secs,...> [欄=JSON ...]（欄は全行に上書き。欄=DEL は消す。@N:欄=JSON は N 行目だけ）"""
import json, sys
STAGES = ["classify", "runner-cache", "install", "typecheck", "bundle", "unit", "golden",
          "safety", "integration", "activate"]
ENV = {"head_subject": "s", "merge_base_subject": "m", "merge_base_expr": "e", "rustc": "r", "cargo": "c",
       "cpu_model": "x", "cpus": 1, "loadavg": [0, 0, 0], "cargo_rustc_procs": 0,
       "sccache": {"present": False, "reachable": False, "hits_delta": None, "misses_delta": None},
       "target_dir": "/t", "target_free_gb": 1.0, "local_free_gb": 1.0, "cargo_incremental": "0",
       "profile_dev_debug": "line-tables-only"}
out, mode, values, *over = sys.argv[1:]
lines = []
for i, secs in enumerate(values.split(","), 1):
    r = {"schema": "celeris.edit-activate-baseline/1", "kind": "trial", "scenario": "policy-ts", "mode": mode,
         "rep": i, "at": "2026-11-02T00:00:00Z", "env": ENV, "cmd": "ts-verify", "secs": float(secs), "exit": 0,
         "class": "ts-only", "stages": [{"name": n, "secs": 0.1, "exit": 0} for n in STAGES],
         "typecheck_secs": 0.1, "bundle_secs": 0.1, "cargo_invocations": 0, "link_count": 0,
         "edit": {"file": "f", "nonce": "n"}, "reverted": True}
    for o in over:
        only = None
        if o.startswith("@"):
            only, o = o[1:].split(":", 1)
        key, _, val = o.partition("=")
        if only is not None and int(only) != i:
            continue
        if val == "DEL":
            r.pop(key, None)
        else:
            r[key] = json.loads(val, parse_constant=float)
    lines.append(json.dumps(r, ensure_ascii=False))
open(out, "a").write("\n".join(lines) + "\n")
PY
gen() { python3 -I "$work/gen.py" "$@"; }
# 20 個の秒を作る: 10 番目までを a、11〜19 番目を b、20 番目を c にする（昇順なら p50 = a、p95 = b）
twenty() { python3 -I -c 'a,b,c=sys_argv=__import__("sys").argv[1:4]; print(",".join([a]*10+[b]*9+[c]))' "$1" "$2" "$3"; }
ct() { bash "$src" --check-targets "$@"; }
new() { rm -f "$work/r.jsonl"; gen "$work/r.jsonl" "$@"; }

# ---- 1. --dry-run ----
before="$(sha256sum "$lane")"
expect "dry-run" 0 bash "$src" --dry-run --scenario policy-ts
for word in lane_rules.ts ts-verify.sh typecheck bundle unit golden safety integration activate "--offline" "--check-targets"; do
  if grep -qF -- "$word" "$work/last.out"; then ok; else ng "dry-run の出力に「$word」が無い"; fi
done
if [ "$before" = "$(sha256sum "$lane")" ] && [ ! -s "$work/fake-cargo.log" ]; then ok; else ng "dry-run が file を変えたか cargo を呼んだ"; fi
expect "既存の scenario の dry-run は残る" 0 bash "$src" --dry-run --scenario policy
expect "route_priority の TS scenario は無い" 2 bash "$src" --dry-run --scenario route-priority-ts
expect "cos の TS scenario は無い" 2 bash "$src" --dry-run --scenario cos-ts

# ---- 2. --check-targets の warm: 目標の内 ----
new warm "$(twenty 30 60 500)"
expect "warm: p50=30・p95=60 ちょうど（20 番目は外れ値）" 0 ct "$work/r.jsonl"
python3 -I - "$work/last.out" <<'PY' && ok || ng "warm の集計（n=20 の p50 は 10 番目、p95 は 19 番目）"
import json, sys
m = json.load(open(sys.argv[1]))["modes"]["warm"]
sys.exit(0 if (m["trials"], m["p50"], m["p95"]) == (20, 30.0, 60.0) else 1)
PY
# 並びに依存しない
new warm "500,$(twenty 30 60 1 | cut -d, -f1-19)"
expect "warm: 行の順に依存しない" 0 ct "$work/r.jsonl"

# ---- 3. warm: 目標の外 ----
new warm "$(python3 -I -c 'print(",".join(["30"]*9+["30.5"]+["60"]*10))')"
expect "warm: 10 番目が 30 秒を超える" 1 ct "$work/r.jsonl"
stderr_has "warm p50" "p50"
new warm "$(twenty 30 60.5 61)"
expect "warm: 19 番目が 60 秒を超える" 1 ct "$work/r.jsonl"
stderr_has "warm p95" "p95"

# ---- 4. cold ----
new cold "$(twenty 110 120 1000)"
expect "cold: p95=120 ちょうど（p50 は判定しない）" 0 ct "$work/r.jsonl"
new cold "$(twenty 110 120.5 1000)"
expect "cold: 19 番目が 120 秒を超える" 1 ct "$work/r.jsonl"
new warm "$(twenty 110 120 1000)"
expect "同じ秒でも warm なら目標の外" 1 ct "$work/r.jsonl"
rm -f "$work/r.jsonl"
gen "$work/r.jsonl" warm "$(twenty 1 2 3)"
gen "$work/r.jsonl" cold "$(twenty 1 121 122)"
expect "warm は内・cold は外の 1 file" 1 ct "$work/r.jsonl"
# 最近傍順位: n=1 は 1 番目。n=2 の p50 は 1 番目、p95 は 2 番目
new warm 30
expect "warm n=1 の内" 0 ct "$work/r.jsonl"
new warm 30,61
expect "warm n=2 の p95 は 2 番目" 1 ct "$work/r.jsonl"

# ---- 5. 全試行の条件: 秒が内でも 1 試行外れたら落ちる ----
small="$(twenty 1 1 1)"
new warm "$small"
expect "基準（全部内）" 0 ct "$work/r.jsonl"
for field in '@7:cargo_invocations=1' '@7:cargo_invocations=DEL' '@7:cargo_invocations=null' '@7:cargo_invocations=false' \
  '@7:link_count=1' '@7:link_count=DEL' '@7:exit=1' '@7:class="rust"' '@7:class=DEL' '@7:reverted=false' \
  '@7:stages=[]' '@7:stages=DEL' '@7:secs=NaN' '@7:secs=null' '@7:secs=-1' '@7:schema="x"' '@7:rep=1' '@7:mode="warm2"'; do
  new warm "$small" "$field"
  expect "1 試行だけ $field" 1 ct "$work/r.jsonl"
done
new warm "$small" '@7:stages=[{"name":"classify","secs":0.1,"exit":0}]'
expect "段が足りない（分類で抜けた試行）" 1 ct "$work/r.jsonl"
python3 -I -c '
import json
names = ["classify","runner-cache","install","typecheck","bundle","unit","golden","safety","integration","activate"]
print(json.dumps([{"name": n, "secs": 0.1, "exit": 1 if n == "safety" else 0} for n in names]))' >"$work/st.json"
new warm "$small" "@7:stages=$(cat "$work/st.json")"
expect "安全シナリオの段が失敗した試行" 1 ct "$work/r.jsonl"

# ---- 6. 記録が無い・違う ----
: >"$work/r.jsonl"
expect "空の file" 1 ct "$work/r.jsonl"
expect "無い file" 2 ct "$work/nonexistent.jsonl"
echo 'not json' >"$work/r.jsonl"
expect "JSON でない行" 1 ct "$work/r.jsonl"
new warm "$small" 'scenario="policy"'
expect "S0 の scenario の試行は判定しない" 1 ct "$work/r.jsonl"
new warm "$small"
echo '{"schema":"celeris.edit-activate-baseline/1","kind":"step","scenario":"policy-ts","mode":"warm","rep":1}' >>"$work/r.jsonl"
expect "試行でない未知の行が混ざる" 1 ct "$work/r.jsonl"
expect "引数なしの --check-targets" 2 bash "$src" --check-targets

# ---- 7. --validate ----
new warm "$small"
expect "validate: 試行の記録を受け付ける" 0 bash "$src" --validate "$work/r.jsonl"
for field in 'stages=DEL' 'typecheck_secs=DEL' 'bundle_secs=DEL' 'cargo_invocations=DEL' 'link_count=DEL' 'class=DEL' \
  'reverted=false' 'secs="x"' 'stages=[{"name":"x"}]' 'env={}'; do
  new warm 1 "$field"
  expect "validate: $field を拒む" 1 bash "$src" --validate "$work/r.jsonl"
done
# S0 の記録（kind=step・prime）は従来どおり通り、欠けは従来どおり落ちる
python3 -I - "$work/s0.jsonl" "$work/s0-bad.jsonl" <<'PY'
import json, sys
env = {"head_subject": "s", "merge_base_subject": "m", "merge_base_expr": "e", "rustc": "r", "cargo": "c",
       "cpu_model": "x", "cpus": 1, "loadavg": [0, 0, 0], "cargo_rustc_procs": 0,
       "sccache": {"present": False, "reachable": False, "hits_delta": None, "misses_delta": None},
       "target_dir": "/t", "target_free_gb": 1.0, "local_free_gb": 1.0, "cargo_incremental": "0",
       "profile_dev_debug": "line-tables-only"}
base = {"schema": "celeris.edit-activate-baseline/1", "scenario": "policy", "mode": "warm", "rep": 1,
        "at": "t", "env": env, "step": "check", "cmd": "c", "secs": 1.0, "exit": 0,
        "compile_secs": 1.0, "link_secs": 0.0, "link_count": 0}
step = dict(base, kind="step", edit={"file": "f", "nonce": "n"}, reverted=True)
prime = dict(base, kind="prime", rep=0)
open(sys.argv[1], "w").write(json.dumps(step) + "\n" + json.dumps(prime) + "\n")
del step["link_count"]
open(sys.argv[2], "w").write(json.dumps(step) + "\n")
PY
expect "validate: S0 の記録" 0 bash "$src" --validate "$work/s0.jsonl"
expect "validate: S0 の記録の欠け" 1 bash "$src" --validate "$work/s0-bad.jsonl"

# ---- 8. 実行: git fixture と偽の ts-verify で試行の流れを確かめる ----
fx="$work/repo"
mkdir -p "$fx/scripts/dev" "$fx/control-ts/src/domains"
cp "$src" "$fx/scripts/dev/edit-activate-baseline.sh"
cp "$lane" "$fx/control-ts/src/domains/lane_rules.ts"
printf 'node_modules/\n' >"$fx/.gitignore"
# --list は本物の分類を使う。実行は偽物: 編集が当たっていて基準が HEAD であることを確かめ、記録を書く。
cat >"$fx/scripts/dev/ts-verify.sh" <<EOF
#!/usr/bin/env bash
set -eu
if [ "\${1:-}" = --list ]; then exec bash "$real_verify" "\$@"; fi
[ "\$1" = --record ] || exit 2
record="\$2"
echo "base=\$CELERIS_WU_BASE node_modules=\$([ -e control-ts/node_modules ] && echo yes || echo no)" >>"$work/verify-calls"
[ "\$CELERIS_WU_BASE" = HEAD ] || exit 2
grep -q 'frontier/edit-activate-baseline-' control-ts/src/domains/lane_rules.ts || exit 2
grep -q 'when: () => false' control-ts/src/domains/lane_rules.ts || exit 2
mkdir -p control-ts/node_modules
rc="\${FAKE_VERIFY_RC:-0}"
python3 -I - "\$record" "\$rc" <<'PY'
import json, sys
names = ["classify","runner-cache","install","typecheck","bundle","unit","golden","safety","integration","activate"]
rc = int(sys.argv[2])
stages = [{"name": n, "secs": 0.25 if n == "typecheck" else 0.5 if n == "bundle" else 0.01, "exit": 0} for n in names]
if rc:
    stages = stages[:8] + [{"name": "integration", "secs": 0.1, "exit": 1}]
json.dump({"schema": "celeris.ts-verify-record/1", "class": "ts-only", "exit": rc, "total_secs": 1.0, "stages": stages,
           "cargo_invocations": 0, "cargo_compile_invocations": 0, "cargo_calls": [], "link_count": 0, "units": ["ALL"]},
          open(sys.argv[1], "w"))
PY
exit "\$rc"
EOF
cat >"$fx/scripts/dev/script-runner-cache.sh" <<'EOF'
#!/bin/sh
[ "$1" = ensure ] || exit 2
echo /nonexistent/script-host-runner
EOF
(
  cd "$fx"
  git init -q -b main .
  git config user.email t@example.com
  git config user.name t
  git add -A
  git commit -q -m base
)
orig="$(sha256sum <"$fx/control-ts/src/domains/lane_rules.ts")"
run_fx() { (cd "$fx" && unset CELERIS_WU_BASE && bash scripts/dev/edit-activate-baseline.sh "$@"); }
restored() { [ "$orig" = "$(sha256sum <"$fx/control-ts/src/domains/lane_rules.ts")" ]; }

expect "warm の実行" 0 run_fx --scenario policy-ts --mode warm --reps 2 --out "$work/fx-warm.jsonl"
if restored; then ok; else ng "warm の後に編集が戻っていない"; fi
expect "warm の記録の validate" 0 bash "$src" --validate "$work/fx-warm.jsonl"
expect "warm の記録の check-targets" 0 ct "$work/fx-warm.jsonl"
python3 -I - "$work/fx-warm.jsonl" <<'PY' && ok || ng "warm の記録の中身（prime 2 行・試行 2 行・型検査と bundle の欄・nonce が違う）"
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
primes = [r for r in rows if r["kind"] == "prime"]
trials = [r for r in rows if r["kind"] == "trial"]
assert [p["step"] for p in primes] == ["runner-ensure", "ts-verify"]
assert primes[0]["runner_build_secs"] >= 0 and primes[0]["rep"] == 0
assert [t["rep"] for t in trials] == [1, 2]
for t in trials:
    assert t["typecheck_secs"] == 0.25 and t["bundle_secs"] == 0.5
    assert t["cargo_invocations"] == 0 and t["link_count"] == 0 and t["reverted"] is True
    assert t["secs"] > 0 and len(t["stages"]) == 10 and t["class"] == "ts-only"
nonces = {r["edit"]["nonce"] for r in rows if "edit" in r}
assert len(nonces) == 3
PY

: >"$work/verify-calls"
expect "cold の実行" 0 run_fx --scenario policy-ts --mode cold --reps 2 --out "$work/fx-cold.jsonl"
if restored; then ok; else ng "cold の後に編集が戻っていない"; fi
if [ "$(grep -c 'node_modules=no' "$work/verify-calls")" = 2 ] && [ "$(wc -l <"$work/verify-calls")" = 2 ]; then ok; else
  ng "cold は全試行を node_modules なしで始め、prime で ts-verify を走らせない: $(cat "$work/verify-calls")"
fi
expect "cold の記録の validate" 0 bash "$src" --validate "$work/fx-cold.jsonl"
expect "cold の記録の check-targets" 0 ct "$work/fx-cold.jsonl"

expect "空でない --out は拒む" 2 run_fx --scenario policy-ts --mode warm --reps 1 --out "$work/fx-cold.jsonl"

# ts-verify の失敗は記録に残り、編集は戻り、判定で落ちる
rm -f "$work/fx-fail.jsonl"
expect "失敗する試行の実行（prime 無しの cold）" 0 env FAKE_VERIFY_RC=1 bash -c \
  'cd "$1" && unset CELERIS_WU_BASE && bash scripts/dev/edit-activate-baseline.sh --scenario policy-ts --mode cold --reps 1 --out "$2"' _ "$fx" "$work/fx-fail.jsonl"
if restored; then ok; else ng "失敗の後に編集が戻っていない"; fi
expect "失敗の記録の validate" 0 bash "$src" --validate "$work/fx-fail.jsonl"
expect "失敗の記録の check-targets" 1 ct "$work/fx-fail.jsonl"

# 文書以外の変更が在る作業ツリーでは測らない。文書だけの変更は測れる（進捗の file に記録を書く使い方）
echo '// dirty' >>"$fx/scripts/dev/script-runner-cache.sh"
rm -f "$work/fx-dirty.jsonl"
expect "文書以外の変更が在ると拒む" 2 run_fx --scenario policy-ts --mode warm --reps 1 --out "$work/fx-dirty.jsonl"
[ -s "$work/fx-dirty.jsonl" ] && ng "拒んだのに記録を書いた" || ok
(cd "$fx" && git checkout -q scripts/dev/script-runner-cache.sh)
expect "記録の置き場所が文書の下なら測れる" 0 run_fx --scenario policy-ts --mode warm --reps 1 --out "$fx/agent-docs/progress/x.jsonl"
expect "文書の下の記録の check-targets" 0 ct "$fx/agent-docs/progress/x.jsonl"

# 中断しても編集が戻る（本物の lane_rules.ts の写しで anchor が 1 回だけのことは上の実行が確かめた）
if restored; then ok; else ng "最後に編集が戻っていない"; fi
if [ ! -s "$work/fake-cargo.log" ]; then ok; else ng "cargo の build 系が呼ばれた: $(cat "$work/fake-cargo.log")"; fi

echo "edit-activate-policy-ts: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
