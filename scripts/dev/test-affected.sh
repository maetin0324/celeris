#!/usr/bin/env bash
# 変更 crate とその逆依存 crate だけの試験を走らせる（ADR 2026-10-20 full-tests-at-release-only）。
# 全体の試験（test-parallel.sh・web e2e・本物 browser/userns 試験）は release gate だけ。
#   bash scripts/dev/test-affected.sh          nextest + doc test を対象 crate に走らせる
#   bash scripts/dev/test-affected.sh --list   対象 crate 名を 1 行ずつ出し、web の検査が対象なら
#                                              `web: typecheck+unit (理由: …)`・`control-ts: typecheck+test (理由: …)`
#                                              の行を足す（何も走らせない）
# 基準は ${CELERIS_WU_BASE:-$(git merge-base HEAD main)}。差分は基準との git diff と未追跡 file。
# Cargo.lock・ルート Cargo.toml・.config/nextest.toml・rust-toolchain の変更は全 crate 扱い（stderr に警告）。
# web の型検査（typecheck script = tsc -b）と unit 試験（e2e でない test script）は、web の型に届きうる変更で走る
# （ADR 2026-10-23-test-affected-web-typecheck）: web/・docs/api/（生成型の元の schema）・workspace 共通 file・
# task-api とその通常依存の crate（schema の型と event kind の元）。e2e・browser は release gate だけ。
# control-ts の型検査と試験は control-ts/ と docs/api/script-host/（生成型の元の schema）の変更で走る。
# TS だけの変更（中立の agent-docs/・docs/ を除く変更がすべて control-ts/src/ の業務 TS）は、control-ts の検査を
# scripts/dev/ts-verify.sh に委ね、cargo を起動しない（ADR 2026-11-02-ts-fast-verify (5)(6)）。判定はこの script が
# 変わった path を自分で見て決め、ts-verify の記録の class が ts-only であることも確かめる。
# それ以外の control-ts/・docs/api/script-host/ の変更（Host API・検証の道具・golden）は従来の control-ts の検査に加え
# celeris-script とその逆依存の cargo の試験を走らせる。control-ts/fixtures/ の変更は fixture を書く・読む crate も足す。
# さらに `bash scripts/dev/ts-verify.sh --grade` で候補の作業ツリーを本物の runner で 10 段採点し、非 0 ならこの script も
# 非 0 で終わる（ADR 2026-11-02-ts-fast-verify 付記 grade-gate。--list では `control-ts: ts-verify-grade (理由: …)` の行）。
# runner の cache key の入力（crates/script-runtime/・crates/task-core/・Cargo.lock・ルート Cargo.toml・
# host-v1.schema.json）か検証の道具（scripts/dev/ts-verify*・script-runner-cache.sh・test-ts-verify.sh・この script）が
# 変われば scripts/dev/test-ts-verify.sh も走らせる（runner を作り直して TS の段を通す）。
# node_modules が無い・lockfile が変わったときは先に `pnpm install --offline --frozen-lockfile`。
# 対象 crate に celeris か celeris-script が入るときは、cargo の前に control-ts の source から使い捨ての dir（$TMPDIR）へ
# bundle を作り（build-bundle.mjs）、CELERIS_SCRIPT_BUNDLE_DIR と CELERIS_SCRIPT_REQUIRE_BUNDLE=1 を渡す。task の根の
# control-ts/dist は読まず、書き換えもしない（境界 ADR 2026-11-02-ts-control-layer-boundary 付記 S5-9）。作れないときは
# 理由を 1 行出して渡さない。ただし control-ts/・docs/api/script-host/・crates/script-runtime/・script の試験と support・
# daemon の script 層の変更では作れないことを失敗にする。bundle を読む試験の SKIP は CELERIS_SCRIPT_SKIP_LOG の file で
# 数えて `script bundle SKIP: N 件` を出し、bundle を作れていて N > 0 なら非 0 で終わる。
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
trap 'rm -f "$changed" "$changed.status"' EXIT
# rename は両側の path（--no-renames で削除と追加に分ける）。1 行 = "<status>\t<path>"。
{
  git diff --name-status --no-renames "$base" --
  git ls-files --others --exclude-standard | sed 's/^/A\t/'
} | sort -u -k2 >"$changed.status"
cut -f2- "$changed.status" | sort -u >"$changed"

# TS だけの変更か（ADR 2026-11-02-ts-fast-verify (1) の ts-only の path。ts-verify.sh の分類とは別にここで見る）。
ts_only=1
ts_seen=0
while IFS="$(printf '\t')" read -r st f; do
  case "$f" in
    agent-docs/*) ;;
    docs/api/*) ts_only=0 ;;
    docs/*) ;;
    control-ts/src/testing/*|control-ts/src/host.d.ts) ts_only=0 ;;
    control-ts/src/*.test.ts) if [ "$st" = D ]; then ts_only=0; else ts_seen=1; fi ;;
    control-ts/src/*.ts) ts_seen=1 ;;
    *) ts_only=0 ;;
  esac
done <"$changed.status"
[ "$ts_seen" -eq 1 ] || ts_only=0

if [ "$ts_only" -eq 1 ]; then
  ts_reason="TS だけの変更（control-ts/src の業務 TS）。cargo は走らせない"
  if [ "$list_only" -eq 1 ]; then
    printf 'control-ts: ts-verify (理由: %s)\n' "$ts_reason"
    exit 0
  fi
  echo "test-affected: control-ts の検査を scripts/dev/ts-verify.sh に委ねる（理由: $ts_reason）" >&2
  rec="$changed.ts-verify.json"
  trap 'rm -f "$changed" "$changed.status" "$rec"' EXIT
  bash scripts/dev/ts-verify.sh --record "$rec"
  # ts-verify が ts-only と判断して段を走らせたこと（「従来の経路へ」で何もせず抜けていないこと）を確かめる。
  python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
if r.get("class") != "ts-only" or r.get("exit") != 0 or len(r.get("stages", [])) < 2:
    sys.exit("test-affected: ts-verify の記録が ts-only の検証の通過を示さない: class=%s exit=%s" % (r.get("class"), r.get("exit")))
' "$rec"
  exit 0
fi

all=0
web_dir=0
docs_api=0
control_ts_reason=""
# 変わった path から足す crate（Host API・検証の道具・golden の変更。名前は workspace に在るものだけ使う）。
extra_crates=""
fixtures=0
tsv_reason=""
# bundle を読む試験に届く変更（bundle を作れないことを失敗にする）。
bundle_need=""
while IFS= read -r f; do
  case "$f" in
    Cargo.lock|Cargo.toml|.config/nextest.toml|rust-toolchain|rust-toolchain.toml|tests/support/*) all=1 ;;
    web/*) web_dir=1 ;;
    docs/api/*) docs_api=1 ;;
  esac
  case "$f" in
    control-ts/*) control_ts_reason="control-ts/ の変更"; extra_crates="celeris-script" ;;
    docs/api/script-host/*) [ -n "$control_ts_reason" ] || control_ts_reason="docs/api/script-host/ の変更（生成型の元の schema）"
                            extra_crates="celeris-script" ;;
  esac
  case "$f" in control-ts/fixtures/*) fixtures=1 ;; esac
  case "$f" in
    control-ts/*|docs/api/script-host/*|crates/script-runtime/*|crates/celeris/tests/script_*|crates/celeris/tests/support/*|crates/celeris/src/daemon/script_*|scripts/dev/test-affected.sh)
      bundle_need="$f" ;;
  esac
  case "$f" in
    scripts/dev/ts-verify*|scripts/dev/script-runner-cache.sh|scripts/dev/test-ts-verify.sh|scripts/dev/test-affected.sh)
      tsv_reason="検証の道具の変更"; extra_crates="celeris-script" ;;
    crates/script-runtime/*|crates/task-core/*|Cargo.lock|Cargo.toml|docs/api/script-host/host-v1.schema.json|control-ts/fixtures/cos_digest/pass_traces.jsonl)
      [ -n "$tsv_reason" ] || tsv_reason="runner の cache key の入力の変更" ;;
  esac
done <"$changed"
if [ "$fixtures" -eq 1 ]; then
  # fixture を書く・読む crate（crates/*/{src,examples,tests} で path control-ts/fixtures/ を参照する crate）の dir 名。
  fx="$(git grep -l -F 'control-ts/fixtures/' -- ':(glob)crates/*/src/**' ':(glob)crates/*/examples/**' ':(glob)crates/*/tests/**' |
    cut -d/ -f2 | sort -u || true)"
  for d in $fx; do
    n="$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' "crates/$d/Cargo.toml" | head -1)"
    extra_crates="$extra_crates ${n:-$d}"
  done
fi

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
direct |= {n for n in sys.argv[4].split() if n in names}
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
' "$all" "$changed" "$web_api_roots" "$extra_crates")"
crates="$(printf '%s\n' "$picked" | grep -v '^@api ' || true)"
api_hit="$(printf '%s\n' "$picked" | sed -n 's/^@api //p')"

# ts-only でない control-ts/・docs/api/script-host/ の変更（ts-only はこの前で ts-verify.sh に委ねて抜けている）。
# control-ts の試験・compare.mjs は候補が書き換えうるので、候補の tree を Rust の runner と cache の golden で採点する。
grade_reason=""
[ -z "$control_ts_reason" ] ||
  grade_reason="TS だけでない $(printf '%s' "$control_ts_reason" | sed 's/ の変更.*$//') の変更。候補の作業ツリーを本物の runner で採点する"

web_reason=""
add_reason() { web_reason="${web_reason:+$web_reason; }$1"; }
[ "$web_dir" -eq 0 ] || add_reason "web/ の変更"
[ "$docs_api" -eq 0 ] || add_reason "docs/api/ の変更（生成型の元の schema）"
[ -z "$api_hit" ] || add_reason "web の型の元になる crate の変更: $api_hit"
[ "$all" -eq 0 ] || add_reason "workspace 共通 file の変更"

if [ "$list_only" -eq 1 ]; then
  [ -z "$crates" ] || printf '%s\n' "$crates"
  [ -z "$web_reason" ] || printf 'web: typecheck+unit (理由: %s)\n' "$web_reason"
  [ -z "$control_ts_reason" ] || printf 'control-ts: typecheck+test (理由: %s)\n' "$control_ts_reason"
  [ -z "$grade_reason" ] || printf 'control-ts: ts-verify-grade (理由: %s)\n' "$grade_reason"
  [ -z "$tsv_reason" ] || printf 'ts-verify: test-ts-verify (理由: %s)\n' "$tsv_reason"
  exit 0
fi

# JS package の 1 段（install・typecheck・unit）を自分の session で走らせ、上限を超えたら止まっている process の木を
# stderr に出して失敗させる（check の timeout より先に、理由に証拠が残るように）。
# 試験は pnpm を通さず web/node_modules/.bin を PATH に足して直に走らせる（pnpm run と同じ解決）。
# daemon の決定的 check で corepack＋pnpm 経由の実行が 600 秒＋1200 秒止まり、同じ HEAD の手元では
# 10 秒で通った（原因未特定。task 01M5JAYKJ5W80FD5954CXHDNEX）。
js_step() { # package label cmd
  local pkg="$1" label="$2" cmd="$3" pid dog rc
  echo "test-affected: $pkg $label ($cmd、上限 ${web_timeout} 秒)" >&2
  # bash の background は group leader でないので setsid は fork せず pid＝session id。
  setsid sh -c "$cmd" </dev/null &
  pid=$!
  (
    sleep "$web_timeout" </dev/null >/dev/null 2>&1
    echo "test-affected: $pkg $label が ${web_timeout} 秒で終わらない。止まっている process:" >&2
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
  [ "$rc" -eq 0 ] || echo "test-affected: $pkg $label が失敗した (exit $rc)" >&2
  return "$rc"
}

# package（cwd）の pnpm。PATH に無ければ package.json の packageManager の版を corepack で使う。
pnpm_cmd() {
  if command -v pnpm >/dev/null 2>&1; then
    echo pnpm
  else
    echo "corepack pnpm@$(python3 -I -c 'import json;print(json.load(open("package.json"))["packageManager"].split("@",1)[1])')"
  fi
}

run_js_package() { # dir（web か control-ts）
  local pkg="$1" typecheck test pm
  typecheck="$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("scripts",{}).get("typecheck",""))' "$pkg/package.json")"
  test="$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1])).get("scripts",{}).get("test",""))' "$pkg/package.json")"
  case "$test" in
    *e2e*|*playwright*) echo "test-affected: $pkg の test script が e2e のため unit 試験は走らせない: $test" >&2; test="" ;;
  esac
  web_timeout="${TEST_AFFECTED_WEB_TIMEOUT:-300}"
  (
    cd "$pkg"
    # worktree（WU を含む）には node_modules が無い。依存は pnpm の store（host 共有）から offline で入れる。
    # store に無い版（依存を足した直後）だけ network を許して入れ直す。lockfile 固定は release gate と同じ。
    if [ ! -f node_modules/.modules.yaml ] || grep -qx "$pkg/\(pnpm-lock.yaml\|package.json\)" "$changed"; then
      pm="$(pnpm_cmd)"
      js_step "$pkg" install "$pm install --offline --frozen-lockfile" ||
        js_step "$pkg" install "$pm install --prefer-offline --frozen-lockfile"
    fi
    export PATH="$PWD/node_modules/.bin:$PATH"
    if [ -n "$typecheck" ]; then
      js_step "$pkg" typecheck "$typecheck"
    else
      echo "test-affected: $pkg/package.json に typecheck script がない" >&2
      exit 1
    fi
    [ -z "$test" ] || js_step "$pkg" "unit 試験" "$test"
  )
}

# control-ts の source から out（使い捨ての dir）に bundle を作る（ts-verify.sh の bundle 段と同じ build-bundle.mjs）。
# 作れなければ理由を stdout に 1 行出して非 0。node_modules が無ければ offline で入れる。
build_script_bundle() { # out
  local out="$1" log
  log="$(mktemp "${TMPDIR:-/tmp}/test-affected-bundle.XXXXXX")"
  if ! command -v node >/dev/null 2>&1; then
    echo "node が無い"
    rm -f "$log"
    return 1
  fi
  if [ ! -f control-ts/node_modules/.modules.yaml ]; then
    if ! (cd control-ts && $(pnpm_cmd) install --offline --frozen-lockfile) >"$log" 2>&1; then
      echo "control-ts の pnpm install --offline が失敗した: $(tail -1 "$log")"
      rm -f "$log"
      return 1
    fi
  fi
  if ! TEST_AFFECTED_BUILD="$root/control-ts/scripts/build-bundle.mjs" TEST_AFFECTED_OUT="$out" \
    node --input-type=module -e '
      const { buildBundle } = await import(process.env.TEST_AFFECTED_BUILD);
      await buildBundle({ outDir: process.env.TEST_AFFECTED_OUT });
    ' >"$log" 2>&1; then
    echo "build-bundle.mjs が失敗した: $(tail -1 "$log")"
    rm -f "$log"
    return 1
  fi
  rm -f "$log"
}

ran=0
if [ -n "$web_reason" ]; then
  echo "test-affected: web typecheck+unit を走らせる（理由: $web_reason）" >&2
  run_js_package web
  ran=1
fi

if [ -n "$control_ts_reason" ]; then
  echo "test-affected: control-ts typecheck+test を走らせる（理由: $control_ts_reason）" >&2
  run_js_package control-ts
  ran=1
fi

if [ -n "$grade_reason" ]; then
  echo "test-affected: bash scripts/dev/ts-verify.sh --grade を走らせる（理由: $grade_reason）" >&2
  grec="$changed.grade.json"
  trap 'rm -f "$changed" "$changed.status" "$grec"' EXIT
  if ! bash scripts/dev/ts-verify.sh --grade --record "$grec"; then
    echo "test-affected: ts-verify.sh --grade が失敗した" >&2
    exit 1
  fi
  # 採点した（ts-only・host-api）なら 10 段すべてが exit 0 であること。rust は ts-verify が従来の経路へ回す。
  python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
want = "classify runner-cache install typecheck bundle unit golden safety integration activate".split()
if r.get("exit") != 0:
    sys.exit("test-affected: ts-verify --grade の記録の exit が 0 でない: %s" % r.get("exit"))
if r.get("class") in ("ts-only", "host-api") and [s.get("name") for s in r.get("stages", []) if s.get("exit") == 0] != want:
    sys.exit("test-affected: ts-verify --grade の記録が 10 段の通過を示さない: class=%s" % r.get("class"))
' "$grec"
  rm -f "$grec"
  ran=1
fi

if [ -n "$tsv_reason" ]; then
  # runner を作り直し（cargo を使う）、$TMPDIR の写しで ts-only の経路と意図的な破壊の検出を確かめる。
  echo "test-affected: bash scripts/dev/test-ts-verify.sh を走らせる（理由: $tsv_reason）" >&2
  bash scripts/dev/test-ts-verify.sh
  ran=1
fi

if [ -n "$crates" ]; then
  args=()
  while IFS= read -r c; do args+=(-p "$c"); done <<<"$crates"
  echo "test-affected: crates: $(echo "$crates" | tr '\n' ' ')" >&2
  # test-parallel.sh と同じく、試験の一時 dir を /tmp 直下の短い path に閉じ込める。Unix socket の sun_path は短く、
  # 深い TMPDIR の下では credentiald・browser の IPC 試験が socket を bind できず起動待ちで時間切れになる。
  tmproot="$(mktemp -d "/tmp/celeris-test-affected.XXXXXX")"
  trap 'rm -f "$changed" "$changed.status"; chmod -R u+w "$tmproot" 2>/dev/null || true; rm -rf "$tmproot" 2>/dev/null || true' EXIT
  mkdir -p "$tmproot/tmp"
  export TMPDIR="$tmproot/tmp"
  bundle_built=0
  skip_log=""
  if printf '%s\n' "$crates" | grep -qx 'celeris\|celeris-script'; then
    skip_log="$tmproot/script-skip.log"
    : >"$skip_log"
    export CELERIS_SCRIPT_SKIP_LOG="$skip_log"
    unset CELERIS_SCRIPT_BUNDLE_DIR
    bundle_out="$tmproot/script-bundle"
    if why="$(build_script_bundle "$bundle_out")"; then
      bundle_built=1
      export CELERIS_SCRIPT_BUNDLE_DIR="$bundle_out" CELERIS_SCRIPT_REQUIRE_BUNDLE=1
      echo "test-affected: control-ts の source から bundle を作った（$bundle_out）。bundle を読む試験はこれを使い、SKIP を失敗にする" >&2
    elif [ -n "$bundle_need" ]; then
      echo "test-affected: bundle を作れない（$why）。bundle を読む試験に届く変更（$bundle_need）があるので失敗にする" >&2
      exit 1
    else
      echo "test-affected: bundle を作れない（$why）。CELERIS_SCRIPT_BUNDLE_DIR を渡さない" >&2
    fi
  fi
  cargo nextest run "${args[@]}"
  cargo test --doc "${args[@]}"
  if [ -n "$skip_log" ]; then
    skips="$(grep -c . "$skip_log" || true)"
    echo "test-affected: script bundle SKIP: ${skips} 件" >&2
    if [ "$skips" -gt 0 ]; then
      sed 's/^/test-affected:   SKIP /' "$skip_log" >&2
      if [ "$bundle_built" -eq 1 ]; then
        echo "test-affected: 作った bundle で SKIP した試験がある" >&2
        exit 1
      fi
    fi
  fi
  ran=1
fi

[ "$ran" -eq 1 ] || echo "test-affected: 試験対象の変更なし（docs のみ等）" >&2
exit 0
