#!/usr/bin/env bash
# browser-ledger uses the release-build scratch lease by default and never
# follows a reclaimed .cargo-target link. All paths and tools are temporary.
set -euo pipefail
unset CARGO_TARGET_DIR
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/scratch/targets/release-build/target"
: >"$root/config/config.toml"
git init -q "$root/repo"
git -C "$root/repo" -c user.email=t@example.invalid -c user.name=t commit -q --allow-empty -m init
sha="$(git -C "$root/repo" rev-parse HEAD)"
sha12="${sha:0:12}"
mkdir -p "$root/state/releases/$sha12/bin"
cat >"$root/state/releases/$sha12/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
echo '{"ok":true,"code":"ok"}'
EOF
chmod +x "$root/state/releases/$sha12/bin/celerisctl"
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CTL_LOG"
if [ "$1 $2" = 'scratch lease' ]; then
  [ "${LEASE_FAIL:-0}" = 0 ] || exit 1
  printf '%s\n' "$LEASE_TARGET"
fi
exit 0
EOF
cat >"$root/bin/agent-browser" <<'EOF'
#!/usr/bin/env bash
echo 'agent-browser 0.38.1'
EOF
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "${CARGO_TARGET_DIR:-}" >>"$TARGET_LOG"
exit 0
EOF
cat >"$root/bin/runner" <<'EOF'
#!/usr/bin/env bash
cargo target-observed
case " $* " in
  *' --protocol-scripted '*)
    out=''
    while [ $# -gt 0 ]; do [ "$1" = --output-dir ] && out="$2"; shift; done
    mkdir -p "$out"
    printf '{"generated_for":{"celeris_release":"$EXPECTED_SHA12"},"results":[]}\n' >"$out/conformance.json"
    ;;
  *' --credential-evidence '*)
    out=''
    while [ $# -gt 0 ]; do [ "$1" = --output-dir ] && out="$2"; shift; done
    mkdir -p "$out"
    printf '{"complete":false,"code":"launcher_unavailable","reason":"fixture"}\n' >"$out/credential-evidence.json"
    ;;
esac
exit 0
EOF
chmod +x "$root/bin/"*
: >"$root/ctl.log"
: >"$root/targets.log"
run_ledger() {
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" CTL_LOG="$root/ctl.log" \
    LEASE_TARGET="$root/scratch/targets/release-build/target" TARGET_LOG="$root/targets.log" \
    SD_BROWSER_LEDGER_RUNNER="$root/bin/runner" SD_BROWSER_LEDGER_CHECK_BIN="$root/state/releases/$sha12/bin/celerisctl" EXPECTED_SHA12="$sha12" \
    SD_AGENT_BROWSER="$root/bin/agent-browser" PATH="$root/bin:$PATH" \
    bash "$here/browser-ledger.sh" "$sha12" --force >"$root/ledger.out" 2>&1 || { cat "$root/ledger.out" >&2; return 1; }
}
targets_are() { awk -v expected="$1" '$0 != expected { exit 1 } END { if (NR < 1) exit 1 }' "$root/targets.log"; }

# Default selects the same owner and the leased directory.
ln -s "$root/no-such-initial-lease" "$root/state/releases/.cargo-target"
run_ledger
targets_are "$root/scratch/targets/release-build/target"
[ "$(readlink "$root/state/releases/.cargo-target")" = "$root/scratch/targets/release-build/target" ]
grep -q -- '--owner release-build' "$root/ctl.log"

# A dangling legacy link is removed; fallback mkdir uses .cargo-target itself,
# never the link's absent target.
rm -rf "$root/state/releases/$sha12/browser" "$root/state/releases/.build"
: >"$root/targets.log"
rm -f "$root/state/releases/.cargo-target"
ln -s "$root/no-such-old-lease" "$root/state/releases/.cargo-target"
CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
  SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" CTL_LOG="$root/ctl.log" LEASE_FAIL=1 \
  TARGET_LOG="$root/targets.log" SD_BROWSER_LEDGER_RUNNER="$root/bin/runner" \
  SD_BROWSER_LEDGER_CHECK_BIN="$root/state/releases/$sha12/bin/celerisctl" EXPECTED_SHA12="$sha12" SD_AGENT_BROWSER="$root/bin/agent-browser" \
  PATH="$root/bin:$PATH" bash "$here/browser-ledger.sh" "$sha12" --force >"$root/ledger.out" 2>&1 || { cat "$root/ledger.out" >&2; exit 1; }
[ ! -L "$root/state/releases/.cargo-target" ]
[ -d "$root/state/releases/.cargo-target" ]
targets_are "$root/state/releases/.cargo-target"

# Explicit caller target takes precedence and does not request a lease.
rm -rf "$root/state/releases/$sha12/browser" "$root/state/releases/.build"
: >"$root/targets.log"
: >"$root/ctl.log"
CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
  SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" CTL_LOG="$root/ctl.log" \
  TARGET_LOG="$root/targets.log" SD_BROWSER_LEDGER_RUNNER="$root/bin/runner" \
  SD_BROWSER_LEDGER_CHECK_BIN="$root/state/releases/$sha12/bin/celerisctl" EXPECTED_SHA12="$sha12" SD_AGENT_BROWSER="$root/bin/agent-browser" \
  CARGO_TARGET_DIR="$root/caller-target" PATH="$root/bin:$PATH" \
  bash "$here/browser-ledger.sh" "$sha12" --force >"$root/ledger.out" 2>&1 || { cat "$root/ledger.out" >&2; exit 1; }
targets_are "$root/caller-target"
! grep -q 'scratch lease' "$root/ctl.log"
echo 'browser_ledger_default_target: ok'
