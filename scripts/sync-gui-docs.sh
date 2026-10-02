#!/usr/bin/env bash
# celeris が持つ API 仕様（docs/api/v1/gui-api.md）を GUI 側の写し（gui/docs/celeris-api-v1.md）に反映する（ADR-0020 D4）。
#
#   scripts/sync-gui-docs.sh          写しを更新する（変わったら 0、変更なしでも 0）
#   scripts/sync-gui-docs.sh --check  ずれているかだけ見る（ずれていたら exit 1。CI / ランナー用）
#
# 同期するのは **celeris が正である 1 ファイルだけ**。gui/docs/DESIGN.md や gui/docs/adr/* は GUI 側が育てるので触らない。
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO/docs/api/v1/gui-api.md"
DST="${GUI_REPO:-$REPO/gui}/docs/celeris-api-v1.md"
CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

[ -f "$SRC" ] || { echo "sync-gui-docs: $SRC が無い" >&2; exit 2; }
[ -d "$(dirname "$DST")" ] || { echo "sync-gui-docs: $(dirname "$DST") が無い（GUI をまだ立ち上げていない）" >&2; exit 0; }

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
# 写し先ではファイル名が celeris-api-v1.md になる。正本と同じディレクトリへの相対リンクは写し先（gui/docs/）から正本の位置へ向け直す。
UP="../.."
sed -E -e 's#(^|[^-A-Za-z0-9_])gui-api\.md#\1celeris-api-v1.md#g' \
  -e "s#\\]\\((overview\.md|api-v1\.schema\.json|event\.schema\.json)([#)])#](${UP}/docs/api/v1/\1\2#g" "$SRC" > "$tmp"

if cmp -s "$tmp" "$DST"; then
  echo "sync-gui-docs: up to date"
  exit 0
fi
if [ "$CHECK" = "1" ]; then
  echo "sync-gui-docs: $DST が docs/api/v1/gui-api.md とずれている（scripts/sync-gui-docs.sh で更新する）" >&2
  diff -u "$DST" "$tmp" | head -40 >&2
  exit 1
fi
cp "$tmp" "$DST"
echo "sync-gui-docs: updated $DST"
