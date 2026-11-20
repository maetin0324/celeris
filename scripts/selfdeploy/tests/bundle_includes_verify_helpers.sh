#!/usr/bin/env bash
# scripts/selfdeploy/tests/bundle_includes_verify_helpers.sh — release に同梱する selfdeploy の道具一式に、
# 各 .sh が自分の隣から読む file（`$(dirname "${BASH_SOURCE[0]}")/<名前>` 等）がすべて入ること。
#
# 2026-11-06 の配送（task 01M6QW0ANN9CS5CWHMDP3N6PKF）で、release.sh が scripts/selfdeploy/*.sh だけを写したため
# snapshot_db.py が release に入らず、current/scripts/verify.sh が「can't open file .../snapshot_db.py」で落ちた。
#
# 試験は release 全体を起こさず、lib.sh の sd_bundle_selfdeploy_scripts を一時 dir に対して呼ぶ。
#   1. release.sh が同梱に sd_bundle_selfdeploy_scripts を使っている（手書きの *.sh loop に戻っていない）
#   2. 同梱先に、各 .sh が隣から参照する file がすべてある（参照は source から抽出する）
#   3. tests/ など下位 directory は同梱しない
#   4. snapshot_db.py が無い source では sd_die（exit 1）する
# 時計・負荷・network に依らず決定的。
# 実行: bash scripts/selfdeploy/tests/bundle_includes_verify_helpers.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/.." && pwd)"
root="$(mktemp -d "${TMPDIR:-/tmp}/bundle-helpers.XXXXXX")"
trap 'rm -rf "$root"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

export CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/cfg" SD_LOG_FILE=""
mkdir -p "$CELERIS_STATE_DIR" "$CELERIS_CONFIG_DIR"

# 1. release.sh が同梱の関数を呼ぶこと
grep -Eq '^[[:space:]]*sd_bundle_selfdeploy_scripts[[:space:]]+"\$BUILD/scripts/selfdeploy"[[:space:]]+"\$STAGE/scripts"' "$SRC/release.sh" \
  || fail "release.sh does not bundle via sd_bundle_selfdeploy_scripts \"\$BUILD/scripts/selfdeploy\" \"\$STAGE/scripts\""

# 2. 同梱を一時 dir に対して行う（subshell: sd_die の exit を閉じ込める）
dest="$root/rel/scripts"
( . "$SRC/lib.sh"; sd_bundle_selfdeploy_scripts "$SRC" "$dest" ) || fail "sd_bundle_selfdeploy_scripts failed on $SRC"

# 各 .sh が自分の隣を指す参照を抽出する:
#   $(dirname "${BASH_SOURCE[0]}")/<名前>
#   $(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/<名前>
#   $SD_DIR/<名前>、$script_dir/<名前>（どちらも dirname BASH_SOURCE を入れた変数）
# 名前に / を含むもの（../../deploy など repo を辿る参照）は同梱の対象外なので除く。
refs="$(grep -ohE '(\$\(dirname "\$\{BASH_SOURCE\[0\]\}"\)|dirname "\$\{BASH_SOURCE\[0\]\}"\)" && pwd\)|\$SD_DIR|\$script_dir)/[A-Za-z0-9_.-]+' "$SRC"/*.sh \
  | sed -E 's#.*/##' | grep -vE '^\.+$' | sort -u)"
[ -n "$refs" ] || fail "no sibling references found (extraction broken)"
echo "$refs" | grep -qx snapshot_db.py || fail "extraction did not find verify.sh's snapshot_db.py reference"
n=0
for name in $refs; do
  [ -f "$dest/$name" ] || fail "bundle lacks $name (referenced next to a selfdeploy script)"
  n=$((n + 1))
done
[ -x "$dest/promote.sh" ] || fail "bundled promote.sh is not executable"

# 3. 下位 directory は同梱しない
[ ! -e "$dest/tests" ] || fail "tests/ was bundled"
if find "$dest" -mindepth 1 -type d | grep -q .; then fail "bundle has subdirectories"; fi

# 4. snapshot_db.py が無い source では sd_die する
bad="$root/bad-src"
mkdir -p "$bad"
cp -p "$SRC"/*.sh "$bad/"
set +e
( . "$SRC/lib.sh"; sd_bundle_selfdeploy_scripts "$bad" "$root/bad-dest" ) 2>"$root/bad.err"
rc=$?
set -e
[ "$rc" -ne 0 ] || fail "bundling without snapshot_db.py did not fail"
grep -q 'snapshot_db.py is missing' "$root/bad.err" || fail "missing snapshot_db.py error not reported: $(cat "$root/bad.err")"

echo "OK: bundle has all $n sibling references (incl. snapshot_db.py); tests/ excluded; missing helper dies"
