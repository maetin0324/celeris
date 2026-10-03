#!/bin/sh
# rustc-wrapper.sh の分岐 3 通りを確かめる。引数なしで `sh scripts/host-sccache/test-wrapper.sh`
# として実行する。偽の rustc と偽の sccache を一時 dir に作るだけで、実 sccache・実 server・
# ネットワークは使わない（SCCACHE_WRAPPER_PROBE を "true"/"false" に差し替えて到達判定を模擬する）。
# 全部通れば exit 0、1 つでも落ちれば exit 1。

set -eu

here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
wrapper="$here/rustc-wrapper.sh"

if ! sh -n "$wrapper"; then
    echo "FAIL: rustc-wrapper.sh が sh -n を通らない" >&2
    exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

log="$tmp/log"

write_fake_rustc() {
    path="$1"
    cat > "$path" <<'EOF'
#!/bin/sh
echo "rustc $*" >> "$LOG_FILE"
exit 0
EOF
    chmod +x "$path"
}

write_fake_sccache() {
    path="$1"
    cat > "$path" <<'EOF'
#!/bin/sh
echo "sccache $*" >> "$LOG_FILE"
rustc_bin="$1"
shift
exec "$rustc_bin" "$@"
EOF
    chmod +x "$path"
}

fake_rustc="$tmp/rustc"
fake_sccache="$tmp/sccache"
write_fake_rustc "$fake_rustc"
write_fake_sccache "$fake_sccache"

export LOG_FILE="$log"

assert_called() {
    who="$1"
    if ! grep -q "^${who} " "$log"; then
        echo "FAIL: $2" >&2
        cat "$log" >&2
        exit 1
    fi
}

assert_not_called() {
    who="$1"
    if grep -q "^${who} " "$log"; then
        echo "FAIL: $2" >&2
        cat "$log" >&2
        exit 1
    fi
}

# 分岐1: sccache があり server に届く → sccache 経由で rustc を呼ぶ
: > "$log"
SCCACHE_BIN="$fake_sccache" SCCACHE_WRAPPER_PROBE="true" "$wrapper" "$fake_rustc" --crate-name demo
assert_called sccache "branch1 (reachable): sccache が呼ばれていない"
assert_called rustc "branch1 (reachable): rustc が呼ばれていない"

# 分岐2: sccache はあるが server に届かない → 素の rustc を直接呼ぶ
: > "$log"
SCCACHE_BIN="$fake_sccache" SCCACHE_WRAPPER_PROBE="false" "$wrapper" "$fake_rustc" --crate-name demo
assert_not_called sccache "branch2 (unreachable): sccache が呼ばれてしまった"
assert_called rustc "branch2 (unreachable): rustc が呼ばれていない"

# 分岐3: sccache が無い → probe を試みず素の rustc を直接呼ぶ
: > "$log"
SCCACHE_BIN="$tmp/no-such-sccache" "$wrapper" "$fake_rustc" --crate-name demo
assert_not_called sccache "branch3 (missing): sccache が呼ばれてしまった"
assert_called rustc "branch3 (missing): rustc が呼ばれていない"

echo "ok: 3 branches passed (reachable / unreachable / missing)"
