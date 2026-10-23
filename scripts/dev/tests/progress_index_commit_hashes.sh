#!/bin/sh
# progress-index.sh --check の commit hash 検査（agent-docs/adr/2026-10-23-commit-refs-by-subject.md D4）を
# 一時 git repo で確かめる。外部ネットワーク・本番 repo には触れない。
#   (1) 件名だけ・main 側の commit の hash・ULID・数字だけの語は通る
#   (2) rebase 前の task branch の commit（HEAD の祖先でない）の hash は失敗する
#   (3) rebase 後の task branch の commit（HEAD の祖先だが main の祖先でない）の hash も失敗する
#   (4) commit として解決できない hex も失敗する
#   (5) この branch が変えていない進捗ファイルの hash は見ない
set -eu

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

script_path=$0
case "$script_path" in
  /*) : ;;
  *) script_path="$PWD/$script_path" ;;
esac
script_dir=$(dirname -- "$script_path")
repo_root=$(CDPATH='' cd -- "$script_dir/../../.." && pwd -P)

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT INT TERM

export HOME="$root"
export GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
unset CELERIS_WU_BASE 2>/dev/null || true

repo="$root/repo"
d=agent-docs/progress
mkdir -p "$repo/scripts/dev" "$repo/$d"
cp "$repo_root/scripts/dev/progress-index.sh" "$repo/scripts/dev/"
cd "$repo"
git init -q -b main
fm() {
  printf -- '---\ntitle: t\ntasks: [01M5MQ6SXVA1WQYJNDG7VJRW8T]\nstatus: running\nupdated: 2026-10-23\n---\n# t\n'
}
old="$d/2026-10-01-old.md"
fm >"$old"
echo 'deadbeef1234 は消えた commit（main の既存ファイル。この branch は変えない）' >>"$old"
git add -A
git commit -q -m "base"
main_sha=$(git rev-parse HEAD)

git switch -q -c task
echo work >work.txt
git add work.txt
git commit -q -m "phase 1: work"
pre_rebase=$(git rev-parse HEAD)

progress="$d/2026-10-23-new.md"
check() {
  sh scripts/dev/progress-index.sh --check >"$root/out" 2>"$root/err"
}

# (1)
fm >"$progress"
{
  echo '- phase 1: work（件名で書く）'
  echo "- main 側の commit ${main_sha} と短縮 $(printf %.12s "$main_sha")"
  echo '- run 01M5MQ757JWZE0FR7574NN3DZK・日付 20261023・ADR-0128'
} >>"$progress"
check || fail "(1) subject / main hash / ULID must pass: $(cat "$root/out")"
grep -q 'ok' "$root/err" || fail "(1) expected ok on stderr"
echo "OK (1) subjects, main-side hashes, ULIDs and digits pass"

# (2) rebase 前の hash: main を進めて task を rebase すると pre_rebase は HEAD の祖先でなくなる
git switch -q main
echo m >main.txt
git add main.txt
git commit -q -m "main moves"
git switch -q task
git rebase -q main
git merge-base --is-ancestor "$pre_rebase" HEAD && fail "setup: pre-rebase commit is still an ancestor"
echo "- 検証した commit $(printf %.8s "$pre_rebase")" >>"$progress"
if check; then fail "(2) pre-rebase hash must fail"; fi
grep -q "$progress:.*$(printf %.8s "$pre_rebase") は main の祖先でない" "$root/out" \
  || fail "(2) unexpected output: $(cat "$root/out")"
echo "OK (2) pre-rebase hash fails"

# (3) rebase 後の task branch の hash（HEAD の祖先）も失敗する
fm >"$progress"
echo "- 検証した commit $(git rev-parse --short=10 HEAD)" >>"$progress"
if check; then fail "(3) task-branch hash must fail"; fi
grep -q 'main の祖先でない' "$root/out" || fail "(3) unexpected output: $(cat "$root/out")"
echo "OK (3) task-branch hash (ancestor of HEAD only) fails"

# (4) 解決できない hex
fm >"$progress"
echo '- tested_sha: 0123456789abcdef0123' >>"$progress"
if check; then fail "(4) unresolvable hex must fail"; fi
grep -q '0123456789abcdef0123 は commit として解決できない' "$root/out" \
  || fail "(4) unexpected output: $(cat "$root/out")"
echo "OK (4) unresolvable hex fails"

# (5) 変えていない進捗ファイルは見ない（(1) の形に戻して通る。old.md の deadbeef1234 は残っている）
fm >"$progress"
echo '- phase 1: work' >>"$progress"
check || fail "(5) unchanged files must not be checked: $(cat "$root/out")"
echo "OK (5) files this branch did not change are not checked"

echo "progress_index_commit_hashes: all checks passed"
