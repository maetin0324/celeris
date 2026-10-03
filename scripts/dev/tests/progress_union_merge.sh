#!/bin/sh
# ADR agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md 付記（union の範囲）の前提を
# 確かめる。.gitattributes の `merge=union` は凍結済みの `agent-docs/PROGRESS.md` と移行期間の旧
# `docs/PROGRESS.md` の 2 行だけに限る（人の方針）。task ごとの進捗ファイル
# `agent-docs/progress/**/*.md` は front matter（`status:` 等）を書き換えるので union を使わない:
# 両側が front matter の同じ行を別の値へ変えたら `git merge` は衝突して止まり（黙って片方を残して
# もう片方を消さない）、records resolver（crates/task-dispatch/src/auto_resolve/records.rs）が
# 人の判断（NeedsHuman）へ回す。
# 一時 git repo の中だけで完結し、外部ネットワーク・本番 repo には触れない。
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

attrs="$repo_root/.gitattributes"
[ -f "$attrs" ] || fail "not found: $attrs"
grep -q '^agent-docs/PROGRESS\.md merge=union$' "$attrs" \
  || fail "$attrs is missing 'agent-docs/PROGRESS.md merge=union'"
grep -q '^docs/PROGRESS\.md merge=union$' "$attrs" \
  || fail "$attrs is missing 'docs/PROGRESS.md merge=union'"
if grep -q 'agent-docs/progress/' "$attrs"; then
  fail "$attrs must not set merge=union under agent-docs/progress/ (front matter is rewritten there, not append-only)"
fi
echo "OK: .gitattributes limits merge=union to agent-docs/PROGRESS.md and docs/PROGRESS.md"

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT INT TERM

export HOME="$root"
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME=progress-union-test
export GIT_AUTHOR_EMAIL=progress-union-test@example.invalid
export GIT_COMMITTER_NAME=progress-union-test
export GIT_COMMITTER_EMAIL=progress-union-test@example.invalid

repo="$root/repo"
mkdir -p "$repo"
cd "$repo"
if ! git init -q -b main >/dev/null 2>&1; then
  git init -q >/dev/null 2>&1
  git symbolic-ref HEAD refs/heads/main
fi
cp "$attrs" .gitattributes

# (a) 凍結された追記専用ファイルは、共通の祖先から両側がそれぞれ末尾に別の節を追記しただけなら
# `git merge` を衝突なく終え、両方の節を残す。
check_frozen_append() {
  rel_path=$1
  label=$2
  dir=$(dirname -- "$rel_path")
  [ "$dir" = "." ] || mkdir -p "$dir"

  printf '# Progress (%s)\n\n## common\nshared line\n' "$label" > "$rel_path"
  git add .gitattributes "$rel_path"
  git commit -q -m "base ($label)"

  git checkout -q -b "a-$label"
  printf '\n## section A (%s)\nline from A\n' "$label" >> "$rel_path"
  git commit -q -am "a ($label)"

  git checkout -q main
  git checkout -q -b "b-$label"
  printf '\n## section B (%s)\nline from B\n' "$label" >> "$rel_path"
  git commit -q -am "b ($label)"

  git checkout -q "a-$label"
  if ! git merge -q --no-edit "b-$label" >"$root/merge-$label.log" 2>&1; then
    fail "git merge conflicted for $rel_path ($label): $(cat "$root/merge-$label.log")"
  fi
  if grep -q '<<<<<<<' "$rel_path"; then
    fail "conflict markers left in $rel_path ($label) after git merge"
  fi
  grep -q "section A ($label)" "$rel_path" || fail "section A missing after git merge for $label"
  grep -q "section B ($label)" "$rel_path" || fail "section B missing after git merge for $label"
  echo "OK: git merge kept both sections for $rel_path ($label)"

  git checkout -q main
}

check_frozen_append agent-docs/PROGRESS.md agent-docs-progress-md
check_frozen_append docs/PROGRESS.md docs-progress-md

# (b) task ごとの進捗ファイルは front matter の既存行を両側が別の値に変えると、union が効かないので
# `git merge` が衝突して止まる（黙って片方を残してもう片方を消さない）。
rel_path=agent-docs/progress/2026-10-03-example/leaf.md
dir=$(dirname -- "$rel_path")
mkdir -p "$dir"
cat > "$rel_path" <<'EOF'
---
title: 例
tasks: [example]
status: running
updated: 2026-10-03
---

# 例

本文
EOF
git add .gitattributes "$rel_path"
git commit -q -m "base (front-matter)"

git checkout -q -b a-front-matter
sed -i 's/^status: running$/status: done/' "$rel_path"
git commit -q -am "a (front-matter): status done"

git checkout -q main
git checkout -q -b b-front-matter
sed -i 's/^status: running$/status: blocked/' "$rel_path"
git commit -q -am "b (front-matter): status blocked"

git checkout -q a-front-matter
if git merge -q --no-edit b-front-matter >"$root/merge-front-matter.log" 2>&1; then
  fail "git merge for $rel_path must conflict (both sides changed the same status: line) but it exited 0: $(cat "$root/merge-front-matter.log")"
fi
grep -q '<<<<<<<' "$rel_path" || fail "expected conflict markers in $rel_path after git merge"
git merge --abort
git checkout -q main

echo "OK: front matter conflict is not silently merged"

echo "progress_union_merge: all checks passed"
