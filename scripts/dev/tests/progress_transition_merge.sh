#!/bin/sh
# Exercise the transition in a throwaway repository, including a stale branch.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
transition=$script_dir/../progress-transition.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

git init -q -b base "$tmp/repo"
cd "$tmp/repo"
git config user.name 'Progress transition test'
git config user.email 'progress-transition@example.invalid'
mkdir docs agent-docs
{
    printf '# PROGRESS\n'
    n=1
    while [ "$n" -le 100 ]; do
        printf 'Original record %03d: historical detail for this task.\n' "$n"
        n=$((n + 1))
    done
} > docs/PROGRESS.md
git add docs/PROGRESS.md
git commit -qm base

git switch -qc main
{
    printf '# Reorganized PROGRESS\n\n## New records\n'
    n=1
    while [ "$n" -le 160 ]; do
        printf 'New record %03d: main-side detail and update.\n' "$n"
        n=$((n + 1))
    done
    printf '\n## Reordered history\n'
    git show base:docs/PROGRESS.md | awk '{ lines[NR] = $0 } END { for (i = NR; i > 0; i--) print lines[i] }'
    printf '\n## Latest main update\nMain-side append.\n'
} > docs/PROGRESS.md
git add docs/PROGRESS.md
git commit -qm 'reorganize and append on main'

git switch -qc branch-x
printf '\n## Branch X record\nBranch X appended this section.\n' >> docs/PROGRESS.md
git add docs/PROGRESS.md
git commit -qm 'append from branch X'

git switch -q base
git switch -qc task
git mv docs/PROGRESS.md agent-docs/PROGRESS.md
{
    printf '%s\n' '> **このファイルへの追記は終了（ADR-0128）。** 新しい進捗は agent-docs/progress/YYYY-MM-DD-<slug>.md へ。現在地は sh scripts/dev/progress-index.sh。'
    git show base:docs/PROGRESS.md
} > agent-docs/PROGRESS.md
git add agent-docs/PROGRESS.md
git commit -qm 'old transition from base'
git branch old-task task

# (a) Resolve main's modify/delete conflict from inside the merge.
if git merge --no-edit main > "$tmp/new-merge.log" 2>&1; then
    echo 'expected the stale task to conflict with reorganized main' >&2
    exit 1
fi
sh "$transition" main
test -z "$(git ls-files -u)" || { echo 'unresolved index after transition' >&2; exit 1; }
test -n "$(git rev-parse -q --verify MERGE_HEAD)"
if git diff --cached --quiet -- agent-docs/PROGRESS.md; then
    echo 'transition did not stage the replacement' >&2
    exit 1
fi
test ! -e docs/PROGRESS.md
sed '1d' agent-docs/PROGRESS.md > "$tmp/copied-main"
git show main:docs/PROGRESS.md > "$tmp/main-progress"
cmp "$tmp/main-progress" "$tmp/copied-main"
test "$(sed -n '1p' agent-docs/PROGRESS.md)" = '> **このファイルへの追記は終了（ADR-0128）。** 新しい進捗は agent-docs/progress/YYYY-MM-DD-<slug>.md へ。現在地は sh scripts/dev/progress-index.sh。'
git commit -qm 'resolve with current main document'

# (b) Main can fast-forward to the resolution; X's old-path append follows the rename.
git branch main-new main
git switch -q main-new
git merge --ff-only task > "$tmp/ff.log"
git merge --no-edit branch-x > "$tmp/x-new.log"
test -z "$(git ls-files -u)"
test ! -e docs/PROGRESS.md
grep -q '^## Branch X record$' agent-docs/PROGRESS.md

# A ref without the legacy path must leave the worktree and index untouched.
git branch no-progress base
git switch -q no-progress
git rm -q docs/PROGRESS.md
git commit -qm 'remove the legacy progress file'
git switch -q main-new
sh "$transition" no-progress
test -z "$(git status --porcelain)"
if sh "$transition" > "$tmp/usage.log" 2>&1; then
    echo 'transition accepted a missing argument' >&2
    exit 1
fi

# (c) The former resolution discarded main's body, so the same X merge conflicts.
git switch -q old-task
if git merge --no-edit main > "$tmp/old-merge.log" 2>&1; then
    echo 'expected the old task to conflict with reorganized main' >&2
    exit 1
fi
git show old-task:agent-docs/PROGRESS.md > agent-docs/PROGRESS.md
git add agent-docs/PROGRESS.md
git commit -qm 'old resolution kept the stale renamed copy'
git branch main-old main
git switch -q main-old
git merge --ff-only old-task > "$tmp/old-ff.log"
if git merge --no-edit branch-x > "$tmp/x-old.log" 2>&1; then
    echo 'old transition unexpectedly merged branch X cleanly' >&2
    exit 1
fi
test -n "$(git ls-files -u)" || { echo 'old transition did not leave a conflict' >&2; exit 1; }

echo 'progress transition: current-main resolution and branch X merge passed; old resolution conflicted as expected'
