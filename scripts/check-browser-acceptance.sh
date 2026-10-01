#!/usr/bin/env bash
# Browser capability Phase 1〜4 受け入れ行の追跡表（docs/progress/phase-browser-acceptance.md）を検査する。
#
# 1. 表の各行（`| P…` と `| A…`）の判定が `合格`・`後続`・`未達`。合格行は `cmd:` と `test:`、後続行は ULID の task id、未達行は `理由:` と `後続:` を持つ。A1〜A17 が全部ある。
# 2. 文書中の `test: \`<名前>\`` の各名前が crates/ 配下に `fn <名前>` として実在する。
# 3. 決定適合節の `crates/...rs:N` が実在ファイルで、N がその行数以内。
# 4. 引用した `cmd:` を (crate, target) ごとにまとめて cargo test で実行し、全部 exit 0 かつ引用した各テストが `... ok` と出る。
#
# 引数なし。CARGO_TARGET_DIR などは環境のまま使う。失敗したら理由を出して非 0。
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 2
DOC="docs/progress/phase-browser-acceptance.md"

fail=0
err() {
  echo "FAIL: $*" >&2
  fail=1
}

if [[ ! -f "$DOC" ]]; then
  echo "FAIL: $DOC がない" >&2
  exit 1
fi

ULID_RE='[0-9A-HJKMNP-TV-Z]{26}'

# ---- 1. 表の行 ----
rows=0
declare -A seen_a=()
while IFS= read -r line; do
  rows=$((rows + 1))
  id="$(awk -F'|' '{gsub(/^ +| +$/, "", $2); print $2}' <<<"$line")"
  verdict="$(awk -F'|' '{gsub(/^ +| +$/, "", $4); print $4}' <<<"$line")"
  evidence="$(awk -F'|' '{print $5}' <<<"$line")"
  [[ "$id" =~ ^A([0-9]+)$ ]] && seen_a[${BASH_REMATCH[1]}]=1
  case "$verdict" in
    合格)
      grep -q 'cmd: `cargo test ' <<<"$evidence" || err "$id: 合格行に cmd: がない"
      grep -q 'test: `' <<<"$evidence" || err "$id: 合格行に test: がない"
      ;;
    後続)
      grep -Eq "task: \`?$ULID_RE" <<<"$evidence" || err "$id: 後続行に ULID の task id がない"
      ;;
    未達)
      grep -q '理由:' <<<"$evidence" || err "$id: 未達行に 理由: がない"
      grep -q '後続:' <<<"$evidence" || err "$id: 未達行に 後続: がない"
      ;;
    *)
      err "$id: 判定が '合格'・'後続'・'未達' のいずれでもない（'$verdict'）"
      ;;
  esac
done < <(grep -E '^\| (P|A)[0-9A-Za-z-]* \|' "$DOC")
[[ $rows -gt 0 ]] || err "表の行が 1 つもない"
for i in $(seq 1 17); do
  [[ -n "${seen_a[$i]:-}" ]] || err "A$i の行がない"
done
echo "rows: $rows"

# ---- 2. 引用したテスト名が crates/ に実在する ----
mapfile -t tests < <(grep -oE 'test: `[A-Za-z0-9_]+`' "$DOC" | sed -E 's/test: `(.*)`/\1/' | sort -u)
[[ ${#tests[@]} -gt 0 ]] || err "test: の引用が 1 つもない"
for t in "${tests[@]}"; do
  grep -rqE --include='*.rs' "fn ${t}\b" crates/ || err "test \`$t\` が crates/ に fn として無い"
done
echo "tests cited: ${#tests[@]}"

# ---- 3. 決定適合節の file:line ----
refs=0
while IFS= read -r ref; do
  refs=$((refs + 1))
  path="${ref%:*}"
  n="${ref##*:}"
  if [[ ! -f "$path" ]]; then
    err "$ref: ファイルが無い"
    continue
  fi
  lines="$(wc -l <"$path")"
  if [[ "$n" -lt 1 || "$n" -gt "$lines" ]]; then
    err "$ref: $path は $lines 行"
  fi
done < <(sed -n '/^## 決定適合/,$p' "$DOC" | grep -oE 'crates/[A-Za-z0-9_./-]+\.rs:[0-9]+' | sort -u)
[[ $refs -gt 0 ]] || err "決定適合節に crates/...rs:N が無い"
echo "file:line refs: $refs"

if [[ $fail -ne 0 ]]; then
  echo "check-browser-acceptance: 静的検査で失敗（cargo test は実行しない）" >&2
  exit 1
fi

# ---- 4. cmd: を (crate, target) ごとにまとめて実行 ----
# 形は `cargo test -p <crate> --lib [filter]` か `cargo test -p <crate> --test <name> [filter]`。
declare -A filters=()
declare -A unfiltered=()
while IFS= read -r cmd; do
  read -r -a w <<<"$cmd"
  if [[ "${w[0]}" != cargo || "${w[1]}" != test || "${w[2]}" != -p ]]; then
    err "解釈できない cmd: $cmd"
    continue
  fi
  crate="${w[3]}"
  if [[ "${w[4]}" == --lib ]]; then
    key="$crate --lib"
    filter="${w[5]:-}"
    extra="${w[6]:-}"
  elif [[ "${w[4]}" == --test && -n "${w[5]:-}" ]]; then
    key="$crate --test ${w[5]}"
    filter="${w[6]:-}"
    extra="${w[7]:-}"
  else
    err "解釈できない cmd: $cmd"
    continue
  fi
  [[ -z "$extra" ]] || err "filter の後に引数がある cmd は扱わない: $cmd"
  if [[ -z "$filter" ]]; then
    unfiltered[$key]=1
  else
    filters[$key]="${filters[$key]:-} $filter"
  fi
  filters[$key]="${filters[$key]:-}"
done < <(grep -oE 'cmd: `[^`]+`' "$DOC" | sed -E 's/cmd: `(.*)`/\1/' | sort -u)
[[ $fail -eq 0 ]] || exit 1

log="$(mktemp)"
errlog="$(mktemp)"
trap 'rm -f "$log" "$errlog"' EXIT
mapfile -t keys < <(printf '%s\n' "${!filters[@]}" | sort)
for key in "${keys[@]}"; do
  read -r -a kw <<<"$key"
  args=()
  if [[ -z "${unfiltered[$key]:-}" ]]; then
    read -r -a fs <<<"${filters[$key]}"
    mapfile -t args < <(printf '%s\n' "${fs[@]}" | sort -u)
  fi
  echo "==> cargo test -p ${kw[*]} -- ${args[*]}"
  # `test <名前> ... ok` の判定は stdout だけで行う。test の eprintln!（stderr）が同じ行に割り込むと
  # `... ok` が行頭から切れて偽の不合格になるため、stderr は別に取って失敗時だけ出す。
  out="$(cargo test -p "${kw[@]}" -- "${args[@]}" 2>"$errlog")"
  code=$?
  printf '%s\n' "$out" >>"$log"
  printf '%s\n' "$out" | grep -E '^test result:' || true
  [[ $code -eq 0 ]] || {
    tail -40 "$errlog" >&2
    printf '%s\n' "$out" | tail -40 >&2
    err "cargo test -p ${kw[*]} が exit $code"
  }
done

for t in "${tests[@]}"; do
  grep -qE "^test (.*::)?${t} \.\.\. ok$" "$log" || err "引用したテスト \`$t\` が '... ok' と出ていない"
done

if [[ $fail -ne 0 ]]; then
  echo "check-browser-acceptance: 失敗" >&2
  exit 1
fi
echo "check-browser-acceptance: OK（${#keys[@]} 群の cargo test、引用テスト ${#tests[@]} 件がすべて ok）"
