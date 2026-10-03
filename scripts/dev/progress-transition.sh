#!/bin/sh
# Resolve the legacy PROGRESS rename against the current main document.
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: sh scripts/dev/progress-transition.sh <main-ref>" >&2
    exit 2
fi

main_ref=$1
if ! git cat-file -e "$main_ref:docs/PROGRESS.md" 2>/dev/null; then
    exit 0
fi

mkdir -p agent-docs
{
    printf '%s\n' '> **このファイルへの追記は終了（ADR-0128）。** 新しい進捗は agent-docs/progress/YYYY-MM-DD-<slug>.md へ。現在地は sh scripts/dev/progress-index.sh。'
    git show "$main_ref:docs/PROGRESS.md"
} > agent-docs/PROGRESS.md

git rm -f -q --ignore-unmatch -- docs/PROGRESS.md
git add -- agent-docs/PROGRESS.md
