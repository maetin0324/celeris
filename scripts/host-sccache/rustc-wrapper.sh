#!/bin/sh
# host の ~/.cargo/config.toml ([build] rustc-wrapper) から呼ばれる POSIX sh 台本。
# ADR-0075 R7-7 の挙動を host 側へ移したもの（docs/adr/0129-host-sccache-reflink-targets.md §2）。
#
# cargo の rustc-wrapper 規約: $1 が実際の rustc、残り($2..)が rustc への引数。
# sccache が見つからない、または server に短い timeout で届かないときは、sccache を経由せず
# `exec "$@"` で rustc を直接動かす。届けば `exec <sccache> "$@"` で sccache を通す
# （sccache 自身が rustc-wrapper protocol で rustc path を受け取る）。
# 到達判定に sccache client は使わない（server を自動で起こさないため）。
#
# 試験用に差し替えられる環境変数:
#   SCCACHE_BIN             sccache 実行ファイルの場所。絶対/相対 path か PATH 上の名前
#                            （既定: "sccache"）
#   SCCACHE_WRAPPER_PROBE   到達判定に使うコマンド文字列（`sh -c` で実行し、exit 0 = 届く
#                            とみなす）。設定すれば既定の TCP probe を置き換える
#   SCCACHE_SERVER_PORT     既定の TCP probe が繋ぐ port（既定 4226。sccache の既定と同じ）
#   SCCACHE_WRAPPER_TIMEOUT 既定の TCP probe の timeout 秒（既定 1）
#
# cc-rs 等が RUSTC_WRAPPER の stem で挙動を変えることがあるため、この台本自体の名前は
# cargo-config.toml.example の例に合わせて自由に変えてよい（中身の判定には関係しない）。

set -eu

sccache_bin="${SCCACHE_BIN:-sccache}"

case "$sccache_bin" in
    */*)
        if [ ! -x "$sccache_bin" ]; then
            exec "$@"
        fi
        ;;
    *)
        resolved="$(command -v "$sccache_bin" 2>/dev/null)" || exec "$@"
        sccache_bin="$resolved"
        ;;
esac

probe_default() {
    port="${SCCACHE_SERVER_PORT:-4226}"
    timeout="${SCCACHE_WRAPPER_TIMEOUT:-1}"
    if ! command -v nc >/dev/null 2>&1; then
        return 1
    fi
    nc -z -w "$timeout" 127.0.0.1 "$port" >/dev/null 2>&1
}

reachable=1
if [ -n "${SCCACHE_WRAPPER_PROBE:-}" ]; then
    if sh -c "$SCCACHE_WRAPPER_PROBE"; then
        reachable=0
    fi
elif probe_default; then
    reachable=0
fi

if [ "$reachable" -eq 0 ]; then
    exec "$sccache_bin" "$@"
fi
exec "$@"
