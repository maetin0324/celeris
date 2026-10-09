#!/usr/bin/env bash
# ADR 2026-10-08-browser-prod-enablement D1.2 / D1.3 / D1.5: release.sh の browser-ledger 段（非 blocking）と
# browser-ledger.sh（人が実行する作り直し）。
#   (1) 未配置: agent-browser が無い／版違い／生成器が落ちる → release は exit 0 で作られ、browser/ledger-status.json に
#       {ok:false, code}、manifest.json と gate.json の browser_ledger にも code。gate-logs/browser-ledger.log が残る。
#   (2) 配置済み: 全部通ると release に browser/conformance.json と ledger-status.json（ok）が入る。P4-B が落ちても
#       公開能力の台帳は残る（p4b=false）。
#   (3) browser-ledger.sh: 台帳が無い／古い（別 release 用）なら作り直し、有効なら作り直さない（--force で強制）。
# 偽の cargo / pnpm / 生成器 / agent-browser / celerisctl を使い、本番の ~/.config/celeris・~/.local/celeris・/local には触れない。
[ -n "${BASH_VERSION:-}" ] || exec bash "$0" "$@"
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state/releases"
: >"$root/config/config.toml"
: >"$root/fake.log"

fail() {
  echo "FAIL: $*" >&2
  [ -f "$root/release.out" ] && tail -n 40 "$root/release.out" >&2
  exit 1
}

# ---- 偽の道具 ----------------------------------------------------------------

cat >"$root/bin/cargo" <<'EOF2'
#!/usr/bin/env bash
case "$1" in
  metadata) printf '{"workspace_members":["x 0.1.0"],"packages":[{"id":"x 0.1.0","name":"x"}]}\n' ;;
  nextest)
    case "$2" in
      --version) echo 'cargo-nextest 0.9.146 (fake)' ;;
      run)
        printf '    Starting 3 tests across 1 binary (1 test skipped)\n'
        printf '     Summary [   0.010s] 3 tests run: 3 passed, 1 skipped\n'
        ;;
    esac
    ;;
  test) printf '   Doc-tests x\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n' ;;
  build)
    mkdir -p "$CARGO_TARGET_DIR/release"
    for b in celeris celerisctl celeris-credentiald; do printf '#!/bin/sh\nexit 0\n' >"$CARGO_TARGET_DIR/release/$b"; chmod +x "$CARGO_TARGET_DIR/release/$b"; done
    ;;
esac
exit 0
EOF2
# gui の pnpm（corepack の shim 相当）。GUI_FAIL_STEP=typecheck なら gui の gate を落とす。
cat >"$root/bin/pnpm" <<'EOF2'
#!/usr/bin/env bash
printf '%s\t%s\n' "$PWD" "$*" >>"$PNPM_LOG"
[ "${GUI_FAIL_STEP:-}" = "$1" ] && { echo "gui $1 failed (fake)"; exit 1; }
case "$1" in
  --version) echo 11.27.0 ;;
  install) mkdir -p node_modules && : >node_modules/.modules.yaml ;;
  build) mkdir -p build && echo ok >build/index.html ;;
esac
exit 0
EOF2
# web の pnpm は `corepack pnpm@12.6.0 …` で呼ばれる。WEB_FAIL_STEP=test なら web-pnpm-test を落とす。
cat >"$root/bin/corepack" <<'EOF2'
#!/usr/bin/env bash
spec="$1"; shift
printf '%s\t%s\t%s\n' "$PWD" "$spec" "$*" >>"$COREPACK_LOG"
case "$spec" in pnpm@12.6.0) ;; *) echo "unexpected corepack spec: $spec" >&2; exit 2 ;; esac
dir="."
if [ "${1:-}" = -C ]; then dir="$2"; shift 2; fi
[ "${WEB_FAIL_STEP:-}" = "$1" ] && { echo "web $1 failed (fake)"; exit 1; }
case "$1" in
  install) mkdir -p "$dir/node_modules" && : >"$dir/node_modules/.modules.yaml" ;;
  release)
    name="celeris-web-0.1.0-${CELERIS_WEB_RELEASE:?}"
    stage="$(mktemp -d)"
    mkdir -p "$stage/$name/server" "$stage/$name/dist" release
    printf '// gateway\n' >"$stage/$name/server/index.js"
    printf '{"name":"celeris-web","release":"%s"}\n' "$CELERIS_WEB_RELEASE" >"$stage/$name/package.json"
    tar -czf "release/$name.tar.gz" -C "$stage" "$name"
    rm -rf "$stage"
    printf 'release/%s.tar.gz\n' "$name"
    ;;
esac
exit 0
EOF2
cat >"$root/bin/node" <<'EOF2'
#!/usr/bin/env bash
[ "${1:-}" = --version ] && echo v24.0.0
exit 0
EOF2
cat >"$root/bin/celerisctl" <<'EOF2'
#!/usr/bin/env bash
if [ "$1 $2" = "scratch lease" ]; then
  owner=""
  while [ $# -gt 0 ]; do [ "$1" = "--owner" ] && owner="$2"; shift; done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
  exit 0
fi
if [ "$1 $2 $3" = "browser ledger check" ]; then
  echo "check $*" >>"$FAKE_LOG"
  file=""; rel=""
  while [ $# -gt 0 ]; do
    case "$1" in --file) file="$2" ;; --release) rel="$2" ;; esac
    shift
  done
  python3 - "$file" "$rel" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    print(json.dumps({"ok": False, "code": "invalid"})); sys.exit(3)
if d.get("generated_for", {}).get("celeris_release") != sys.argv[2]:
    print(json.dumps({"ok": False, "code": "stale_release"})); sys.exit(3)
credential = any(x.get("test") == "credential-proof" for x in d.get("results", []))
print(json.dumps({"ok": True, "code": "ok", "backends": ["claude-code", "browser-specialist"], "credential_backends": ["claude-code"] if credential else []}))
PY
  exit $?
fi
exit 0
EOF2
# 偽の生成器（browser-conformance.py の代わり）。credential evidence は FAKE_CREDENTIAL_EXIT で制御。
cat >"$root/bin/fake-runner" <<'EOF2'
#!/usr/bin/env bash
echo "runner $*" >>"$FAKE_LOG"
[ "${FAKE_RUNNER_FAIL:-0}" = 1 ] && { echo "runner failed (fake)" >&2; exit 1; }
out=""; rel=""; p4b=false; credential=false; ledger=""
while [ $# -gt 0 ]; do
  case "$1" in --output-dir) out="$2" ;; --celeris-release) rel="$2" ;; --p4b-evidence) p4b=true ;; --credential-evidence) credential=true; ledger="$2" ;; esac
  shift
done
if [ "$p4b" = true ]; then exit "${FAKE_P4B_EXIT:-0}"; fi
if [ "$credential" = true ]; then
  [ "${CELERIS_USERNS_TESTS:-}" = 1 ] || { echo 'missing CELERIS_USERNS_TESTS' >&2; exit 9; }
  mkdir -p "$out/credential-logs"
  code=tests_failed; reason='fixture isolation failed'
  if [ "${FAKE_CREDENTIAL_EXIT:-0}" = 0 ]; then
    code=ok; reason=''
    cp "$ledger" "$out/conformance.json"
    python3 - "$ledger" "$out/conformance.json" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); d['results'].append({'test':'credential-proof','status':'passed'})
json.dump(d,open(sys.argv[2],'w'))
PY
  fi
  printf '{"complete":%s,"code":"%s","reason":"%s","evidence":[]}' "$([ "$code" = ok ] && echo true || echo false)" "$code" "$reason" >"$out/credential-evidence.json"
  exit "${FAKE_CREDENTIAL_EXIT:-0}"
fi
mkdir -p "$out"
printf '{"schema":1,"source":"celeris-browser-conformance-protocol-scripted","generated_for":{"celeris_release":"%s"},"results":[]}\n' "$rel" >"$out/conformance.json"
EOF2
cat >"$root/bin/agent-browser" <<'EOF2'
#!/usr/bin/env bash
echo "agent-browser ${FAKE_AB_VERSION:-0.38.1}"
EOF2
chmod +x "$root/bin/"*

# ---- 元のリポジトリ（web/ は c1 で足す） --------------------------------------------

repo="$root/repo"
mkdir -p "$repo/gui" "$repo/crates/task-core/src" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
printf '{"name":"celeris-gui","version":"0.1.0","packageManager":"pnpm@11.27.0"}\n' >"$repo/gui/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/gui/pnpm-lock.yaml"
printf 'packages:\n  - "."\n' >"$repo/gui/pnpm-workspace.yaml"
printf '// server\n' >"$repo/gui/server.js"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
printf '// v0\n' >"$repo/crates/x/src/lib.rs"
cp "$here"/*.sh "$repo/scripts/selfdeploy/"
chmod +x "$repo/scripts/selfdeploy/"*.sh
mkdir -p "$repo/scripts/dev" "$repo/tools/nextest"
cp "$here/../dev/test-parallel.sh" "$repo/scripts/dev/"
cp "$here/../dev/source-size-report.py" "$repo/scripts/dev/"
cp "$here/../../tools/nextest/VERSION" "$repo/tools/nextest/"
git init -q "$repo"
gitc() { git -C "$repo" -c user.email=t@example.invalid -c user.name=t "$@"; }
commit() { printf "// %s\n" "$1" >"$repo/crates/x/src/lib.rs"; gitc add -A && gitc commit -q -m "$1" && git -C "$repo" rev-parse HEAD; }

# ---- 元のリポジトリ（web/ は c1 で足す） --------------------------------------------

repo="$root/repo"
mkdir -p "$repo/gui" "$repo/crates/task-core/src" "$repo/crates/celeris" "$repo/crates/x/src" "$repo/scripts/selfdeploy"
printf '{"name":"celeris-gui","version":"0.1.0","packageManager":"pnpm@11.27.0"}\n' >"$repo/gui/package.json"
printf 'lockfileVersion: 9.0\n' >"$repo/gui/pnpm-lock.yaml"
printf 'packages:\n  - "."\n' >"$repo/gui/pnpm-workspace.yaml"
printf '// server\n' >"$repo/gui/server.js"
printf 'pub const SCHEMA_VERSION: u32 = 7;\n' >"$repo/crates/task-core/src/store.rs"
printf '[package]\nname = "celeris"\nversion = "0.1.0"\n' >"$repo/crates/celeris/Cargo.toml"
printf '// v0\n' >"$repo/crates/x/src/lib.rs"
cp "$here"/*.sh "$repo/scripts/selfdeploy/"
chmod +x "$repo/scripts/selfdeploy/"*.sh
mkdir -p "$repo/scripts/dev" "$repo/tools/nextest"
cp "$here/../dev/test-parallel.sh" "$repo/scripts/dev/"
cp "$here/../dev/source-size-report.py" "$repo/scripts/dev/"
cp "$here/../../tools/nextest/VERSION" "$repo/tools/nextest/"
git init -q "$repo"
gitc() { git -C "$repo" -c user.email=t@example.invalid -c user.name=t "$@"; }

run_release() {
  : >"$root/pnpm.log"
  : >"$root/corepack.log"
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    PNPM_LOG="$root/pnpm.log" COREPACK_LOG="$root/corepack.log" FAKE_SCRATCH="$root/scratch" SD_GATE_SKIP_WEB=1 SD_RELEASE_PRUNE=0 \
    FAKE_LOG="$root/fake.log" SD_BROWSER_LEDGER_RUNNER="$root/bin/fake-runner" \
    SD_BROWSER_LEDGER_CHECK_BIN="$root/bin/celerisctl" SD_BROWSER_LEDGER_TIMEOUT=120 \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$1" >"$root/release.out" 2>&1
}
run_ledger() { # browser-ledger.sh <sha12> [args]
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$repo" SD_CELERISCTL="$root/bin/celerisctl" FAKE_SCRATCH="$root/scratch" FAKE_LOG="$root/fake.log" \
    SD_BROWSER_LEDGER_RUNNER="$root/bin/fake-runner" SD_BROWSER_LEDGER_CHECK_BIN="$root/bin/celerisctl" \
    SD_BROWSER_LEDGER_TIMEOUT=120 PATH="$root/bin:$PATH" bash "$here/browser-ledger.sh" "$@" >"$root/ledger.out" 2>&1
}
runner_calls() { grep -c '^runner --protocol-scripted' "$root/fake.log" || true; }
# `json_is <file> <python expr over d>`
json_is() {
  python3 - "$1" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
sys.exit(0 if eval(sys.argv[2], {"d": d}) else 1)
PY
}

# ---- (1a) agent-browser が無い → release は作られ、台帳は未配置（agent_browser_missing） ----
c0="$(commit base)"
c0_12="${c0:0:12}"
SD_AGENT_BROWSER="$root/bin/no-such-agent-browser" run_release "$c0" || fail "release failed although only the browser ledger is missing"
rel0="$root/state/releases/$c0_12"
[ -x "$rel0/bin/celeris" ] || fail "release was not created"
[ ! -e "$rel0/browser/conformance.json" ] || fail "a ledger was placed without agent-browser"
json_is "$rel0/browser/ledger-status.json" "d['ok'] is False and d['code'] == 'agent_browser_missing'" || fail "ledger-status (missing) is wrong"
json_is "$rel0/manifest.json" "d['gate_ok'] is True and d['browser_ledger'] == {'ok': False, 'code': 'agent_browser_missing'}" || fail "manifest.browser_ledger (missing) is wrong"
json_is "$rel0/gate.json" "d['ok'] is True and d['browser_ledger']['code'] == 'agent_browser_missing'" || fail "gate.browser_ledger (missing) is wrong"
[ -f "$rel0/gate-logs/browser-ledger.log" ] || fail "gate-logs/browser-ledger.log is missing"
[ "$(runner_calls)" = 0 ] || fail "the generator ran without agent-browser"
[ ! -e "$rel0/browser.partial" ] || fail "browser.partial was left behind"

# ---- (1b) 版違い ----
c1="$(commit v1)"
c1_12="${c1:0:12}"
FAKE_AB_VERSION=0.37.0 run_release "$c1" || fail "release failed on an agent-browser version mismatch"
json_is "$root/state/releases/$c1_12/browser/ledger-status.json" "d['ok'] is False and d['code'] == 'agent_browser_version'" || fail "ledger-status (version) is wrong"

# ---- (1c) 生成器が落ちる ----
c2="$(commit v2)"
c2_12="${c2:0:12}"
FAKE_RUNNER_FAIL=1 run_release "$c2" || fail "release failed although the ledger generator failed"
rel2="$root/state/releases/$c2_12"
json_is "$rel2/browser/ledger-status.json" "d['ok'] is False and d['code'] == 'generator_failed'" || fail "ledger-status (generator) is wrong"
[ ! -e "$rel2/browser/conformance.json" ] || fail "a ledger was placed although the generator failed"
grep -q 'runner failed (fake)' "$rel2/gate-logs/browser-ledger.log" || fail "generator output is not in gate-logs/browser-ledger.log"

# ---- (2) 配置済み: 全部通る ----
c3="$(commit v3)"
c3_12="${c3:0:12}"
run_release "$c3" || fail "release with a working ledger failed"
rel3="$root/state/releases/$c3_12"
[ -f "$rel3/browser/conformance.json" ] || fail "release has no browser/conformance.json"
json_is "$rel3/browser/ledger-status.json" "d['ok'] is True and d['code'] == 'ok' and d['agent_browser'] == '0.38.1' and d['p4b'] is True and d['credential_backends'] == ['claude-code']" || fail "ledger-status (ok) is wrong"
json_is "$rel3/manifest.json" "d['browser_ledger'] == {'ok': True, 'code': 'ok'}" || fail "manifest.browser_ledger (ok) is wrong"
json_is "$rel3/browser/conformance.json" "d['generated_for']['celeris_release'] == '$c3_12'" || fail "ledger is not for this release"
grep -q -- "--celeris-release $c3_12" "$root/fake.log" || fail "generator did not get --celeris-release"
grep -q -- "--p4b-backend claude-code --p4b-backend browser-specialist" "$root/fake.log" || fail "p4b evidence step did not run"
grep -q -- '--credential-evidence .*--credential-backend claude-code --credential-backend browser-specialist' "$root/fake.log" || fail "credential evidence step did not run"
json_is "$rel3/browser/ledger-status.json" "d['credential_evidence'] == {'ok': True, 'code': 'ok', 'reason': ''}" || fail "credential evidence status is wrong"
json_is "$rel3/browser/conformance.json" "any(x.get('test') == 'credential-proof' for x in d['results'])" || fail "credential evidence ledger was not adopted"

# ---- (2b) P4-B が落ちても公開能力の台帳は残る ----
c4="$(commit v4)"
c4_12="${c4:0:12}"
FAKE_P4B_EXIT=1 run_release "$c4" || fail "release failed on a p4b failure"
rel4="$root/state/releases/$c4_12"
[ -f "$rel4/browser/conformance.json" ] || fail "public-capability ledger was dropped on a p4b failure"
json_is "$rel4/browser/ledger-status.json" "d['ok'] is True and d['p4b'] is False" || fail "ledger-status (p4b failed) is wrong"
json_is "$rel4/browser/ledger-status.json" "d['credential_evidence'] == {'ok': False, 'code': 'p4b_incomplete', 'reason': 'P4-B evidence incomplete'}" || fail "credential evidence was not marked skipped after P4-B failure"

# ---- (2c) credential evidence が失敗しても公開台帳を維持し理由を記録 ----
c5="$(commit v5)"
c5_12="${c5:0:12}"
FAKE_CREDENTIAL_EXIT=1 run_release "$c5" || fail "release failed on credential evidence failure"
rel5="$root/state/releases/$c5_12"
[ -f "$rel5/browser/conformance.json" ] || fail "public ledger was dropped on credential failure"
json_is "$rel5/browser/ledger-status.json" "d['ok'] is True and d['credential_evidence']['ok'] is False and d['credential_evidence']['code'] == 'tests_failed' and d['credential_backends'] == []" || fail "credential failure status/backends are wrong"
grep -q 'credential evidence exit 1 (code=tests_failed)' "$rel5/gate-logs/browser-ledger.log" || fail "credential failure reason is absent from release log"

# ---- (3) browser-ledger.sh ----
# 未配置の rel0 を作り直す（agent-browser は今は通る）。
: >"$root/fake.log"
run_ledger "$c0_12" || { cat "$root/ledger.out" >&2; fail "browser-ledger.sh failed on a release without a ledger"; }
[ -f "$rel0/browser/conformance.json" ] || fail "browser-ledger.sh did not place the ledger"
json_is "$rel0/browser/ledger-status.json" "d['ok'] is True and d['credential_evidence']['ok'] is True and d['credential_backends'] == ['claude-code']" || fail "ledger-status after rebuild is wrong"
[ "$(runner_calls)" = 1 ] || fail "expected one generator run, got $(runner_calls)"
grep -q -- '--credential-evidence .*--credential-backend claude-code --credential-backend browser-specialist' "$root/fake.log" || fail "browser-ledger.sh did not run credential evidence"
[ ! -e "$rel0/browser.new" ] && [ ! -e "$rel0/browser.old" ] || fail "temporary browser dirs were left behind"
# 有効なら作り直さない。
run_ledger "$c0_12" || fail "browser-ledger.sh failed on a valid ledger"
grep -q 'already valid' "$root/ledger.out" || fail "valid ledger was not recognized"
[ "$(runner_calls)" = 1 ] || fail "a valid ledger was rebuilt"
# --force は作り直す。
run_ledger "$c0_12" --force || fail "browser-ledger.sh --force failed"
[ "$(runner_calls)" = 2 ] || fail "--force did not rebuild"
# 古い（別 release 用）台帳は作り直す。
python3 - "$rel0/browser/conformance.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); d["generated_for"]["celeris_release"] = "000000000000"
json.dump(d, open(sys.argv[1], "w"))
PY
run_ledger "$c0_12" || fail "browser-ledger.sh failed on a stale ledger"
[ "$(runner_calls)" = 3 ] || fail "a stale ledger was not rebuilt"
json_is "$rel0/browser/conformance.json" "d['generated_for']['celeris_release'] == '$c0_12'" || fail "stale ledger was not replaced"
# 作り直しが失敗したら既存の台帳を残して exit 1。
python3 - "$rel0/browser/conformance.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); d["generated_for"]["celeris_release"] = "000000000000"
json.dump(d, open(sys.argv[1], "w"))
PY
if FAKE_RUNNER_FAIL=1 run_ledger "$c0_12"; then fail "browser-ledger.sh succeeded although the generator failed"; fi
json_is "$rel0/browser/conformance.json" "d['generated_for']['celeris_release'] == '000000000000'" || fail "the existing ledger was replaced by a failed rebuild"
# 存在しない release は exit 非 0。
if run_ledger aaaaaaaaaaaa; then fail "browser-ledger.sh accepted a missing release"; fi

echo "PASS: browser_ledger_release"
