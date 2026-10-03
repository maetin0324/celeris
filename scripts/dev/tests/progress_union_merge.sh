#!/bin/sh
# ADR 2026-10-02-parallel-integration-auto-resolve records resolver の前提を確かめる: .gitattributes の
# `agent-docs/PROGRESS.md merge=union` / `agent-docs/progress/*.md merge=union` /
# `agent-docs/progress/**/*.md merge=union`（入れ子の <slug>/<unit>.md）が、共通の祖先から
# 2 branch がそれぞれ末尾に別の節を追記しただけのときに、`git merge` を衝突なく終え
# 両方の節を残すこと。`git merge-tree --write-tree` でも同じ結果になるかも確かめるが、
# 効かなくても（merge-tree は attributes を同じようには扱わない実装上の罠があるため）
# このスクリプト自体は失敗にせず、結果を出力に書くだけにする。
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
grep -q '^agent-docs/progress/\*\.md merge=union$' "$attrs" \
  || fail "$attrs is missing 'agent-docs/progress/*.md merge=union'"
grep -q '^agent-docs/progress/\*\*/\*\.md merge=union$' "$attrs" \
  || fail "$attrs is missing 'agent-docs/progress/**/*.md merge=union'"
echo "OK: .gitattributes declares merge=union for agent-docs/PROGRESS.md and agent-docs/progress/ (nested included)"

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

# check_union <relative-path> <label>
# 共通の祖先に <relative-path> を作り、a-<label>/b-<label> の 2 branch がそれぞれ
# 末尾に別の節を追記したあと main の代わりの a-<label> へ b-<label> を merge する。
check_union() {
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

  mt_out=$(git merge-tree --write-tree "a-$label" "b-$label" 2>"$root/mt-$label.err") && mt_status=0 || mt_status=$?
  if [ "$mt_status" -eq 0 ]; then
    tree=$(printf '%s\n' "$mt_out" | head -n1)
    if git cat-file -e "$tree:$rel_path" 2>/dev/null; then
      blob=$(git show "$tree:$rel_path")
      if printf '%s' "$blob" | grep -q "section A ($label)" && printf '%s' "$blob" | grep -q "section B ($label)"; then
        echo "INFO: git merge-tree --write-tree also kept both sections for $rel_path ($label)"
      else
        echo "INFO: git merge-tree --write-tree produced a tree for $rel_path ($label) without both sections; the records resolver (ADR 2026-10-02-parallel-integration-auto-resolve) must not rely on merge-tree alone for this path"
      fi
    else
      echo "INFO: git merge-tree --write-tree result has no $rel_path ($label); the records resolver (ADR 2026-10-02-parallel-integration-auto-resolve) must not rely on merge-tree alone for this path"
    fi
  else
    mt_err=$(tr '\n' ' ' < "$root/mt-$label.err")
    echo "INFO: git merge-tree --write-tree reported a conflict for $rel_path ($label) even though .gitattributes requests merge=union ($mt_err); the records resolver (ADR 2026-10-02-parallel-integration-auto-resolve) must not rely on merge-tree alone for this path"
  fi

  git checkout -q main
}

check_union agent-docs/PROGRESS.md progress
check_union agent-docs/progress/2026-10-03-example.md progress-dir
check_union agent-docs/progress/2026-10-03-example/unit.md progress-nested

echo "progress_union_merge: all checks passed"
