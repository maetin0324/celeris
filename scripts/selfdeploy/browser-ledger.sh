#!/usr/bin/env bash
# scripts/selfdeploy/browser-ledger.sh <sha12> [--force] — release の browser 適合台帳を作り直す
# （ADR 2026-10-08-browser-prod-enablement D1.5）。**人が実行する**（release dir への書き込み＝本番操作。ADR-0095 付記 D-d）。
#
#   release 時に agent-browser が無かった・P4-B が落ちた等で、release を作り直さずに台帳だけ作り直したいときに使う。
#   1. releases/<sha12>/browser/conformance.json が今の host で有効（`celerisctl browser ledger check` が ok）なら
#      何もしない（--force で強制）。
#   2. build worktree（$SD_BUILD_TREE）をその sha に合わせ、`sd_browser_ledger` を回す（release.sh と同じ関数）。
#   3. 台帳が置けたら releases/<sha12>/browser/ を置き換える。daemon は台帳の mtime を見て再起動なしに拾う。
#      置けなかったら（既存の台帳があるときは）既存を残して exit 1。
#
# env: SD_REPO / CELERIS_STATE_DIR（lib.sh）、SD_BROWSER_LEDGER_TIMEOUT・SD_BROWSER_LEDGER_RUNNER・SD_AGENT_BROWSER・
#   SD_BROWSER_LEDGER_CHECK_BIN（既定は release の bin/celerisctl。lib.sh の `sd_browser_ledger` 参照）。
set -euo pipefail

SD_PROG=browser-ledger
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

usage() {
  echo "usage: browser-ledger.sh <sha12> [--force]" >&2
  exit 2
}

SHA12=""
FORCE=false
while [ $# -gt 0 ]; do
  case "$1" in
    --force) FORCE=true ;;
    -h | --help) usage ;;
    --*) usage ;;
    *)
      [ -z "$SHA12" ] || usage
      SHA12="$1"
      ;;
  esac
  shift
done
[ -n "$SHA12" ] || usage

sd_require_json_tool
SD_LOG_FILE=""
REL="$(sd_release_dir "$SHA12")"
[ -d "$REL" ] || sd_die "release $SHA12 does not exist: $REL"
CHECK_BIN="${SD_BROWSER_LEDGER_CHECK_BIN:-$REL/bin/celerisctl}"
export SD_BROWSER_LEDGER_CHECK_BIN="$CHECK_BIN"

if [ "$FORCE" != true ] && [ -f "$REL/browser/conformance.json" ] && [ -x "$CHECK_BIN" ] \
  && "$CHECK_BIN" browser ledger check --file "$REL/browser/conformance.json" --release "$SHA12" \
    --agent-browser "${SD_AGENT_BROWSER:-agent-browser}" --json >&2; then
  sd_log "browser ledger of $SHA12 is already valid; nothing to do (--force to rebuild)"
  exit 0
fi

sd_mkdirs
SHA_FULL="$(sd_sha_full "$SHA12")"
BUILD="$SD_BUILD_TREE"
sd_lock_or_tempfail 9 "$SD_BUILD_ROOT/.lock-$SHA12" 0 "release.sh/browser-ledger.sh of $SHA12"
sd_lock_or_tempfail 8 "$SD_RELEASES/.lock-release" "${SD_RELEASE_LOCK_WAIT:-1800}" "release.sh sharing the build tree"

if [ -f "$BUILD/.git" ] && git -C "$BUILD" checkout --quiet --detach --force "$SHA_FULL" >&2; then
  git -C "$BUILD" clean -ffdxq >&2
else
  git -C "$SD_REPO" worktree remove --force "$BUILD" >/dev/null 2>&1 || true
  rm -rf "$BUILD"
  git -C "$SD_REPO" worktree prune
  git -C "$SD_REPO" worktree add --detach "$BUILD" "$SHA_FULL" >&2
fi
[ "$(git -C "$BUILD" rev-parse HEAD)" = "$SHA_FULL" ] || sd_die "build worktree $BUILD is not at $SHA_FULL"

# Keep the explicit caller override. Otherwise share release.sh's release-build
# lease; a stale .cargo-target symlink is never followed as a fallback.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  SD_CARGO_TARGET="$CARGO_TARGET_DIR"
  sd_log "using caller CARGO_TARGET_DIR: $SD_CARGO_TARGET"
else
  sd_release_build_target "$SHA_FULL" "$BUILD" || sd_die "no safe cargo target is available"
fi
mkdir -p "$SD_CARGO_TARGET"

mkdir -p "$SD_STAGING"
WORK="$(mktemp -d "$SD_STAGING/browser-ledger-$SHA12.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$SD_LOGS"
LOG="$SD_LOGS/browser-ledger-$SHA12-$(sd_stamp).log"
sd_log "rebuilding the browser ledger of $SHA12 (log: $LOG)"
OK=true
CARGO_TARGET_DIR="$SD_CARGO_TARGET" CELERIS_USERNS_TESTS=1 \
  sd_browser_ledger "$BUILD" "$WORK" "$SHA12" 2>&1 | tee "$LOG" >&2 || true
[ -f "$WORK/browser/conformance.json" ] || OK=false

if [ "$OK" != true ]; then
  code="$(sd_json_get "$WORK/browser/ledger-status.json" code 2>/dev/null || echo unknown)"
  sd_log "no ledger was produced (code=$code)."
  if [ ! -d "$REL/browser" ]; then
    mv -T "$WORK/browser" "$REL/browser"
    sd_log "recorded $REL/browser/ledger-status.json"
  else
    sd_log "the existing $REL/browser is left as it is"
  fi
  exit 1
fi

# 置き換え: 新しい browser/ を release 内に用意（同じ file system）→ 旧を退避 → rename。
mv -T "$WORK/browser" "$REL/browser.new"
if [ -d "$REL/browser" ]; then mv -T "$REL/browser" "$REL/browser.old"; fi
mv -T "$REL/browser.new" "$REL/browser"
rm -rf "$REL/browser.old"
sd_log "browser ledger replaced: $REL/browser/conformance.json"
