#!/bin/sh
# ADR-0128 D7: scripts/dev/docs-layout.tsv どおりに docs/ の再配置が終わっているかを確かめる。
# 呼び方: sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv
#
# '#' で始まる行は無視する。残りの各行は <旧パス>\t<新パス>\t<human|agent|delete> の形:
#   - 新パスが '-'（delete）: 旧パスが存在しない（git rm 済み）ことを確かめる。
#   - 旧 != 新（move）: 旧パスが存在しない、かつ新パスが git の追跡対象であることを確かめる。
#   - 旧 == 新（unchanged）: 旧パスが存在することを確かめる。
# 違反を 1 行ずつ標準出力に出し、1 件でもあれば exit 1。POSIX sh（dash）。
set -eu

tsv="${1:-}"
if [ -z "$tsv" ] || [ ! -f "$tsv" ]; then
    echo "usage: sh scripts/dev/check-doc-layout.sh <tsv>" >&2
    exit 1
fi

tab="$(printf '\t')"
violations=0

while IFS="$tab" read -r old new kind || [ -n "$old" ]; do
    case "$old" in
        '#'*) continue ;;
        '') continue ;;
    esac

    if [ "$new" = "-" ]; then
        if [ -e "$old" ]; then
            echo "$old: delete 行だが旧パスがまだ存在する（kind=$kind）"
            violations=$((violations + 1))
        fi
        continue
    fi

    if [ "$old" = "$new" ]; then
        if [ ! -e "$old" ]; then
            echo "$old: 旧=新 の行だがファイルが存在しない（kind=$kind）"
            violations=$((violations + 1))
        fi
        continue
    fi

    if [ -e "$old" ]; then
        echo "$old: 旧≠新 の行だが旧パスがまだ存在する（新 $new, kind=$kind）"
        violations=$((violations + 1))
    fi
    if ! git ls-files --error-unmatch "$new" >/dev/null 2>&1; then
        echo "$new: 旧≠新 の行だが新パスが git の追跡対象ではない（旧 $old, kind=$kind）"
        violations=$((violations + 1))
    fi
done <"$tsv"

if [ "$violations" -gt 0 ]; then
    echo "check-doc-layout: $violations violation(s)" >&2
    exit 1
fi

echo "check-doc-layout: ok"
