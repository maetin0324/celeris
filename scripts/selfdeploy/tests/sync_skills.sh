#!/usr/bin/env bash
# ADR 2026-11-11-cos-skill-sync D2: release.sh bundles the CoS skills of cos-skills.txt, and
# sync-skills.sh replaces the KB's skills/<name>/ with them and commits only those paths — the pages
# curation wrote (committed, modified or staged) stay as they were. Run by crates/celeris/tests/cos_skill_sync.rs.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
T="$(mktemp -d "${TMPDIR:-/tmp}/sync-skills.XXXXXX")"
trap 'rm -rf "$T"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
g() { git -C "$T/kb" -c user.name=t -c user.email=t@t "$@"; }

# release.sh's bundle step
( SD_LOG_FILE=/dev/null; . "$REPO/scripts/selfdeploy/lib.sh"; sd_bundle_cos_skills "$REPO" "$T/rel" ) || fail "bundle"
for name in cos-operator cos-inbox-triage; do
  diff -r "$REPO/config/skills/$name" "$T/rel/skills/$name" || fail "bundle of $name differs"
done
[ ! -e "$T/rel/skills/ui-ux-quality-gate" ] || fail "a non-CoS skill was bundled"

# a KB with an old triage skill, a stale file, and curation's pages
mkdir -p "$T/kb/skills/cos-inbox-triage" "$T/kb/skills/cos-operator" "$T/kb/skills/yomiyasu" "$T/kb/projects"
g init -q
printf -- '---\nname: cos-inbox-triage\nmetadata:\n  version: "10"\n---\n' >"$T/kb/skills/cos-inbox-triage/SKILL.md"
echo stale >"$T/kb/skills/cos-operator/stale.md"
echo keep >"$T/kb/skills/yomiyasu/SKILL.md"
echo page >"$T/kb/projects/page.md"
g add -A && g commit -qm seed
echo "curation edit" >>"$T/kb/projects/page.md"
echo new >"$T/kb/projects/new.md" && g add projects/new.md
before="$(g rev-parse HEAD)"

bash "$REPO/scripts/selfdeploy/sync-skills.sh" "$T/rel" "$T/kb" || fail "sync exit $?"
for name in cos-operator cos-inbox-triage; do
  diff -r "$REPO/config/skills/$name" "$T/kb/skills/$name" || fail "KB $name differs from the repo"
done
[ "$(g rev-list --count "$before"..HEAD)" = 1 ] || fail "expected one commit"
changed="$(g show --name-only --format= HEAD)"
printf '%s\n' "$changed" | grep -qv '^skills/cos-' && fail "the commit touched other paths: $changed"
printf '%s\n' "$changed" | grep -q '^skills/cos-operator/stale.md$' || fail "stale.md was not removed"
[ "$(cat "$T/kb/projects/page.md")" = "$(printf 'page\ncuration edit')" ] || fail "curation's page changed"
[ "$(cat "$T/kb/skills/yomiyasu/SKILL.md")" = keep ] || fail "another skill changed"
g diff --cached --name-only | grep -qx projects/new.md || fail "curation's staged page was not left staged"
[ -z "$(g status --porcelain -- skills)" ] || fail "skills/ is not clean after the sync"

# a second sync of the same release adds no commit
bash "$REPO/scripts/selfdeploy/sync-skills.sh" "$T/rel" "$T/kb" >/dev/null || fail "second sync"
[ "$(g rev-list --count "$before"..HEAD)" = 1 ] || fail "second sync committed again"

# promote/rollback find the KB from `[knowledge] root` of the config; an unreadable config fails
# (exit 4 of promote.sh) instead of syncing a guessed default KB.
mkdir -p "$T/cfg"
printf '[knowledge]\nroot = "kb-here"\n' >"$T/cfg/rel.toml"
printf '[cos]\nenabled = true\n' >"$T/cfg/nokey.toml"
printf '[knowledge\nroot = "x"\n' >"$T/cfg/bad.toml"
root_of() ( unset SD_KNOWLEDGE_ROOT; SD_LOG_FILE=/dev/null; . "$REPO/scripts/selfdeploy/lib.sh"; SD_CONFIG="$1"; sd_knowledge_root )
[ "$(root_of "$T/cfg/rel.toml")" = "$T/cfg/kb-here" ] || fail "relative [knowledge] root"
[ "$(HOME="$T/home" root_of "$T/cfg/nokey.toml")" = "$T/home/.local/share/celeris/knowledge" ] || fail "default root"
if root_of "$T/cfg/bad.toml" >/dev/null 2>&1; then fail "an unreadable config resolved a KB root"; fi
head="$(g rev-parse HEAD)"
if ( unset SD_KNOWLEDGE_ROOT; SD_LOG_FILE=/dev/null; . "$REPO/scripts/selfdeploy/lib.sh"; SD_CONFIG="$T/cfg/bad.toml"; sd_sync_cos_skills "$T/rel" ); then
  fail "sd_sync_cos_skills succeeded with an unreadable config"
fi
[ "$(g rev-parse HEAD)" = "$head" ] || fail "the KB changed although the config was unreadable"
echo "sync_skills: ok"
