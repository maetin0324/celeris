#!/usr/bin/env bash
# scripts/selfdeploy/tests/repo_health_bundle.sh — release に repo health の検査器を同梱する（lib.sh の sd_bundle_repo_health）。
#
# ADR 2026-11-08-health-enforcement-loop D1.4: base に検査器の無い repo には、Celeris の release に同梱した検査器
# （<release>/repo-health/）を使う。同梱は build した commit の scripts/dev/repo-health/（profiles/ を除く）と
# scripts/dev/source-size-report.py を、task-dispatch の with_base_checker が base から `git archive` で出すときと同じ
# repo 相対の配置で置き、CHECKER_TREE に tree id と blob id の 2 行を書く（daemon が git なしで checker.sha256 を計算する）。
#
# 試験は release 全体を起こさず、一時 git repo の fixture に対して関数を呼ぶ（本物の release.sh は走らせない）。
#   1. release.sh が sd_bundle_repo_health "$BUILD" "$STAGE/repo-health" を呼ぶ
#   2. 写した file が repo 相対の配置であり、内容が commit と同じ（作業ツリーの未 commit の変更・未追跡 file は入らない）
#   3. profiles/ は写さない
#   4. CHECKER_TREE の 2 行が git rev-parse HEAD:<path> の値と一致する
#   5. 写せないとき（検査器の dir・repo_health.py・source-size-report.py が無い、git でない）は sd_die（exit 1）する
#   6. release.sh・lib.sh に bash の構文の誤りが無い（bash -n。release.sh は起動しない）
# 時計・負荷・network に依らず決定的。
# 実行: bash scripts/selfdeploy/tests/repo_health_bundle.sh
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/.." && pwd)"
root="$(mktemp -d "${TMPDIR:-/tmp}/repo-health-bundle.XXXXXX")"
trap 'rm -rf "$root"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

export CELERIS_STATE_DIR="$root/state" CELERIS_CONFIG_DIR="$root/cfg" SD_LOG_FILE=""
# 「git でない」場合の fixture が、$TMPDIR の上にある別の repo を見つけないようにする
export GIT_CEILING_DIRECTORIES="$root"
mkdir -p "$CELERIS_STATE_DIR" "$CELERIS_CONFIG_DIR"

# 1. release.sh が同梱の関数を呼ぶこと（bundle_includes_verify_helpers.sh と同じ形の grep）
grep -Eq '^[[:space:]]*sd_bundle_repo_health[[:space:]]+"\$BUILD"[[:space:]]+"\$STAGE/repo-health"' "$SRC/release.sh" \
  || fail "release.sh does not bundle via sd_bundle_repo_health \"\$BUILD\" \"\$STAGE/repo-health\""

# fixture: 検査器の file と profiles/ を commit した一時 git repo
repo="$root/repo"
rh="scripts/dev/repo-health"
mkdir -p "$repo/$rh/health" "$repo/$rh/rules" "$repo/$rh/schema/examples" "$repo/$rh/profiles"
printf '#!/usr/bin/env python3\nprint("checker")\n' >"$repo/$rh/repo_health.py"
chmod +x "$repo/$rh/repo_health.py"
printf '# validate\n' >"$repo/$rh/validate_report.py"
printf '' >"$repo/$rh/health/__init__.py"
printf '# engine\n' >"$repo/$rh/health/engine.py"
printf '# rule\n' >"$repo/$rh/rules/size_growth.py"
printf '{}\n' >"$repo/$rh/schema/report-v1.schema.json"
printf '{}\n' >"$repo/$rh/schema/examples/ok.json"
printf '[review]\n' >"$repo/$rh/profiles/celeris.toml"
printf '[rules]\n' >"$repo/$rh/profiles/default.toml"
printf '# size report\n' >"$repo/scripts/dev/source-size-report.py"
printf 'other\n' >"$repo/scripts/dev/other.txt"
git init -q "$repo"
git -C "$repo" add -A
git -C "$repo" -c user.email=t@example.invalid -c user.name=t commit -q -m checker
# 作業ツリーの未 commit の変更と未追跡の file（__pycache__ など）は同梱に入らない（build した commit の内容だけ）
printf '# dirty\n' >>"$repo/$rh/health/engine.py"
mkdir -p "$repo/$rh/__pycache__"
printf 'x' >"$repo/$rh/__pycache__/repo_health.cpython.pyc"

# 2. 同梱を一時 dir に対して行う（subshell: sd_die の exit を閉じ込める）
dest="$root/rel/repo-health"
( . "$SRC/lib.sh"; sd_bundle_repo_health "$repo" "$dest" ) || fail "sd_bundle_repo_health failed on the fixture"

n=0
for f in "$rh/repo_health.py" "$rh/validate_report.py" "$rh/health/__init__.py" "$rh/health/engine.py" \
  "$rh/rules/size_growth.py" "$rh/schema/report-v1.schema.json" "$rh/schema/examples/ok.json" \
  scripts/dev/source-size-report.py; do
  [ -f "$dest/$f" ] || fail "bundle lacks $f"
  [ "$(git -C "$repo" hash-object "$dest/$f")" = "$(git -C "$repo" rev-parse "HEAD:$f")" ] \
    || fail "bundled $f differs from the committed blob"
  n=$((n + 1))
done
[ -x "$dest/$rh/repo_health.py" ] || fail "bundled repo_health.py lost its executable bit"
[ ! -e "$dest/$rh/__pycache__" ] || fail "an untracked __pycache__ was bundled"
[ ! -e "$dest/scripts/dev/other.txt" ] || fail "a file outside the checker was bundled"
[ ! -e "$dest/.checker.tar" ] || fail "the temporary archive was left in the bundle"

# 3. profiles/ は写さない
[ ! -e "$dest/$rh/profiles" ] || fail "profiles/ was bundled"

# 4. CHECKER_TREE の 2 行が git rev-parse の値と一致する
want="$(printf 'repo-health %s\nsource-size-report %s' \
  "$(git -C "$repo" rev-parse "HEAD:$rh")" "$(git -C "$repo" rev-parse HEAD:scripts/dev/source-size-report.py)")"
[ "$(cat "$dest/CHECKER_TREE")" = "$want" ] \
  || fail "CHECKER_TREE is wrong: got [$(cat "$dest/CHECKER_TREE")] want [$want]"
[ "$(wc -l <"$dest/CHECKER_TREE" | tr -d ' ')" = 2 ] || fail "CHECKER_TREE does not have exactly 2 lines"

# 同じ dest に 2 回目を呼んでも前回の中身が混ざらない（release.sh は同じ sha を作り直すことがある）
printf 'stale\n' >"$dest/stale.txt"
( . "$SRC/lib.sh"; sd_bundle_repo_health "$repo" "$dest" ) || fail "the second sd_bundle_repo_health failed"
[ ! -e "$dest/stale.txt" ] || fail "a stale file survived re-bundling"

# 5. 写せないときは sd_die する（黙って省かない）
expect_die() { # <名前> <build> <error に含む語>
  local name="$1" build="$2" word="$3" rc
  set +e
  ( . "$SRC/lib.sh"; sd_bundle_repo_health "$build" "$root/bad-$name" ) 2>"$root/bad-$name.err"
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || fail "$name: bundling did not fail"
  grep -q -- "$word" "$root/bad-$name.err" || fail "$name: error does not mention $word: $(cat "$root/bad-$name.err")"
}
mk_repo() { # <dir> — fixture の写しから <dir> の repo を作る（呼び出し側が消す file を消してから commit する）
  rm -rf "$1"; mkdir -p "$1"
  git -C "$repo" archive HEAD | tar -x -C "$1"
  git init -q "$1"
}
commit_repo() { git -C "$1" add -A && git -C "$1" -c user.email=t@example.invalid -c user.name=t commit -q -m c; }

no_side="$root/no-side"; mk_repo "$no_side"; rm -f "$no_side/scripts/dev/source-size-report.py"; commit_repo "$no_side"
expect_die no-side "$no_side" "source-size-report.py"

no_script="$root/no-script"; mk_repo "$no_script"; rm -f "$no_script/$rh/repo_health.py"; commit_repo "$no_script"
expect_die no-script "$no_script" "repo_health.py"

no_dir="$root/no-dir"; mk_repo "$no_dir"; rm -rf "${no_dir:?}/$rh"; commit_repo "$no_dir"
expect_die no-dir "$no_dir" "$rh"

not_git="$root/not-git"; mkdir -p "$not_git"
expect_die not-git "$not_git" "$rh"

# 6. release.sh・lib.sh の構文（release.sh は起動しない）
bash -n "$SRC/release.sh" || fail "release.sh has a syntax error"
bash -n "$SRC/lib.sh" || fail "lib.sh has a syntax error"

echo "OK: repo_health_bundle: $n checker files bundled at repo-relative paths; profiles/ excluded; CHECKER_TREE matches git rev-parse; missing checker dies; release.sh and lib.sh pass bash -n"
