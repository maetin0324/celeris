#!/usr/bin/env bash
# sccache L1（ADR-0075 D4、Phase G2）のバイナリを用意する。
#
#   scripts/scratch/setup-sccache.sh                 # cargo install sccache --locked --version <VERSION>
#   scripts/scratch/setup-sccache.sh --from <path>   # 既に入っている同じ版のバイナリを写す（ネットワークに出ない）
#
# 置き場所は `$CELERIS_STATE_DIR/tools/sccache/bin/sccache`（既定 `~/.local/celeris/tools/sccache/bin/sccache`。
# `[scratch.sccache] binary` の既定と同じ）。版は `tools/sccache/VERSION`（1 行）。
# GitHub の配布バイナリは使わない（照合する sha256 を持たないため。ADR-0075「Phase G2 実装時の逸脱・明確化」）。
#
# **`cargo test` の一部ではない**（ネットワークに出る。人が 1 回だけ手で叩く。ADR-0009 P-34）。
# これ自体は server を起こさない（`celeris-sccache.service` の有効化は人。docs/ops/sccache-l1.md）。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
version_file="$here/tools/sccache/VERSION"
state_dir="${CELERIS_STATE_DIR:-$HOME/.local/celeris}"
root="$state_dir/tools/sccache"
bin="$root/bin/sccache"

usage() {
  echo "usage: setup-sccache.sh [--from <existing sccache binary>]" >&2
  exit 2
}

from=""
while [ $# -gt 0 ]; do
  case "$1" in
    --from)
      [ $# -ge 2 ] || usage
      from="$2"
      shift 2
      ;;
    -h | --help) usage ;;
    *) usage ;;
  esac
done

[ -f "$version_file" ] || {
  echo "no such file: $version_file" >&2
  exit 1
}
version="$(tr -d '[:space:]' <"$version_file")"
[ -n "$version" ] || {
  echo "empty version in $version_file" >&2
  exit 1
}

echo "version: $version"
echo "install: $bin"
mkdir -p "$root/bin"

if [ -n "$from" ]; then
  got="$("$from" --version 2>/dev/null || true)"
  if [ "$got" != "sccache $version" ]; then
    echo "$from reports '$got', expected 'sccache $version'" >&2
    exit 1
  fi
  install -m 0755 "$from" "$bin.tmp.$$"
  mv -f "$bin.tmp.$$" "$bin"
else
  command -v cargo >/dev/null 2>&1 || {
    echo "cargo not found" >&2
    exit 1
  }
  # target は scratch pool に置く（`/tmp` と NFS に置かない。ADR-0075 D7）。自分で決めたときは終わったら消す。
  scratch_dir="${CELERIS_SCRATCH_DIR:-/var/lib/celeris/scratch}"
  own_target=false
  if [ -z "${CARGO_TARGET_DIR:-}" ]; then
    own_target=true
  fi
  build_target="${CARGO_TARGET_DIR:-$scratch_dir/targets/agent-setup-sccache/target}"
  mkdir -p "$build_target"
  # RUSTC_WRAPPER が sccache を指していると、入れ替え中の自分を呼ぶことがあるので外す。
  env -u RUSTC_WRAPPER CARGO_TARGET_DIR="$build_target" \
    cargo install sccache --locked --version "$version" --root "$root"
  if [ "$own_target" = true ]; then
    rm -rf "$(dirname "$build_target")"
  fi
fi

got="$("$bin" --version)"
[ "$got" = "sccache $version" ] || {
  echo "installed binary reports '$got'" >&2
  exit 1
}
echo "ok: $got"
echo
echo "次（人）: scripts/selfdeploy/install-units.sh で celeris-sccache.service を置き、"
echo "  systemctl --user enable --now celeris-sccache.service"
echo "  ~/.local/celeris/current/bin/celerisctl scratch status   # 'sccache L1 ... (ready)' を確かめる"
