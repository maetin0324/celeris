#!/usr/bin/env bash
# 変更 crate とその逆依存 crate だけの試験を走らせる（ADR 2026-10-20 full-tests-at-release-only）。
# 全体の試験（test-parallel.sh・web e2e・本物 browser/userns 試験）は release gate だけ。
#   bash scripts/dev/test-affected.sh          nextest + doc test を対象 crate に走らせる
#   bash scripts/dev/test-affected.sh --list   対象 crate 名を 1 行ずつ出し、web の検査が対象なら
#                                              `web: typecheck+unit (理由: …)` の行を足す（何も走らせない）
# 基準は ${CELERIS_WU_BASE:-$(git merge-base HEAD main)}。差分は基準との git diff と未追跡 file。
# Cargo.lock・ルート Cargo.toml・.config/nextest.toml・rust-toolchain の変更は全 crate 扱い（stderr に警告）。
# web の型検査（typecheck script = tsc -b）と unit 試験（e2e でない test script）は、web の型に届きうる変更で走る
# （ADR 2026-10-23-test-affected-web-typecheck）: web/・docs/api/（生成型の元の schema）・workspace 共通 file・
# task-api とその通常依存の crate（schema の型と event kind の元）。e2e・browser は release gate だけ。
# node_modules が無い・lockfile が変わったときは先に `pnpm install --offline --frozen-lockfile`。
# 何も対象が無ければ（docs だけ等）何もせず exit 0。
set -euo pipefail

list_only=0
case "${1:-}" in
  --list) list_only=1 ;;
  "") ;;
  *) echo "usage: test-affected.sh [--list]" >&2; exit 2 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "$root"
# Required for every check, including docs-only changes; --list remains read-only.
if [ "$list_only" -eq 0 ]; then
  bash scripts/dev/check-health.sh
fi
base="${CELERIS_WU_BASE:-$(git merge-base HEAD main)}"

changed="$(mktemp "${TMPDIR:-/tmp}/test-affected.XXXXXX")"
trap 'rm -f "$changed"' EXIT
{
  git diff --name-only "$base" --
  git ls-files --others --exclude-standard
} | sort -u >"$changed"

all=0
web_dir=0
docs_api=0
while IFS= read -r f; do
  case "$f" in
    Cargo.lock|Cargo.toml|.config/nextest.toml|rust-toolchain|rust-toolchain.toml|tests/support/*) all=1 ;;
    web/*) web_dir=1 ;;
    docs/api/*) docs_api=1 ;;
  esac
done <"$changed"

if [ "$all" -eq 1 ]; then
  echo "test-affected: workspace 共通 file が変わったため全 crate を対象にする" >&2
fi

# web の生成型（web/api/generated）の元の schema を書く crate。これとその通常依存（dev 依存は除く）の変更は
# web の型（Event["type"] の網羅・API の欄）を変えうる。
web_api_roots="task-api"

# 変更 file → crate（manifest dir の最長一致）→ 逆依存閉包。workspace 内の依存（dev 含む）だけを辿る。
# 出力: 対象 crate を 1 行ずつ。web の型の元になる crate が直接変わったら最後に `@api <crate…>` の 1 行。
picked="$(cargo metadata --format-version 1 --no-deps | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
all_crates = sys.argv[1] == "1"
api_roots = sys.argv[3].split()
root = meta["workspace_root"].rstrip("/") + "/"
pk = [p for p in meta["packages"] if p["id"] in set(meta["workspace_members"])]
names = {p["name"] for p in pk}
dirs = {}
for p in pk:
    d = p["manifest_path"].rsplit("/", 1)[0] + "/"
    dirs[p["name"]] = d[len(root):] if d.startswith(root) else d
rdeps = {n: set() for n in names}
normal = {n: set() for n in names}
for p in pk:
    for d in p["dependencies"]:
        if d["name"] in names:
            rdeps[d["name"]].add(p["name"])
            if d.get("kind") in (None, "build"):
                normal[p["name"]].add(d["name"])
direct = set()
for f in open(sys.argv[2]).read().split("\n"):
    if not f:
        continue
    best = None
    for n, d in dirs.items():
        if (f + "/").startswith(d) or f.startswith(d):
            if best is None or len(d) > len(dirs[best]):
                best = n
    if best:
        direct.add(best)
hit = set(names) if all_crates else set(direct)
todo = list(hit)
while todo:
    n = todo.pop()
    for r in rdeps[n]:
        if r not in hit:
            hit.add(r)
            todo.append(r)
api_src = {r for r in api_roots if r in names}
todo = list(api_src)
while todo:
    n = todo.pop()
    for d in normal[n]:
        if d not in api_src:
            api_src.add(d)
            todo.append(d)
out = sorted(hit)
api_hit = sorted(direct & api_src)
if api_hit:
    out.append("@api " + " ".join(api_hit))
print("\n".join(out))
' "$all" "$changed" "$web_api_roots")"
crates="$(printf '%s\n' "$picked" | grep -v '^@api ' || true)"
api_hit="$(printf '%s\n' "$picked" | sed -n 's/^@api //p')"

web_reason=""
add_reason() { web_reason="${web_reason:+$web_reason; }$1"; }
[ "$web_dir" -eq 0 ] || add_reason "web/ の変更"
[ "$docs_api" -eq 0 ] || add_reason "docs/api/ の変更（生成型の元の schema）"
[ -z "$api_hit" ] || add_reason "web の型の元になる crate の変更: $api_hit"
[ "$all" -eq 0 ] || add_reason "workspace 共通 file の変更"

if [ "$list_only" -eq 1 ]; then
  [ -z "$crates" ] || printf '%s\n' "$crates"
  [ -z "$web_reason" ] || printf 'web: typecheck+unit (理由: %s)\n' "$web_reason"
  exit 0
fi

# web の 1 段（install・typecheck・unit）を自分の session で走らせ、上限を超えたら止まっている process の木を
# stderr に出して失敗させる（check の timeout より先に、理由に証拠が残るように）。
# 試験は pnpm を通さず web/node_modules/.bin を PATH に足して直に走らせる（pnpm run と同じ解決）。
# daemon の決定的 check で corepack＋pnpm 経由の実行が 600 秒＋1200 秒止まり、同じ HEAD の手元では
# 10 秒で通った（原因未特定。task 01M5JAYKJ5W80FD5954CXHDNEX）。
web_step() { # label cmd
  local label="$1" cmd="$2" pid dog rc
  echo "test-affected: web $label ($cmd、上限 ${web_timeout} 秒)" >&2
  # bash の background は group leader でないので setsid は fork せず pid＝session id。
  setsid sh -c "$cmd" </dev/null &
  pid=$!
  (
    sleep "$web_timeout" </dev/null >/dev/null 2>&1
    echo "test-affected: web $label が ${web_timeout} 秒で終わらない。止まっている process:" >&2
    ps -o pid,ppid,etime,stat,wchan:24,args --forest -s "$pid" | cut -c1-200 >&2 || true
    kill -TERM -- "-$pid" 2>/dev/null || true
  ) &
  dog=$!
  set +e
  wait "$pid"
  rc=$?
  set -e
  pkill -P "$dog" 2>/dev/null || true
  kill "$dog" 2>/dev/null || true
  wait "$dog" 2>/dev/null || true
  [ "$rc" -eq 0 ] || echo "test-affected: web $label が失敗した (exit $rc)" >&2
  return "$rc"
}

run_web() {
  local typecheck test pm
  typecheck="$(python3 -I -c 'import json;print(json.load(open("web/package.json")).get("scripts",{}).get("typecheck",""))')"
  test="$(python3 -I -c 'import json;print(json.load(open("web/package.json")).get("scripts",{}).get("test",""))')"
  case "$test" in
    *e2e*|*playwright*) echo "test-affected: web の test script が e2e のため unit 試験は走らせない: $test" >&2; test="" ;;
  esac
  web_timeout="${TEST_AFFECTED_WEB_TIMEOUT:-300}"
  (
    cd web
    # worktree（WU を含む）には node_modules が無い。依存は pnpm の store（host 共有）から offline で入れる。
    # store に無い版（依存を足した直後）だけ network を許して入れ直す。lockfile 固定は release gate と同じ。
    if [ ! -f node_modules/.modules.yaml ] || grep -qx 'web/\(pnpm-lock.yaml\|package.json\)' "$changed"; then
      if command -v pnpm >/dev/null 2>&1; then
        pm=pnpm
      else
        pm="corepack pnpm@$(python3 -I -c 'import json;print(json.load(open("package.json"))["packageManager"].split("@",1)[1])')"
      fi
      web_step install "$pm install --offline --frozen-lockfile" ||
        web_step install "$pm install --prefer-offline --frozen-lockfile"
    fi
    export PATH="$PWD/node_modules/.bin:$PATH"
    if [ -n "$typecheck" ]; then
      web_step typecheck "$typecheck"
    else
      echo "test-affected: web/package.json に typecheck script がない" >&2
      exit 1
    fi
    [ -z "$test" ] || web_step "unit 試験" "$test"
  )
}

ran=0
if [ -n "$web_reason" ]; then
  echo "test-affected: web typecheck+unit を走らせる（理由: $web_reason）" >&2
  run_web
  ran=1
fi

if [ -n "$crates" ]; then
  args=()
  while IFS= read -r c; do args+=(-p "$c"); done <<<"$crates"
  echo "test-affected: crates: $(echo "$crates" | tr '\n' ' ')" >&2
  # test-parallel.sh と同じく、試験の一時 dir を /tmp 直下の短い path に閉じ込める。Unix socket の sun_path は短く、
  # 深い TMPDIR の下では credentiald・browser の IPC 試験が socket を bind できず起動待ちで時間切れになる。
  tmproot="$(mktemp -d "/tmp/celeris-test-affected.XXXXXX")"
  trap 'rm -f "$changed"; chmod -R u+w "$tmproot" 2>/dev/null || true; rm -rf "$tmproot" 2>/dev/null || true' EXIT
  mkdir -p "$tmproot/tmp"
  export TMPDIR="$tmproot/tmp"
  cargo nextest run "${args[@]}"
  # doc test は lib target のある crate だけ（bin だけの crate を -p に渡すと「no library targets」で落ちる）。
  libs="$(cargo metadata --no-deps --format-version 1 | python3 -I -c 'import json,sys
for p in json.load(sys.stdin)["packages"]:
    if any("lib" in t["kind"] or "proc-macro" in t["kind"] for t in p["targets"]): print(p["name"])')"
  doc_args=()
  while IFS= read -r c; do
    grep -qx "$c" <<<"$libs" && doc_args+=(-p "$c")
  done <<<"$crates"
  [ "${#doc_args[@]}" -eq 0 ] || cargo test --doc "${doc_args[@]}"
  ran=1
fi

[ "$ran" -eq 1 ] || echo "test-affected: 試験対象の変更なし（docs のみ等）" >&2
exit 0
