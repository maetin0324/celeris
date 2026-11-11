#!/usr/bin/env bash
# scripts/selfdeploy/sync-skills.sh <release dir> <knowledge root> — ADR 2026-11-11-cos-skill-sync D2.
#
# The repo is the source of the CoS skills; the KB is where they are deployed. Each
# `<release>/skills/<name>/` (release.sh bundles the names of cos-skills.txt) replaces the KB's
# `skills/<name>/` as a whole (files the release lacks are removed). When the KB is a git work tree,
# only those paths are committed (`git commit --only`), so nothing else in the KB — pages curation
# wrote, its uncommitted changes, other skills — is touched. Exit 0 also when nothing changed.
set -euo pipefail
[ "$#" -eq 2 ] || { echo "usage: sync-skills.sh <release dir> <knowledge root>" >&2; exit 2; }
REL="$1"
KB="$2"
SRC="$REL/skills"
[ -d "$SRC" ] || { echo "sync-skills: $SRC is missing (release built before the CoS skill bundle?)" >&2; exit 1; }
mkdir -p "$KB/skills"
paths=()
for dir in "$SRC"/*/; do
  name="$(basename "$dir")"
  tmp="$KB/skills/.$name.sync-tmp"
  rm -rf "$tmp"
  cp -R "$dir" "$tmp"
  rm -rf "${KB:?}/skills/$name"
  mv "$tmp" "$KB/skills/$name"
  paths+=("skills/$name")
  echo "sync-skills: $name -> $KB/skills/$name"
done
[ "${#paths[@]}" -gt 0 ] || { echo "sync-skills: no skill under $SRC" >&2; exit 1; }
if ! git -C "$KB" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "sync-skills: $KB is not a git work tree; copied without a commit"
  exit 0
fi
git -C "$KB" add -A -- "${paths[@]}"
if git -C "$KB" diff --cached --quiet -- "${paths[@]}"; then
  echo "sync-skills: the KB already had these skills; no commit"
  exit 0
fi
git -C "$KB" -c user.name="${SD_KB_GIT_NAME:-celeris selfdeploy}" -c user.email="${SD_KB_GIT_EMAIL:-selfdeploy@celeris.invalid}" \
  commit --quiet --only -m "selfdeploy: sync CoS skills from release $(basename "$REL")" -- "${paths[@]}"
echo "sync-skills: committed $(git -C "$KB" rev-parse --short HEAD)"
