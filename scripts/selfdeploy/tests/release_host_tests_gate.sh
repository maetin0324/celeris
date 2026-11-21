#!/usr/bin/env bash
# ADR 2026-11-21-release-gate-second-stage-skip（G1・G4・G5・G6）: release.sh は userns の試験 7・9 を、人が明示したとき
# （SD_GATE_SKIP_HOST_TESTS）と、試験 9 だけ NoNewPrivs 1 を読んだときにだけ nextest の filter で外し、gate.json の
# `host_tests` に理由を残す。外さないときは filter を渡さない。`gate_env` に 5 つの env の実際の値が出る。
# G7（移行）: release.sh は試験の runner に CELERIS_SECOND_STAGE_TESTS=require を渡す（呼び出し元の値によらない。NoNewPrivs 0 でも 1 でも）。
# 偽の cargo / pnpm / node / celerisctl を使い、本番のパス・ネットワークには触れない（一時ディレクトリだけ）。
set -euo pipefail
# 理由: worker の環境は本番の CELERIS_LOGS_DIR を継ぎ、継いだままだと台本が偽の要約の行を本番の release-records.jsonl に足す。
unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases"
: >"$root/config/config.toml"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/release.out" ] && tail -n 40 "$root/release.out" >&2
  exit 1
}

# 偽の cargo / pnpm / node / celerisctl、偽の木（$repo）と new_commit は共有の fixture が用意する。偽の cargo の nextest の
# Summary 行は注記付き（`7 passed (1 slow), 2 skipped`・失敗は `6 passed (1 slow, 1 leaky), 1 failed, 2 skipped`）で、
# FAKE_* の env の意味も fixture の冒頭にある。
. "$here/tests/fixtures/release-gate-tree.sh"

run_release() {
  : >"$root/cargo.log"
  : >"$root/cargo-env.log"
  CARGO_ENV_LOG="$root/cargo-env.log" CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    CARGO_LOG="$root/cargo.log" FAKE_SCRATCH="$root/scratch" SD_RELEASE_PRUNE=0 \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$1" >"$root/release.out" 2>&1
}

# `gate_is <gate.json> <python expr over g>`
gate_is() {
  python3 - "$1" "$2" <<'PY'
import json, sys
g = json.load(open(sys.argv[1], encoding="utf-8"))
steps = {s["step"]: s for s in g["steps"]}
sys.exit(0 if eval(sys.argv[2], {"g": g, "steps": steps}) else 1)
PY
}


run_gate() { # run_gate <sha>（env は呼び出し側が付ける）
  run_release "$1"
}
status_file() { printf 'Name:\tx\nNoNewPrivs:\t%s\nSeccomp:\t0\n' "$1" >"$root/status"; echo "$root/status"; }
SUDO_NAME=real_process_with_another_uid_is_rejected_by_dac_then_peer_uid_mismatch
SSH_NAME=db_guard::tests::ssh_config_includes_still_parse_inside_the_namespace
nextest_line() { grep -- '^nextest run ' "$root/cargo.log" | head -1; }
# G7: nextest run を起こした cargo が見た CELERIS_SECOND_STAGE_TESTS の値。
second_stage_env() { grep -- '^nextest run ' "$root/cargo-env.log" | head -1 | sed -n 's/.* CELERIS_SECOND_STAGE_TESTS=//p'; }

# ---- (i) 除外なし・NoNewPrivs 0 → filter なし、両方 run ----
c="$(new_commit)"; c12="${c:0:12}"
SD_PROC_SELF_STATUS="$(status_file 0)" run_gate "$c" || fail "plain release failed"
nextest_line | grep -q -- ' -E ' && fail "a filter was passed without any exclusion: $(nextest_line)"
g="$root/state/releases/$c12/gate.json"
gate_is "$g" "g['host_tests']['sudo']['run'] and g['host_tests']['ssh']['run'] and g['host_tests']['sudo']['off_by'] is None" \
  || fail "host_tests wrong for no exclusion: $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["host_tests"])' "$g")"
# (iv) gate_env: 5 つの値。既定の CELERIS_USERNS_TESTS=1・ISOLATION=require、LAUNCHER は立てない（null）、SECOND_STAGE=require（G7）。
gate_is "$g" "g['gate_env'] == {'CELERIS_USERNS_TESTS': '1', 'CELERIS_ISOLATION_TESTS': 'require', 'CELERIS_LAUNCHER_TESTS': None, 'CELERIS_SECOND_STAGE_TESTS': 'require', 'SD_GATE_SKIP_HOST_TESTS': None}" \
  || fail "gate_env wrong: $(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["gate_env"])' "$g")"
# G7: NoNewPrivs 0 の gate では試験 7・9 が strict で走る（runner に env が届く）。
[ "$(second_stage_env)" = require ] || fail "NoNewPrivs 0: runner did not get CELERIS_SECOND_STAGE_TESTS=require: $(cat "$root/cargo-env.log")"
# release-records.jsonl の要約にも host_tests が残る。
grep -q '"host_tests"' "$root/state/logs/release-records.jsonl" 2>/dev/null \
  || grep -rq '"host_tests"' "$root/state" --include=release-records.jsonl || fail "release-records has no host_tests"

# ---- (ii) 人が明示: SD_GATE_SKIP_HOST_TESTS=sudo,ssh → 2 つを外す -E、off_by が env ----
c="$(new_commit)"; c12="${c:0:12}"
SD_GATE_SKIP_HOST_TESTS=sudo,ssh SD_PROC_SELF_STATUS="$(status_file 0)" run_gate "$c" || fail "release with explicit exclusion failed"
line="$(nextest_line)"
case "$line" in *"-E not ((package(celeris-credentiald) & test(=$SUDO_NAME)) | (package(task-worker) & test(=$SSH_NAME)))"*) ;; *) fail "filter missing/wrong: $line" ;; esac
g="$root/state/releases/$c12/gate.json"
gate_is "$g" "not g['host_tests']['sudo']['run'] and not g['host_tests']['ssh']['run'] and g['host_tests']['sudo']['off_by'] == 'SD_GATE_SKIP_HOST_TESTS' and g['host_tests']['ssh']['off_by'] == 'SD_GATE_SKIP_HOST_TESTS' and g['gate_env']['SD_GATE_SKIP_HOST_TESTS'] == 'sudo,ssh'" \
  || fail "host_tests wrong for explicit exclusion"

# ---- (iii) 未知の key → gate の前に落ちる（nextest は走らない） ----
c="$(new_commit)"; c12="${c:0:12}"
if SD_GATE_SKIP_HOST_TESTS=sudo,bogus SD_PROC_SELF_STATUS="$(status_file 0)" run_gate "$c"; then fail "unknown key accepted"; fi
grep -q "unknown key 'bogus'" "$root/release.out" || fail "no reason for the unknown key"
if grep -q '^nextest run ' "$root/cargo.log"; then fail "tests ran despite an unknown key"; fi

# ---- I4: NoNewPrivs 1 → 試験 9 だけ外れ、off_by が no_new_privs。ssh は外れない ----
c="$(new_commit)"; c12="${c:0:12}"
SD_PROC_SELF_STATUS="$(status_file 1)" run_gate "$c" || fail "release under NoNewPrivs 1 failed"
line="$(nextest_line)"
case "$line" in *"-E not ((package(celeris-credentiald) & test(=$SUDO_NAME)))"*) ;; *) fail "NoNewPrivs 1 filter wrong: $line" ;; esac
case "$line" in *"$SSH_NAME"*) fail "ssh test was excluded by NoNewPrivs: $line" ;; esac
g="$root/state/releases/$c12/gate.json"
gate_is "$g" "not g['host_tests']['sudo']['run'] and g['host_tests']['sudo']['off_by'] == 'no_new_privs' and 'NoNewPrivs: 1' in g['host_tests']['sudo']['reason'] and g['host_tests']['ssh']['run']" \
  || fail "host_tests wrong for NoNewPrivs 1"
# G7: NoNewPrivs 1 でも env は立つ（試験 9 は filter で skipped、試験 7 は strict で走る）。
[ "$(second_stage_env)" = require ] || fail "NoNewPrivs 1: runner did not get CELERIS_SECOND_STAGE_TESTS=require: $(cat "$root/cargo-env.log")"
# G7: 呼び出し元が別の値を立てても、gate は require に揃える（人は env で厳格化を外せない）。
c="$(new_commit)"; c12="${c:0:12}"
CELERIS_SECOND_STAGE_TESTS=0 SD_PROC_SELF_STATUS="$(status_file 0)" run_gate "$c" || fail "release with CELERIS_SECOND_STAGE_TESTS=0 failed"
[ "$(second_stage_env)" = require ] || fail "caller's CELERIS_SECOND_STAGE_TESTS overrode the gate: $(cat "$root/cargo-env.log")"
gate_is "$root/state/releases/$c12/gate.json" "g['gate_env']['CELERIS_SECOND_STAGE_TESTS'] == 'require'" || fail "gate_env CELERIS_SECOND_STAGE_TESTS wrong"
# 明示の sudo が先（off_by は env）。
c="$(new_commit)"
SD_GATE_SKIP_HOST_TESTS=sudo SD_PROC_SELF_STATUS="$(status_file 1)" run_gate "$c" || fail "explicit+NoNewPrivs release failed"
gate_is "$root/state/releases/${c:0:12}/gate.json" "g['host_tests']['sudo']['off_by'] == 'SD_GATE_SKIP_HOST_TESTS'" || fail "explicit exclusion must win over NoNewPrivs"

# ---- 非常用 runner（cargo-test）でも同じ試験を --skip --exact で外す ----
c="$(new_commit)"; c12="${c:0:12}"
FAKE_NO_NEXTEST=1 SD_GATE_TEST_RUNNER=cargo-test SD_GATE_SKIP_HOST_TESTS=ssh SD_PROC_SELF_STATUS="$(status_file 0)" run_gate "$c" || fail "cargo-test runner with exclusion failed"
grep -qx -- "test --workspace -- --skip $SSH_NAME --exact" "$root/cargo.log" || fail "cargo-test skip args wrong: $(cat "$root/cargo.log")"

echo "release_host_tests_gate: ok"
