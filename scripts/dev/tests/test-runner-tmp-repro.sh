#!/usr/bin/env bash
# scripts/dev/tests/test-runner-tmp-repro.sh — 現行の test-affected.sh・test-parallel.sh が /tmp に作る一時 dir
# （/tmp/celeris-test-affected.* / /tmp/celeris-test-parallel.*）の寿命を、通常終了と signal 終了で比べて観測する
# （agent-docs/adr/2026-11-06-test-runner-tmp-lifecycle.md の「観測」節の根拠）。引数なしで呼ぶ。
#
# 実 cargo・LLM・ネットワーク・userns は使わない。小さな git fixture に**helper 導入前の runner の固定した写し**（scripts/dev/testdata/runner-tmp-repro）を置き、
# PATH の先頭の偽 cargo が `cargo nextest run` の中で ready を FIFO に書いて release の FIFO を読むまで止まる。
# 待ちはすべて FIFO の出来事待ち（保険は timeout 60 秒）で、sleep の時間当てや busy loop は使わない。
#
# 各 case は runner を専用の session（pgid = runner の pid）で起こし、次を比べる:
#   normal / TERM・KILL・INT・HUP を runner の親 process だけへ / 専用 process group 全体へ
# 観測する欄: rc（runner の exit 状態。signal 死は 128+N）、trap（親が signal を捕捉していたか: /proc の SigCgt）、
#   orphan（親が終わった時に偽 cargo がまだ生きていたか）、child_tmp（親の終了後に子が TMPDIR へ書けたか）、
#   left（runner の一時 dir が残ったか）。
# 期待表（EXPECT）と 1 件でも違えば exit 1。最後に偽 cargo・runner の process group と、この試験が作った一時 dir
# （偽 cargo が記録した path のうち prefix と所有者が合う物だけ）・fixture を必ず回収する。
# `sh <script>` で呼ばれても bash で動かす（dash は ${BASH_SOURCE[0]} を構文エラーにする）。
[ -n "${BASH_VERSION:-}" ] || exec bash "$0" "$@"
set -euo pipefail
# fixture の runner が配線済みの検査（test-affected.sh の scripts/dev 変更時の検査）を再び呼ばないための guard。
export CELERIS_RUNNER_TMP_TEST=1

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/runner-tmp-repro.XXXXXX")"
pgids_file="$work/pgids"
roots_file="$work/tmproots"
: >"$pgids_file"
: >"$roots_file"

cleanup() {
  local rc=$? p r
  trap - EXIT INT TERM HUP
  while read -r p; do
    [ -n "$p" ] && kill -KILL -- "-$p" 2>/dev/null || true
  done <"$pgids_file"
  while read -r r; do
    case "$r" in
      /tmp/celeris-test-affected.?*|/tmp/celeris-test-parallel.?*) ;;
      *) continue ;;
    esac
    # 自分の UID の実 dir（symlink でない）だけを消す。
    if [ -d "$r" ] && [ ! -L "$r" ] && [ "$(stat -c %u "$r")" = "$(id -u)" ]; then
      chmod -R u+w "$r" 2>/dev/null || true
      rm -rf "$r"
    fi
  done <"$roots_file"
  chmod -R u+w "$work" 2>/dev/null || true
  rm -rf "$work"
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# ---- fixture: runner の写しを持つ小さな git repo -------------------------------------------------------------
fx="$work/fx"
mkdir -p "$fx/scripts/dev" "$fx/crates/foo/src" "$fx/tools/nextest" "$work/bin"
# 観測対象は helper 導入前の runner。変更後の runner ではなく、固定した写し（testdata/runner-tmp-repro）を使う。
cp "$here/scripts/dev/testdata/runner-tmp-repro/test-affected.sh.pinned" "$fx/scripts/dev/test-affected.sh"
cp "$here/scripts/dev/testdata/runner-tmp-repro/test-parallel.sh.pinned" "$fx/scripts/dev/test-parallel.sh"
chmod +x "$fx/scripts/dev/test-affected.sh" "$fx/scripts/dev/test-parallel.sh"
cp "$here/tools/nextest/VERSION" "$fx/tools/nextest/VERSION"
printf '#!/bin/sh\nexit 0\n' >"$fx/scripts/dev/check-health.sh"
printf '[package]\nname = "foo"\n' >"$fx/crates/foo/Cargo.toml"
echo '// v1' >"$fx/crates/foo/src/lib.rs"
git -C "$fx" init -q -b main
git -C "$fx" -c user.name=t -c user.email=t@example.invalid add -A
git -C "$fx" -c user.name=t -c user.email=t@example.invalid commit -q -m fixture
echo '// v2' >"$fx/crates/foo/src/lib.rs" # test-affected.sh が foo を対象に選ぶ変更
pin="$(tr -d ' \n' <"$fx/tools/nextest/VERSION")"

# 偽 cargo。`nextest run` だけが FIFO で止まる。状態は $FAKE_STATE（一時 dir の外）に書く。
cat >"$work/bin/cargo" <<'EOF'
#!/bin/sh
case "$1 $2" in
  "nextest --version") echo "cargo-nextest $FAKE_PIN"; exit 0 ;;
  "nextest list") echo '{"rust-binaries":{}}'; exit 0 ;;
  "metadata "*)
    printf '{"workspace_root":"%s","target_directory":"%s/target","workspace_members":["foo"],"packages":[{"id":"foo","name":"foo","manifest_path":"%s/crates/foo/Cargo.toml","dependencies":[],"targets":[]}]}\n' \
      "$FAKE_FX" "$FAKE_STATE" "$FAKE_FX"
    exit 0 ;;
  "nextest run")
    exec 7>"$FAKE_LIFE"                # 生存の印: この fd が閉じる（＝子の終了）と controller の cat が EOF を読む
    echo x >"$TMPDIR/fake-test-file"   # 試験が TMPDIR に残す物の代わり
    echo "$$ $TMPDIR" >"$FAKE_READY"   # ready 通知（controller が読むまで止まる）
    read -r _ <"$FAKE_RELEASE"         # release まで止まる
    if echo y >"$TMPDIR/after-release" 2>/dev/null; then echo ok; else echo fail; fi >"$FAKE_STATE/child_tmp"
    echo "Starting 1 test across 1 binary"
    echo "Summary [   0.001s] 1 test run: 1 passed, 0 skipped"
    echo done >"$FAKE_DONE"
    exit 0 ;;
  "test --doc") echo "test result: ok. 0 passed; 0 failed; 0 ignored"; exit 0 ;;
esac
echo "fake cargo: unexpected args: $*" >&2
exit 97
EOF
chmod +x "$work/bin/cargo"

# 親が sig を捕捉しているか（/proc/<pid>/status の SigCgt の bit）。
caught() { # pid signum
  local m
  m="$(sed -n 's/^SigCgt:[[:space:]]*//p' "/proc/$1/status" 2>/dev/null)" || return 1
  [ -n "$m" ] && [ $(( (16#$m >> ($2 - 1)) & 1 )) -eq 1 ]
}
signum() { case "$1" in TERM) echo 15 ;; KILL) echo 9 ;; INT) echo 2 ;; HUP) echo 1 ;; esac; }
fifo_read() { timeout 60 sh -c 'read -r l <"$1" && echo "$l"' _ "$1"; }
fifo_write() { timeout 60 sh -c 'echo go >"$1"' _ "$1"; }

results="$work/results"
: >"$results"
run_case() { # runner sig target
  local runner="$1" sig="$2" target="$3" d pid life line cpid tmpd root rc trapped=- orphan=- ctmp=- left
  d="$work/case-$runner-$sig-$target"
  mkdir -p "$d"
  mkfifo "$d/ready" "$d/release" "$d/done" "$d/life"
  timeout 60 cat "$d/life" >/dev/null &
  life=$!
  # 専用 session で起こし、bash の background が SIG_IGN にする INT/QUIT と python の SIGPIPE 無視を既定に戻す。
  (
    cd "$fx"
    exec env PATH="$work/bin:$PATH" CELERIS_WU_BASE=main TMPDIR="$d" FAKE_PIN="$pin" FAKE_FX="$fx" FAKE_STATE="$d" \
      FAKE_READY="$d/ready" FAKE_RELEASE="$d/release" FAKE_DONE="$d/done" FAKE_LIFE="$d/life" \
      python3 -I -c 'import os,signal,sys
os.setsid()
for s in (signal.SIGINT, signal.SIGQUIT, signal.SIGTERM, signal.SIGHUP, signal.SIGPIPE):
    signal.signal(s, signal.SIG_DFL)
os.execvp(sys.argv[1], sys.argv[1:])' bash "scripts/dev/$runner.sh"
  ) >"$d/out" 2>&1 </dev/null &
  pid=$!
  echo "$pid" >>"$pgids_file"
  line="$(fifo_read "$d/ready")"
  cpid="${line%% *}"
  tmpd="${line#* }"
  root="$(dirname "$tmpd")"
  echo "$root" >>"$roots_file"
  if [ "$sig" = none ]; then
    fifo_write "$d/release"; fifo_read "$d/done" >/dev/null
    set +e; { wait "$pid"; } 2>/dev/null; rc=$?; set -e
  else
    if caught "$pid" "$(signum "$sig")"; then trapped=yes; else trapped=no; fi
    if [ "$target" = parent ]; then kill -s "$sig" "$pid"; else kill -s "$sig" -- "-$pid"; fi
    if [ "$target" = group ]; then
      # group 全体へ送った: 子も signal を受けて死ぬ。生存の FIFO の EOF（子の終了）を待ってから親を待つ。
      orphan=no
      { wait "$life"; } 2>/dev/null || { echo "fake cargo did not exit after $sig to the group" >&2; exit 1; }
      set +e; { wait "$pid"; } 2>/dev/null; rc=$?; set -e
    elif [ "$trapped" = yes ]; then
      # 捕捉した親は前景の子の終わりまで待つ。子を進めてから親を待つ。
      orphan=no
      fifo_write "$d/release"; fifo_read "$d/done" >/dev/null
      set +e; { wait "$pid"; } 2>/dev/null; rc=$?; set -e
    else
      # 捕捉しない親だけが死ぬ。子は signal を受けず release 待ちのまま残る（孤児）。
      set +e; { wait "$pid"; } 2>/dev/null; rc=$?; set -e
      kill -0 "$cpid" 2>/dev/null || { echo "fake cargo died with its parent" >&2; exit 1; }
      orphan=yes
      fifo_write "$d/release"; fifo_read "$d/done" >/dev/null
    fi
    [ -f "$d/child_tmp" ] && ctmp="$(cat "$d/child_tmp")"
  fi
  [ -n "$ctmp" ] || ctmp=-
  case "$root" in /tmp/celeris-test-affected.?*|/tmp/celeris-test-parallel.?*) ;; *) echo "unexpected tmp root $root" >&2; exit 1 ;; esac
  if [ -e "$root" ]; then left=yes; else left=no; fi
  printf '%s %s %s rc=%s trap=%s orphan=%s child_tmp=%s left=%s\n' "$runner" "$sig" "$target" "$rc" "$trapped" "$orphan" "$ctmp" "$left" >>"$results"
  if [ "$sig" = none ] && [ "$runner" = test-parallel ]; then
    # 試験の残り物の警告は削除の前の診断（偽 cargo が 2 件残す）。警告が出て、その後で dir が消えていること。
    if grep -q 'test-parallel: warning: tests left 2 entries in TMPDIR (removed on exit)' "$d/out"; then
      echo "test-parallel leftovers-warning=yes" >>"$results"
    else
      echo "test-parallel leftovers-warning=no" >>"$results"
    fi
  fi
}

for r in test-affected test-parallel; do
  run_case "$r" none -
  for s in TERM KILL INT HUP; do
    run_case "$r" "$s" parent
    run_case "$r" "$s" group
  done
done

# 観測した現行挙動（bash 5.2、2026-11-06）。行ごとの拡張正規表現。
# - EXIT trap を持つ bash は TERM・INT・HUP を捕捉し、前景の子の終わりを待ってから trap で一時 dir を消す。
# - INT を親だけに送ると、子が INT で死なない限り bash は script を続け rc=0 で終わる（cooperative exit）。
# - KILL（親だけ・group）は trap を走らせず一時 dir を残す。親だけなら子は孤児として残り、その dir を使い続ける。
# - 親だけへの TERM・HUP は、bash が子の wait に入る前に届くと trap が子の終わりを待たずに走り、子が使用中の
#   TMPDIR を消す（child_tmp=fail）。wait 中に届けば子の終わりまで待つ（ok）。どちらになるかは届く瞬間で決まる
#   ので両方を許し、末尾に fail の数を出す。INT は fail を観測していないが、同じ競合の余地があるので両方を許す。
EXPECT='test-affected none - rc=0 trap=- orphan=- child_tmp=- left=no
test-affected TERM parent rc=143 trap=yes orphan=no child_tmp=(ok|fail) left=no
test-affected TERM group rc=143 trap=yes orphan=no child_tmp=- left=no
test-affected KILL parent rc=137 trap=no orphan=yes child_tmp=ok left=yes
test-affected KILL group rc=137 trap=no orphan=no child_tmp=- left=yes
test-affected INT parent rc=(0|130) trap=yes orphan=no child_tmp=(ok|fail) left=no
test-affected INT group rc=130 trap=yes orphan=no child_tmp=- left=no
test-affected HUP parent rc=129 trap=yes orphan=no child_tmp=(ok|fail) left=no
test-affected HUP group rc=129 trap=yes orphan=no child_tmp=- left=no
test-parallel none - rc=0 trap=- orphan=- child_tmp=- left=no
test-parallel leftovers-warning=yes
test-parallel TERM parent rc=143 trap=yes orphan=no child_tmp=(ok|fail) left=no
test-parallel TERM group rc=143 trap=yes orphan=no child_tmp=- left=no
test-parallel KILL parent rc=137 trap=no orphan=yes child_tmp=ok left=yes
test-parallel KILL group rc=137 trap=no orphan=no child_tmp=- left=yes
test-parallel INT parent rc=(0|130) trap=yes orphan=no child_tmp=(ok|fail) left=no
test-parallel INT group rc=130 trap=yes orphan=no child_tmp=- left=no
test-parallel HUP parent rc=129 trap=yes orphan=no child_tmp=(ok|fail) left=no
test-parallel HUP group rc=129 trap=yes orphan=no child_tmp=- left=no'

sed 's/^/runner-tmp-repro: /' "$results"
bad=0
n_exp="$(printf '%s\n' "$EXPECT" | wc -l)"
[ "$(wc -l <"$results")" -eq "$n_exp" ] || { echo "runner-tmp-repro: 行数が違う（期待 $n_exp）" >&2; bad=1; }
i=0
while IFS= read -r pat; do
  i=$((i + 1))
  got="$(sed -n "${i}p" "$results")"
  pat_re="$(printf '%s' "$pat" | sed 's/[.]/\\./g')"
  if ! printf '%s\n' "$got" | grep -Eqx -- "$pat_re"; then
    echo "runner-tmp-repro: 期待と違う: 期待 '$pat' / 観測 '$got'" >&2
    bad=1
  fi
done <<<"$EXPECT"
[ "$bad" -eq 0 ] || exit 1
echo "runner-tmp-repro: ok（$(grep -c ' rc=' "$results") case が期待表と一致。一時 dir が残るのは SIGKILL の 4 case。子が使用中の TMPDIR を trap が消した case: $(grep -c 'child_tmp=fail' "$results")）"
