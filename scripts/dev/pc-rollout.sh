#!/usr/bin/env bash
# P4-G（ADR 2026-11-06-phase4-p4g-shadow-rollout）: Project Controller の 1 案件の限定導入の操作を、非本番の一時 daemon で
# 本番と同じ順（preflight → dry_run → shadow の監視・SIGKILL の回復・差分の検出 → canary の拒否 → undo）に通す。
#   bash scripts/dev/pc-rollout.sh --out <絶対 path> [--monitor-secs 20] [--interval 2]
# LLM を呼ばない（fake harness）。外部ネットワークに出ない（127.0.0.1）。本番の DB・config・systemd・releases に触れない。
set -euo pipefail
out="" extra=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --monitor-secs|--interval) extra+=("$1" "$2"); shift 2 ;;
    *) echo "usage: $0 --out <dir> [--monitor-secs N] [--interval S]" >&2; exit 2 ;;
  esac
done
[ -n "$out" ] || { echo "usage: $0 --out <dir>" >&2; exit 2; }
fail() { echo "pc-rollout: FAIL: $*" >&2; exit 1; }
unset CELERIS_RUN_DB CELERIS_DB CELERIS_CONFIG CELERIS_HOME CELERIS_API_TOKEN CELERIS_API_URL CELERIS_SCRIPT_BUNDLE_DIR || true
root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
(cd "$root" && cargo build -q -p celeris --bin celeris -p celeris-script --bin script-deploy) || fail "build failed"
bin="${CARGO_TARGET_DIR:-$root/target}/debug"
work="$(mktemp -d "${TMPDIR:-/tmp}/pc-rollout.XXXXXX")"
cleanup() { chmod -R u+w "${work:?}" 2>/dev/null || true; rm -rf "${work:?}"; }
trap cleanup EXIT
if [ ! -d "$root/control-ts/node_modules" ]; then
  (cd "$root/control-ts" && pnpm install --offline --frozen-lockfile >/dev/null) || fail "control-ts dependencies are missing"
fi
(cd "$root/control-ts" && SC_BUILD="$root/control-ts/scripts/build-bundle.mjs" SC_OUT="$work/bundle" \
  node --input-type=module -e 'const { buildBundle } = await import(process.env.SC_BUILD); await buildBundle({ outDir: process.env.SC_OUT });' >/dev/null) \
  || fail "control-ts bundle failed"
python3 -I "$root/scripts/dev/pc-rollout/pc_rollout.py" e2e --out "$out" --state "$work/home" --celeris "$bin/celeris" \
  --script-deploy "$bin/script-deploy" --bundle "$work/bundle" --repo "$root" "${extra[@]}" || rc=$?
rc=${rc:-0}
cp "$work/home/daemon.log" "$out/daemon.log" 2>/dev/null || true
exit $rc
