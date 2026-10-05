#!/usr/bin/env bash
# scripts/selfdeploy/tests/promote_handoff_settled.sh — ADR-0040 付記 2026-10-05（規則 4）:
#
#   promote.sh は「handoff done」を、新が active になっただけでなく、旧が draining（または消えた）ことを
#   `GET /api/v1/releases` の `instances` で確かめてから言う。`sd_instances_settled` の判定を見る。
#
# 本番には触れない: 入力は一時ファイルの JSON。
# 実行: bash scripts/selfdeploy/tests/promote_handoff_settled.sh（lib.sh が bash の構文のため）
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }

# shellcheck source=/dev/null
. "$SD/lib.sh"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
NEW=0123456789ab
OLD=ba9876543210

check() {
  local desc="$1" want="$2" body="$3"
  printf '%s' "$body" >"$TMP/releases.json"
  if sd_instances_settled "$TMP/releases.json" "$NEW"; then got=0; else got=1; fi
  if [ "$got" = "$want" ]; then ok "$desc"; else ng "$desc: want exit $want, got $got"; fi
}

# 新だけが active → settled。
check "only the new instance is active" 0 \
  "{\"instances\":[{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# 旧が draining で新が active → settled（旧の drain を待つ必要はない）。
check "old is draining, new is active" 0 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\"},{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# 旧がまだ active（二重 active）→ not settled。
check "old is still active next to the new one" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\"},{\"release\":\"$NEW\",\"role\":\"active\"}]}"
# active が新でない（新が standby）→ not settled。
check "the new instance is only standby" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"active\"},{\"release\":\"$NEW\",\"role\":\"standby\"}]}"
# active が 1 つも無い → not settled。
check "no active instance" 1 \
  "{\"instances\":[{\"release\":\"$OLD\",\"role\":\"draining\"}]}"
# 本文が壊れている → not settled。
check "unreadable body" 1 "not json"

if [ "$FAIL" -ne 0 ]; then
  echo "FAILED" >&2
  exit 1
fi
echo "all promote_handoff_settled checks passed"
