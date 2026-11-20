#!/usr/bin/env bash
# ADR 2026-11-20-release-record-summary-log D2〜D5: gate.json・verify.json・promoted.json を書いた時点で、その要約が
# $SD_LOGS/release-records.jsonl に 1 行ずつ追記され、release の掃除（release.sh の prune_releases）で release dir が消えた後も残る。
#
# 守る不変条件:
#   (1) release.sh は gate の成功（release dir ができた後）と失敗（.build/<sha12>/gate.json）で 1 行足す。verify は verify.json、
#       昇格は promoted.json を書いた後に lib.sh の sd_release_record で 1 行足す。行の値は正本の file の値と同じ。
#   (2) 掃除で release dir が消えても、その sha の gate・verify・promoted の行が残る。
#   (3) 追記だけで既存の行を書き換えない（2 回目以降の追記の後も 1 行目がバイト単位で同じ。再 verify で正本が変わっても
#       前の行は直さない）。
#   (4) 追記先に書けない・正本が読めないときは warning だけで、release.sh の exit code と gate.json は変わらない（D4）。
#   (5) 要約の log は写しで、判断に読まない（D3、unpromoted-release-no-takeover）: crates/ と台本の注釈でない行に
#       release-records の参照が無い。
# 足す理由: 既存の prune の試験（release_prune_production_scale.sh・release_prune_stale_test_binaries.sh・
# prune_tests_stay_in_tmp.sh）は cargo の target の掃除を見るもので、release dir の掃除の後に要約が残るかを見ない。
# release_parallel_test_gate.sh 等の release の試験は SD_RELEASE_PRUNE=0 で掃除を飛ばす。なので、この試験を足す。
#
# 偽の cargo / pnpm / node / celerisctl と一時 dir の fixture だけを使い、本番の path・DB・network・systemd に触れない。
# verify と昇格は本物の台本を回さず（systemd と本番 API の偽物が大量に要る）、lib.sh の sd_release_record を fixture の
# verify.json と、audit.sh の sd_audit begin/finish で作った本物の形の promoted.json に当てる。台本が正本を書いた後に
# 呼ぶことは、台本の行の順で確かめる。
# 実行: bash scripts/selfdeploy/tests/release_record_summary_survives_prune.sh（引数なし。成功で exit 0）
set -euo pipefail
# 本番の paths.env（CELERIS_BACKUPS_DIR・CELERIS_LOGS_DIR・CELERIS_CONFIG）を環境から継がない。log は一時 dir へ向け直す。
unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR CELERIS_CONFIG SD_LOG_FILE

here="$(cd "$(dirname "$0")/.." && pwd)"
top="$(cd "$here/../.." && pwd)"
export SD_SCRIPT_BUNDLE_GATE="$here/tests/fixtures/fake-script-bundle-gate.sh"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases" "$root/logs"
: >"$root/config/config.toml"
records="$root/logs/release-records.jsonl"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/release.out" ] && tail -n 30 "$root/release.out" >&2
  [ -f "$records" ] && { echo "--- $records" >&2; cat "$records" >&2; }
  exit 1
}

# ---- 偽の道具（release_parallel_test_gate.sh と同じ形） ----
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  metadata) printf '{"workspace_members":["x 0.1.0"],"packages":[{"id":"x 0.1.0","name":"x"}]}\n' ;;
  nextest)
    case "$2" in
      --version) echo 'cargo-nextest 0.9.146 (fake)' ;;
      run)
        printf '    Starting 7 tests across 2 binaries (2 tests skipped)\n'
        if [ "${FAKE_NEXTEST_FAIL:-0}" = 1 ]; then
          printf '        FAIL [   0.004s] x tests::b\n'
          printf '     Summary [   0.020s] 7 tests run: 6 passed (1 slow), 1 failed, 2 skipped\n'
          exit 100
        fi
        printf '     Summary [   0.020s] 7 tests run: 7 passed (1 slow), 2 skipped\n'
        ;;
    esac
    ;;
  test)
    printf '   Doc-tests x\n'
    printf 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n'
    ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl celeris-credentiald; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done
    ;;
esac
exit 0
EOF
cat >"$root/bin/pnpm" <<'EOF'
#!/usr/bin/env bash
case "$1" in
  --version) echo 11.27.0 ;;
  install) mkdir -p node_modules && : >node_modules/.modules.yaml ;;
  build) mkdir -p build && echo ok >build/index.html ;;
esac
exit 0
EOF
cat >"$root/bin/node" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = --version ] && echo v24.0.0
exit 0
EOF
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then
  owner=""
  while [ $# -gt 0 ]; do
    [ "$1" = "--owner" ] && owner="$2"
    shift
  done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
fi
exit 0
EOF
chmod +x "$root/bin/"*

# ---- 偽の木（release.sh・test-parallel.sh が読む file を写す。release_parallel_test_gate.sh と同じ） ----
repo="$root/repo"
mkdir -p "$repo/crates/task-core/src/store" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
cp "$here"/*.sh "$here"/*.py "$repo/scripts/selfdeploy/"
printf '# fixture: no CoS skills\n' >"$repo/scripts/selfdeploy/cos-skills.txt"
chmod +x "$repo/scripts/selfdeploy/"*.sh
mkdir -p "$repo/scripts/dev/lib" "$repo/scripts/dev/repo-health" "$repo/tools/nextest"
cp "$top/scripts/dev/test-parallel.sh" "$top/scripts/dev/source-size-report.py" "$repo/scripts/dev/"
cp "$top/scripts/dev/lib/test-tmp-lifecycle.sh" "$top/scripts/dev/lib/check-slot.sh" "$repo/scripts/dev/lib/"
cp "$top/scripts/dev/repo-health/repo_health.py" "$repo/scripts/dev/repo-health/"
cp "$top/tools/nextest/VERSION" "$repo/tools/nextest/"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store/migrations.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
. "$here/tests/fixtures/web-release.sh"
git init -q "$repo"
echo 0 >"$root/n"
new_commit() {
  local n
  n=$(($(cat "$root/n") + 1))
  echo "$n" >"$root/n"
  printf '// v%s\n' "$n" >"$repo/crates/x/src/lib.rs"
  git -C "$repo" add -A
  git -C "$repo" -c user.email=t@example.invalid -c user.name=t commit -q -m "c$n"
  git -C "$repo" rev-parse HEAD
}

# 掃除を飛ばさない（SD_RELEASE_PRUNE を立てない）。LOGS は呼び出しごとに変えられる（場合 6 の書けない置き場）。
LOGS="$root/logs"
sd_env() {
  env CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    CELERIS_LOGS_DIR="$LOGS" SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    FAKE_SCRATCH="$root/scratch" PATH="$root/bin:$PATH" "$@"
}
run_release() { sd_env bash "$here/release.sh" "$1" >"$root/release.out" 2>&1; }
# `record <kind> <path>` — 台本と同じ lib.sh の関数を、同じ置き場の env で呼ぶ。
record() {
  sd_env bash -c 'set -euo pipefail; . "$1/lib.sh"; sd_release_record "$2" "$3"' _ "$here" "$1" "$2" >>"$root/record.err" 2>&1
}
nlines() { if [ -f "$records" ]; then wc -l <"$records" | tr -d ' '; else echo 0; fi; }
line_of() { sed -n "${1}p" "$records"; }
# `row_is <n> <python expr over r (行 n の dict) と f (正本の dict。第 3 引数があれば)>`
row_is() {
  python3 - "$records" "$1" "$2" "${3:-}" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8")]
r = rows[int(sys.argv[2]) - 1]
f = json.load(open(sys.argv[4], encoding="utf-8")) if sys.argv[4] else None
sys.exit(0 if eval("(" + sys.argv[3] + "\n)", {"r": r, "f": f, "rows": rows}) else 1)
PY
}

# ---- 1. release（gate が通る）→ release dir ができた後に gate の行が 1 行 ----
c1="$(new_commit)"
s1="${c1:0:12}"
run_release "$c1" || fail "release $s1 failed"
g1="$root/state/releases/$s1/gate.json"
[ -f "$g1" ] || fail "no gate.json in the release dir"
[ "$(nlines)" = 1 ] || fail "expected 1 record after the first release, got $(nlines)"
row_is 1 "r['kind'] == 'gate' and r['sha12'] == '$s1' and r['ok'] is True and r['file'] == '$g1' and r['sha'] == '$c1'
  and r['failed_step'] is None and r['step_count'] == len(f['steps']) and r['step_count'] > 0
  and r['steps'] == [{'step': s['step'], 'exit': s['exit']} for s in f['steps']]
  and r['cargo_test']['nextest_passed'] == 7 and r['cargo_test']['nextest_skipped'] == 2 and r['cargo_test']['failed'] == 0
  and r['cargo_test']['passed'] == f['cargo_test']['passed'] and r['at'].endswith('Z')" "$g1" \
  || fail "the gate record does not match $g1"
first="$(line_of 1)"

# ---- 2. verify（ok）と昇格 → verify・promoted の行 ----
rel1="$root/state/releases/$s1"
cat >"$rel1/verify.json" <<EOF
{"sha12": "$s1", "at": "2026-11-20T00:00:00Z", "ok": true, "live_ok": false, "schema_version": 7,
 "checks": [{"id": "1", "name": "migrate-snapshot", "ok": true, "detail": "x", "task_id": "", "elapsed_s": 0, "secs": 0.1},
            {"id": "4d", "name": "web-app-start", "ok": true, "detail": "y", "task_id": "", "elapsed_s": 0, "secs": 0.2}]}
EOF
record verify "$rel1/verify.json"
[ "$(nlines)" = 2 ] || fail "expected 2 records after verify, got $(nlines)"
row_is 2 "r['kind'] == 'verify' and r['sha12'] == '$s1' and r['ok'] is True and r['live_ok'] is False and r['schema_version'] == 7
  and r['checks'] == [{'id': '1', 'name': 'migrate-snapshot', 'ok': True}, {'id': '4d', 'name': 'web-app-start', 'ok': True}]" \
  || fail "the verify record is wrong"
# promoted.json は promote.sh と同じく audit.sh の sd_audit begin → <audit>.summary → sd_audit finish 0 で作る。
audit_file="$(SD_ACTOR=script SD_REASON=fixture SD_POLICY_REF=fixture-policy SD_TASK_REFS='["T1"]' \
  bash -c '. "$1/audit.sh"; sd_audit begin "$2" promote "" "$3" stop-start' _ "$here" "$rel1" "$s1")" \
  || fail "sd_audit begin failed"
printf '{"promoted_at": "2026-11-20T00:01:00Z", "mode": "stop-start", "from": null, "included": {"releases": ["%s"], "tasks": [{"task_id": "T1", "title": "t"}], "complete": true}}\n' \
  "$s1" >"$audit_file.summary"
SD_ACTOR=script SD_REASON=fixture SD_POLICY_REF=fixture-policy bash -c '. "$1/audit.sh"; sd_audit finish "$2" 0' _ "$here" "$audit_file" \
  || fail "sd_audit finish failed"
[ -f "$rel1/promoted.json" ] || fail "sd_audit finish did not write promoted.json"
record promoted "$rel1/promoted.json"
[ "$(nlines)" = 3 ] || fail "expected 3 records after promote, got $(nlines)"
row_is 3 "r['kind'] == 'promoted' and r['sha12'] == '$s1' and r['ok'] is True and r['status'] == 'succeeded'
  and r['operation'] == 'promote' and r['mode'] == 'stop-start' and r['from'] is None
  and r['promoted_at'] == '2026-11-20T00:01:00Z' and r['actor'] == 'script' and r['policy_ref'] == 'fixture-policy'
  and r['included_tasks'] == ['T1']" || fail "the promoted record is wrong"
[ "$(line_of 1)" = "$first" ] || fail "line 1 changed after the second append"

# ---- 3. 再 verify（ok が偽）→ 新しい行が増え、前の行は直さない（正本は release dir の verify.json） ----
python3 -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d["ok"]=False; json.dump(d, open(p,"w"))' "$rel1/verify.json"
record verify "$rel1/verify.json"
[ "$(nlines)" = 4 ] || fail "expected 4 records after re-verify, got $(nlines)"
row_is 4 "r['kind'] == 'verify' and r['ok'] is False" || fail "the second verify record is wrong"
row_is 2 "r['ok'] is True" || fail "the first verify record was rewritten"

# ---- 4. gate が落ちる → .build/<sha12>/gate.json の失敗の行 ----
c2="$(new_commit)"
s2="${c2:0:12}"
if FAKE_NEXTEST_FAIL=1 run_release "$c2"; then fail "release $s2 succeeded although a test failed"; fi
g2="$root/state/releases/.build/$s2/gate.json"
[ -f "$g2" ] || fail "no failed gate.json"
[ "$(nlines)" = 5 ] || fail "expected 5 records after the failed gate, got $(nlines)"
row_is 5 "r['kind'] == 'gate' and r['sha12'] == '$s2' and r['ok'] is False and r['failed_step'] == 'cargo-test'
  and r['file'] == '$g2' and r['cargo_test']['failed'] == 1 and r['cargo_test']['nextest_passed'] == 6" \
  || fail "the failed gate record is wrong"

# ---- 5. 次の release の掃除で $s1 の release dir が消えても、$s1 の 3 種の行が残る ----
# current・previous は無く、$s1 の verify.json は ok が偽なので、掃除は $s1 を残さない。
c3="$(new_commit)"
s3="${c3:0:12}"
run_release "$c3" || fail "release $s3 failed"
grep -q "pruning release $s1" "$root/release.out" || fail "release.sh did not prune $s1"
[ ! -e "$rel1" ] || fail "the release dir of $s1 was not pruned"
[ "$(nlines)" = 6 ] || fail "expected 6 records after the third release, got $(nlines)"
python3 - "$records" "$s1" <<'PY' || fail "the records of the pruned release $s1 are not all there"
import json, sys
kinds = [r["kind"] for r in map(json.loads, open(sys.argv[1], encoding="utf-8")) if r["sha12"] == sys.argv[2]]
sys.exit(0 if kinds == ["gate", "verify", "promoted", "verify"] else 1)
PY
row_is 6 "r['kind'] == 'gate' and r['sha12'] == '$s3' and r['ok'] is True" || fail "the gate record of $s3 is wrong"
[ "$(line_of 1)" = "$first" ] || fail "line 1 changed after later appends"

# ---- 6. 追記先に書けない・正本が読めない → warning だけで release.sh の結果は同じ（D4） ----
: >"$root/not-a-dir"
LOGS="$root/not-a-dir/logs"
c4="$(new_commit)"
s4="${c4:0:12}"
run_release "$c4" || fail "release $s4 failed although only the summary log was unwritable"
[ -f "$root/state/releases/$s4/gate.json" ] || fail "no gate.json for $s4"
python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1]))["ok"] is True else 1)' "$root/state/releases/$s4/gate.json" \
  || fail "gate.json of $s4 is not ok"
grep -q 'warning: release-records: could not append the gate summary' "$root/release.out" || fail "no warning for the unwritable log"
LOGS="$root/logs"
: >"$root/record.err"
record verify "$root/missing/verify.json" || fail "sd_release_record returned non-zero for an unreadable source"
grep -q 'warning: release-records: could not summarize' "$root/record.err" || fail "no warning for an unreadable source"
[ "$(nlines)" = 6 ] || fail "a record was appended although the source was unreadable"

# ---- 7. 台本は正本を書いた後に呼ぶ。要約の log を判断に読まない（D3） ----
grep -c 'sd_release_record gate ' "$here/release.sh" | grep -qx 3 || fail "release.sh does not append at the 3 gate.json writes"
awk '/^mv -T "\$STAGE" "\$REL"/{m=1} m && /^sd_release_record gate "\$REL\/gate.json"/{f=1} END{exit !f}' "$here/release.sh" \
  || fail "release.sh does not append the passing gate after the release dir exists"
awk '/} >"\$REL\/verify.json"/{m=1} m && /^sd_release_record verify "\$REL\/verify.json"/{f=1} END{exit !f}' "$here/verify.sh" \
  || fail "verify.sh does not append after writing verify.json"
awk '/^sd_audit finish "\$AUDIT_FILE" 0/{m=1} m && /^sd_release_record promoted "\$REL\/promoted.json"/{f=1} END{exit !f}' "$here/promote.sh" \
  || fail "promote.sh does not append after promoted.json is written"
for f in release verify promote rollback status; do
  if grep -n 'release-records' "$here/$f.sh" | grep -v '^[0-9]*:[[:space:]]*#'; then fail "$f.sh reads release-records outside a comment"; fi
done
if [ -d "$top/crates" ] && grep -rn 'release-records' "$top/crates"; then fail "crates/ refers to release-records"; fi

echo "release_record_summary_survives_prune: ok"
