#!/usr/bin/env bash
# scripts/dev/script-runner-cache.sh の cache key と置き場所・作り直しを、小さな git fixture・偽の rustc・偽の cargo で確かめる
# （ADR 2026-11-02-ts-fast-verify (4)）。確かめること:
#   - key は 5 入力（host の source・Cargo.lock・features・rustc -vV・host-v1.schema.json）のどれを変えても変わる
#     host の source は script-runtime・golden の元の crate（task-core）・ルート Cargo.toml・未追跡 file・未 commit の編集を含む
#   - 業務 TS・文書の変更では key が変わらない。同じ入力なら同じ key。key は cargo を起動しない
#   - 置き場所が /tmp の下・作業ツリーの中なら exit 2、cache が無ければ path は exit 1
#   - ensure は cache が無いときだけ cargo を呼び、golden が commit 済みの fixture と違えば失敗する。照合に落ちた cache は作り直す
set -euo pipefail

src="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/script-runner-cache.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/script-runner-cache-key.XXXXXX")"
trap 'rm -rf "$work"' EXIT
repo="$work/repo"
fake="$work/fake"
calls="$work/calls"
mkdir -p "$repo" "$fake"
export CALLS="$calls"
: >"$calls"

# 文書の path は doc-links の検査が実在の path と見なさないよう変数で組む。
ad=agent-docs
dd=docs
fail=0
pass=0
check() {
  local what="$1"
  shift
  if "$@"; then pass=$((pass + 1)); else echo "FAIL: $what" >&2; fail=$((fail + 1)); fi
}

# 偽の rustc: -vV の出力を FAKE_RUSTC_RELEASE で変える。
cat >"$fake/rustc" <<'EOF'
#!/bin/sh
[ "$1" = -vV ] || exit 1
printf 'rustc %s (fake)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: %s\n' "${FAKE_RUSTC_RELEASE:-1.0.0}" "${FAKE_RUSTC_RELEASE:-1.0.0}"
EOF
# 偽の cargo: metadata・build・run --example だけを真似る。呼ばれたことを CALLS に残す。
cat >"$fake/cargo" <<'EOF'
#!/bin/sh
echo "cargo $*" >>"$CALLS"
case "$1" in
  metadata)
    printf '{"workspace_root":"%s","target_directory":"%s","workspace_members":["sr","tc"],"packages":[' "$PWD" "$FAKE_TARGET"
    printf '{"id":"sr","name":"celeris-script","dependencies":[]},{"id":"tc","name":"task-core","dependencies":[%s]}]}\n' "${FAKE_TC_DEPS:-}" ;;
  build)
    mkdir -p "$FAKE_TARGET/release"
    printf '#!/bin/sh\necho runner\n' >"$FAKE_TARGET/release/script-host-runner"
    chmod +x "$FAKE_TARGET/release/script-host-runner" ;;
  run)
    for a in "$@"; do last="$a"; done
    case "$last" in
      dump-lane-fixtures) echo "${FAKE_LANE:-lane}" ;;
      dump-route-priority-fixtures) echo route ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
EOF
chmod +x "$fake"/*
export PATH="$fake:$PATH"
export FAKE_TARGET="$work/target"

cd "$repo"
git init -q -b main .
git config user.email t@example.com
git config user.name t
w() { mkdir -p "$(dirname "$1")"; printf '%s\n' "$2" >"$1"; }
mkdir -p scripts/dev
cp "$src" scripts/dev/script-runner-cache.sh
w Cargo.toml '[workspace]'
w Cargo.lock '# lock'
w crates/script-runtime/src/lib.rs '// host'
w crates/script-runtime/vendor/deno_core/src/lib.rs '// vendored'
w crates/task-core/src/lib.rs '// golden source'
w crates/other/src/lib.rs '// other'
w docs/api/script-host/host-v1.schema.json '{}'
w control-ts/src/domains/lane_rules.ts 'export const x = 1;'
w control-ts/fixtures/lane_rules/all-features.jsonl 'lane'
w control-ts/fixtures/route_priority/cases.jsonl 'route'
w "$ad/note.md" '# note'
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"

key() { bash scripts/dev/script-runner-cache.sh key; }
k0="$(key)"
check "key は 64 hex" bash -c "[[ '$k0' =~ ^[0-9a-f]{64}$ ]]"
check "同じ入力なら同じ key" test "$(key)" = "$k0"
check "key は cargo を起動しない" bash -c "! grep -q . '$calls'"

reset() { git reset -q --hard "$base"; git clean -qfd; }
changes_key() { # 説明 変更コマンド
  local what="$1" k
  shift
  reset
  "$@"
  k="$(key)"
  check "$what で key が変わる" test "$k" != "$k0"
}
keeps_key() {
  local what="$1" k
  shift
  reset
  "$@"
  k="$(key)"
  check "$what で key が変わらない" test "$k" = "$k0"
}

# 入力 1: host の source
changes_key "host の source（script-runtime の未 commit の編集）" w crates/script-runtime/src/lib.rs '// host changed'
changes_key "host の source（deno_core の写し）" w crates/script-runtime/vendor/deno_core/src/lib.rs '// changed'
changes_key "host の source（script-runtime の未追跡 file）" w crates/script-runtime/src/new.rs '// new'
changes_key "host の source（golden の元の crate task-core）" w crates/task-core/src/lib.rs '// golden changed'
changes_key "host の source（ルート Cargo.toml）" w Cargo.toml '[workspace] # changed'
changes_key "host の source（file の削除）" rm crates/script-runtime/src/lib.rs
# 入力 2: Cargo.lock
changes_key "Cargo.lock" w Cargo.lock '# lock changed'
# 入力 3: features（script の定数）
changes_key "features" sed -i 's/^FEATURES="profile=release /FEATURES="profile=dev /' scripts/dev/script-runner-cache.sh
# 入力 4: rustc -vV
reset
k="$(FAKE_RUSTC_RELEASE=1.0.1 key)"
check "rustc -vV で key が変わる" test "$k" != "$k0"
# 入力 5: host-v1.schema.json
changes_key "host-v1.schema.json" w docs/api/script-host/host-v1.schema.json '{"changed":true}'

keeps_key "業務 TS の変更" w control-ts/src/domains/lane_rules.ts 'export const x = 2;'
keeps_key "文書の変更" w "$ad/note.md" '# changed'
keeps_key "関係の無い crate の変更" w crates/other/src/lib.rs '// changed'
reset

check "inputs は 5 入力の行を出す" bash -c '
  out="$(bash scripts/dev/script-runner-cache.sh inputs)"
  for p in host_src cargo_lock features rustc schema; do printf "%s\n" "$out" | grep -q "^$p " || exit 1; done'

# ---- 置き場所 ----
check "置き場所が /tmp の下なら exit 2" bash -c 'CELERIS_SCRIPT_RUNNER_CACHE=/tmp/x bash scripts/dev/script-runner-cache.sh path >/dev/null 2>&1; [ $? -eq 2 ]'
check "置き場所が作業ツリーの中なら exit 2" bash -c "CELERIS_SCRIPT_RUNNER_CACHE='$repo/cache' bash scripts/dev/script-runner-cache.sh ensure >/dev/null 2>&1; [ \$? -eq 2 ]"
export CELERIS_SCRIPT_RUNNER_CACHE="$work/cache"
check "cache が無ければ path は exit 1" bash -c 'bash scripts/dev/script-runner-cache.sh path >/dev/null 2>&1; [ $? -eq 1 ]'

# ---- ensure ----
: >"$calls"
runner="$(bash scripts/dev/script-runner-cache.sh ensure 2>/dev/null)"
check "ensure は key の dir に runner を置く" test "$runner" = "$work/cache/$k0/script-host-runner" -a -x "$runner"
check "ensure は cargo build と 2 つの example を呼ぶ" bash -c "
  grep -q '^cargo build --release --locked -p celeris-script --bin script-host-runner' '$calls' &&
  grep -q '^cargo run -q --locked -p task-core --example dump-lane-fixtures' '$calls' &&
  grep -q '^cargo run -q --locked -p task-core --example dump-route-priority-fixtures' '$calls'"
check "golden を key の dir に書く" cmp "$work/cache/$k0/golden/lane_rules/all-features.jsonl" control-ts/fixtures/lane_rules/all-features.jsonl
check "path は ensure と同じ runner を出す" test "$(bash scripts/dev/script-runner-cache.sh path)" = "$runner"
: >"$calls"
bash scripts/dev/script-runner-cache.sh ensure >/dev/null 2>&1
check "cache が当たれば ensure は cargo を呼ばない" bash -c "! grep -q . '$calls'"
echo tampered >>"$work/cache/$k0/golden/route_priority/cases.jsonl"
check "golden が meta と違えば path は exit 1" bash -c 'bash scripts/dev/script-runner-cache.sh path >/dev/null 2>&1; [ $? -eq 1 ]'
: >"$calls"
bash scripts/dev/script-runner-cache.sh ensure >/dev/null 2>&1
check "照合に落ちた cache は ensure が作り直す" bash -c "grep -q '^cargo build' '$calls' && bash scripts/dev/script-runner-cache.sh path >/dev/null 2>&1"

w crates/script-runtime/src/lib.rs '// host changed'
check "Rust の golden が fixture と違えば ensure は exit 1" bash -c 'FAKE_LANE=other bash scripts/dev/script-runner-cache.sh ensure >/dev/null 2>&1; [ $? -eq 1 ]'
check "失敗した ensure は key の dir を残さない" bash -c "[ ! -e '$work/cache/$(key)' ] && ! ls -d '$work'/cache/.tmp-* >/dev/null 2>&1"
check "golden の元の crate の閉包が一覧と違えば ensure は exit 1" bash -c \
  'FAKE_TC_DEPS="{\"name\":\"celeris-script\",\"kind\":null}" bash scripts/dev/script-runner-cache.sh ensure >/dev/null 2>&1; [ $? -eq 1 ]'
reset

echo "script-runner-cache-key: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
