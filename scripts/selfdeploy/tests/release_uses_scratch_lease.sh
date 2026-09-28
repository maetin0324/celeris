#!/usr/bin/env bash
# ADR-0075 §5 G1 受け入れ条件 9 + Phase SD-1: release.sh が scratch の lease（全リリースで共有の owner
# `release-build`、固定の作業ツリー `.build/tree`、`--ttl`）を取り、gate の `CARGO_TARGET_DIR` にその target を
# 使い、step の前に touch し、終了時（ここでは gate 失敗）も release せず touch する（release すると GC が即回収する）。
# celerisctl の lease が失敗したら従来の `$SD_RELEASES/.cargo-target` に戻る。
# 偽の celerisctl / cargo / pnpm を使い、本番のパス・ネットワークには触れない（一時ディレクトリだけ）。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/config" "$root/state"

# 元のリポジトリ（commit 1 つ）。
git init -q "$root/repo"
git -C "$root/repo" -c user.email=t@example.invalid -c user.name=t commit -q --allow-empty -m init
sha="$(git -C "$root/repo" rev-parse HEAD)"
sha12="${sha:0:12}"

# 偽の cargo: 見えた CARGO_TARGET_DIR を記録して失敗する（gate の最初の step で止める）。
cat >"$root/bin/cargo" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "${CARGO_TARGET_DIR:-}" >>"$CARGO_ENV_LOG"
exit 1
EOF
printf '#!/usr/bin/env bash\nexit 0\n' >"$root/bin/pnpm"
# 偽の celerisctl: 引数を記録し、`scratch lease` には target のパスを返す（`FAKE_LEASE_FAIL=1` なら失敗）。
cat >"$root/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CTL_LOG"
if [ "$1 $2" = "scratch lease" ]; then
  [ "${FAKE_LEASE_FAIL:-0}" = 1 ] && { echo "error: scratch is disabled" >&2; exit 1; }
  owner=""
  while [ $# -gt 0 ]; do
    [ "$1" = "--owner" ] && owner="$2"
    shift
  done
  printf '%s\n' "$FAKE_SCRATCH/targets/$owner/target"
fi
exit 0
EOF
chmod +x "$root/bin/cargo" "$root/bin/pnpm" "$root/bin/celerisctl"
: >"$root/config/config.toml"

run_release() {
  CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/config" CELERIS_CONFIG="$root/config/config.toml" \
    SD_REPO="$root/repo" SD_CELERISCTL="$root/bin/celerisctl" SD_PNPM_SHIM_DIR="$root/bin" \
    CTL_LOG="$root/ctl.log" CARGO_ENV_LOG="$root/cargo-env.log" FAKE_SCRATCH="$root/scratch" \
    PATH="$root/bin:$PATH" bash "$here/release.sh" "$sha" >"$root/release.out" 2>&1
}

# ---- 1. lease が取れる: scratch の target で gate が走り、終了時に release ----
: >"$root/ctl.log"
: >"$root/cargo-env.log"
if run_release; then
  echo "release.sh succeeded although the fake cargo fails" >&2
  exit 1
fi
build="$root/state/releases/.build/tree"
expected_lease="scratch lease --config $root/config/config.toml --owner release-build --repo $root/repo --worktree $build --base $sha --ttl 172800"
first="$(head -n 1 "$root/ctl.log")"
last="$(tail -n 1 "$root/ctl.log")"
if [ "$first" != "$expected_lease" ]; then
  echo "unexpected first celerisctl call: $first" >&2
  echo "expected: $expected_lease" >&2
  cat "$root/release.out" >&2
  exit 1
fi
grep -qx "scratch touch --config $root/config/config.toml --owner release-build" "$root/ctl.log" || {
  echo "release.sh did not touch the lease before a step" >&2
  cat "$root/ctl.log" >&2
  exit 1
}
if [ "$last" != "scratch touch --config $root/config/config.toml --owner release-build" ]; then
  echo "release.sh did not touch the lease on exit: $last" >&2
  exit 1
fi
if grep -q "scratch release" "$root/ctl.log"; then
  echo "release.sh released the shared lease (the GC would reclaim the shared target at once)" >&2
  exit 1
fi
# gate が落ちた記録は `.build/<sha12>/` に写る（作業ツリーは次のリリースが使い回す）。
[ -f "$root/state/releases/.build/$sha12/gate.json" ] || {
  echo "gate.json of the failed gate was not kept under .build/$sha12" >&2
  exit 1
}
grep -qx "$root/scratch/targets/release-build/target" "$root/cargo-env.log" || {
  echo "the gate did not use the scratch target" >&2
  cat "$root/cargo-env.log" >&2
  exit 1
}
grep -q "CARGO_TARGET_DIR: $root/scratch/targets/release-build/target" "$root/release.out"

# ---- 2. lease が失敗する（scratch 無効）: 従来の `$SD_RELEASES/.cargo-target`、release は呼ばない ----
: >"$root/ctl.log"
: >"$root/cargo-env.log"
if FAKE_LEASE_FAIL=1 run_release; then
  echo "release.sh succeeded although the fake cargo fails" >&2
  exit 1
fi
grep -qx "$root/state/releases/.cargo-target" "$root/cargo-env.log" || {
  echo "the fallback target was not used" >&2
  cat "$root/cargo-env.log" >&2
  exit 1
}
if grep -q "scratch release" "$root/ctl.log"; then
  echo "release.sh released a lease it never took" >&2
  exit 1
fi
echo "release_uses_scratch_lease: ok"
