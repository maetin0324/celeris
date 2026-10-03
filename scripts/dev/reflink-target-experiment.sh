#!/usr/bin/env bash
# seed の target を別 path へ写したとき、cargo が依存 crate を作り直すかを確かめる小さな実験。
#
# 引数なしで実行し、結果を標準出力に出す。外部ネットワークには出ない（cargo は --offline、
# registry 依存は ~/.cargo/registry に既にある crate だけを使う）。実験用の source と target は
# $CARGO_TARGET_DIR/reflink-exp/ の下に置き、終わったら消す。RUSTC_WRAPPER・SCCACHE_* は
# 渡された値のまま使う（sccache が効いても Compiling 行は出るので、それで判定する）。
#
# FICLONE が EPERM でも copy_file_range が extent を共有しうるため、
# cp --reflink=auto を使い、df と filefrag の結果で共有を判定する。
#
# 条件:
#   0  同じ source path・同じ target path（対照。seed をそのまま再ビルド）
#   1  同じ source path・別 target path
#   2  別 source path（cp -a で mtime を保つ）・別 target path
#   2m 別 source path（mtime を更新して新規 checkout を模擬）・別 target path
#   3  別 source path・同じ target path（seed を退避し、写しを元の path に置く）
set -euo pipefail

if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    echo "CARGO_TARGET_DIR が空。Celeris が渡した target の下で実行する" >&2
    exit 2
fi

EXP="$CARGO_TARGET_DIR/reflink-exp"
REG_SRC=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/ 2>/dev/null | head -n 1 || true)
if [[ -z "$REG_SRC" ]]; then
    echo "~/.cargo/registry/src が無い（--offline で registry 依存を解決できない）" >&2
    exit 2
fi

cleanup() { rm -rf "$EXP"; }
trap cleanup EXIT
rm -rf "$EXP"
mkdir -p "$EXP"

# registry にある版を選ぶ（libc と anyhow は build script を持つ）。
pick() { ls -d "$REG_SRC/$1"-[0-9]* 2>/dev/null | sed "s|.*/$1-||" | sort -V | tail -n 1; }
ITOA_V=$(pick itoa)
LIBC_V=$(pick libc)
ANYHOW_V=$(pick anyhow)
for v in "$ITOA_V" "$LIBC_V" "$ANYHOW_V"; do
    if [[ -z "$v" ]]; then
        echo "itoa / libc / anyhow のどれかが registry に無い" >&2
        exit 2
    fi
done

# --- source（workspace: bin app + path 依存 pdep（build script つき））---
SRC_A="$EXP/src-a"
mkdir -p "$SRC_A/app/src" "$SRC_A/pdep/src"
cat >"$SRC_A/Cargo.toml" <<'EOF'
[workspace]
members = ["app", "pdep"]
resolver = "2"
EOF
cat >"$SRC_A/app/Cargo.toml" <<EOF
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
pdep = { path = "../pdep" }
itoa = "=$ITOA_V"
libc = "=$LIBC_V"
anyhow = "=$ANYHOW_V"
EOF
cat >"$SRC_A/app/src/main.rs" <<'EOF'
fn main() -> anyhow::Result<()> {
    let mut buf = itoa::Buffer::new();
    let pid = unsafe { libc::getpid() };
    println!("{} {}", buf.format(pid), pdep::tag());
    Ok(())
}
EOF
cat >"$SRC_A/pdep/Cargo.toml" <<'EOF'
[package]
name = "pdep"
version = "0.1.0"
edition = "2021"
build = "build.rs"
EOF
cat >"$SRC_A/pdep/build.rs" <<'EOF'
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=tag.txt");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let tag = std::fs::read_to_string("tag.txt").expect("tag.txt");
    std::fs::write(format!("{out}/tag.rs"), format!("{:?}", tag.trim())).expect("write tag.rs");
}
EOF
echo "seed" >"$SRC_A/pdep/tag.txt"
cat >"$SRC_A/pdep/src/lib.rs" <<'EOF'
pub fn tag() -> &'static str {
    include!(concat!(env!("OUT_DIR"), "/tag.rs"))
}
EOF
(cd "$SRC_A" && cargo generate-lockfile --offline -q)

LOCAL_NOTE="/local なし"
if [[ -d /local ]]; then
    LOCAL_NOTE="/local ($(stat -f -c %T /local)): $(findmnt -no OPTIONS /local 2>/dev/null || echo 'mount options 不明')"
fi
if command -v btrfs >/dev/null 2>&1; then
    BTRFS_NOTE="btrfs コマンドあり"
else
    BTRFS_NOTE="btrfs コマンドなし（btrfs filesystem du は取れない）"
fi

df_used() { df -B1 --output=used "$EXP" | tail -n 1 | tr -d ' '; }
mib() { awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'; }

# build <name> <source dir> <target dir> <copy note>: Compiling 行数・Dirty 理由・所要時間を出す。
build() {
    local name=$1 src=$2 tgt=$3 log="$EXP/$1.log" t0 t1
    t0=$(date +%s.%N)
    (cd "$src" && cargo build --offline -v --target-dir "$tgt" >"$log" 2>&1)
    t1=$(date +%s.%N)
    local n
    n=$(grep -cE '^ +Compiling ' "$log" || true)
    local crates
    crates=$(grep -E '^ +Compiling ' "$log" | awk '{print $2}' | sort | tr '\n' ' ' || true)
    printf '| %s | %s | %s | %s | %s |\n' "$name" "$n" \
        "$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')" \
        "${crates:--}" "$4"
    grep -E '^ +Dirty ' "$log" | sed "s|$EXP|\$EXP|g; s/^ */    [$name] /" >>"$EXP/dirty.txt" || true
}

# copy_target <from> <to>: 写し、df 前後と共有 extent を返す。
copy_target() {
    local before after sample shared="filefrag なし" btrfs_du="対象外"
    before=$(df_used)
    cp -a --reflink=auto "$1" "$2"
    sync -f "$2"
    after=$(df_used)
    sample=$(find "$2/debug/deps" -maxdepth 1 -type f -name '*.rlib' -print -quit 2>/dev/null || true)
    if [[ -n "$sample" ]] && command -v filefrag >/dev/null 2>&1; then
        if filefrag -v "$sample" 2>/dev/null | grep -q 'shared'; then shared="shared"; else shared="shared 表示なし"; fi
    fi
    if [[ $(stat -f -c %T "$2") == btrfs ]] && command -v btrfs >/dev/null 2>&1; then
        btrfs_du=$(btrfs filesystem du -s "$2" 2>&1 | tail -n 1)
    fi
    echo "df $(mib "$before")→$(mib "$after") MiB (差 $(mib $((after - before))) MiB); du $(du -sm "$2" | cut -f1) MiB; extent $shared; btrfs du $btrfs_du"
}

echo "# reflink target experiment"
echo "- cargo: $(cargo --version)"
echo "- rustc: $(rustc --version)"
echo "- RUSTC_WRAPPER: ${RUSTC_WRAPPER:-（なし）}  CARGO_INCREMENTAL: ${CARGO_INCREMENTAL:-（未設定）}"
echo "- 実験の場所: \$CARGO_TARGET_DIR/reflink-exp（fs: $(stat -f -c %T "$EXP")）"
echo "- 写し方: cp -a --reflink=auto（FICLONE 失敗時の copy_file_range fallback を許す）"
echo "- $LOCAL_NOTE"
echo "- $BTRFS_NOTE"
echo "- 依存: pdep(path, build.rs), itoa $ITOA_V, libc $LIBC_V(build.rs), anyhow $ANYHOW_V(build.rs)"
echo "- df 使用量（開始時）: $(mib "$(df_used)") MiB"
echo
echo "| 条件 | Compiling 行 | 秒 | Compiling した crate | target の写し（df 増分 / du） |"
echo "|---|---|---|---|---|"
: >"$EXP/dirty.txt"

SEED="$EXP/t-seed"
build seed "$SRC_A" "$SEED" "空から"
build "0 同src・同target" "$SRC_A" "$SEED" "-"

c1=$(copy_target "$SEED" "$EXP/t-c1")
build "1 同src・別target" "$SRC_A" "$EXP/t-c1" "$c1"

SRC_B="$EXP/src-b"
cp -a "$SRC_A" "$SRC_B"
c2=$(copy_target "$SEED" "$EXP/t-c2")
build "2 別src(mtime保持)・別target" "$SRC_B" "$EXP/t-c2" "$c2"

SRC_M="$EXP/src-m"
cp -a "$SRC_A" "$SRC_M"
find "$SRC_M" -type f -exec touch {} +
c2m=$(copy_target "$SEED" "$EXP/t-c2m")
build "2m 別src(mtime更新)・別target" "$SRC_M" "$EXP/t-c2m" "$c2m"

mv "$SEED" "$EXP/t-seed.orig"
c3=$(copy_target "$EXP/t-seed.orig" "$SEED")
build "3 別src・同target(写しを元path)" "$SRC_B" "$SEED" "$c3"

echo
echo "- df 使用量（終了時、消す前）: $(mib "$(df_used)") MiB"
echo
echo "Dirty 理由（cargo -v）:"
if [[ -s "$EXP/dirty.txt" ]]; then cat "$EXP/dirty.txt"; else echo "    （なし）"; fi
