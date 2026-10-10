#!/usr/bin/env bash
# scripts/selfdeploy/lib.sh — release / verify / promote / rollback / status / migrate で共有する道具
# （ADR-0040 D1/D2、ADR-0045 D2）。単体では何もしない。`source` して使う。
#
# 置き場（ADR-0045 D2。XDG 流に設定と状態を分ける）:
#   $CELERIS_CONFIG_DIR（既定 ~/.config/celeris） 設定と秘密: config.toml org.toml providers.d/ api.token
#                                                 gui.password gui.session-secret secrets/
#   $CELERIS_STATE_DIR （既定 ~/.local/celeris）  hot state: releases/ staging/ tools/ current previous
#   $CELERIS_BACKUPS_DIR（既定 $CELERIS_STATE_DIR/backups）新規 backup
#   $CELERIS_LOGS_DIR（既定 $SD_BACKUPS）運用 log
#
# 触ってよい場所（ADR-0040 D1、安全規則）:
#   $SD_RELEASES / $SD_STAGING / $SD_BACKUPS と、昇格のときだけ $SD_CURRENT / $SD_PREVIOUS の symlink。
# 触ってはいけない場所: $SD_CONFIG（編集しない。`migrate-to-celeris.sh` だけが移行のときに 1 度書く）、
#   $SD_DB（`sqlite3 .backup` と `mode=ro` で読むだけ）、本番のポート 127.0.0.1:7710 と 0.0.0.0:7700（bind しない）。

# shellcheck shell=bash

# ---- 場所 ------------------------------------------------------------------

CELERIS_CONFIG_DIR="${CELERIS_CONFIG_DIR:-$HOME/.config/celeris}"
# ADR-0136: unit と同じ根を使う（unit は `EnvironmentFile=-%h/.config/celeris/paths.env`）。人が素の
# shell で promote.sh を起こしたときに読まないと、home 側の current だけを書き換え、daemon が見る
# $CELERIS_STATE_DIR/current は古いまま残る（2026-10-03、再起動で promotion gate が拒否して停止した）。
if [ -z "${CELERIS_STATE_DIR:-}" ] && [ -f "$CELERIS_CONFIG_DIR/paths.env" ]; then
  set -a
  # shellcheck source=/dev/null
  . "$CELERIS_CONFIG_DIR/paths.env"
  set +a
fi
CELERIS_STATE_DIR="${CELERIS_STATE_DIR:-$HOME/.local/celeris}"
# `SD_REPO` を要るのは `release.sh`（作業ツリーを生やす）だけ。`verify.sh` / `promote.sh` /
# `rollback.sh` / `status.sh` は上の 2 つの下だけを見るので、作業チェックアウトが無くても動く。
# ADR-0040 D6（Phase 48）: `release.sh` がこの一式を `<release>/scripts/` に写すので、
# `promote.sh` はリリースの中から（`POST /releases/{sha12}/promote` 経由で）起きることがある。
# そのときも `lib.sh` は `dirname "${BASH_SOURCE[0]}"` で自分の隣を読むだけなので、場所に依らない。
# Phase 83 / G36（ADR-0041 追記）: `verify.sh` の検査 4b（gui-e2e）だけ、`$SD_REPO/gui`（devDependencies
# 込みで `pnpm install` 済みの方。release の `gui/` は `pnpm install --prod` で Playwright が無い）を
# **あれば使う**。無くても検査 4b は「未インストール」で false になるだけでクラッシュしない
# （＝この段落の「作業チェックアウトが無くても動く」は変わらない）。
SD_REPO="${SD_REPO:-$HOME/workspace/agent-platform}"

SD_RELEASES="$CELERIS_STATE_DIR/releases"
SD_BUILD_ROOT="$SD_RELEASES/.build"
# Phase SD-1（ADR-0075 追記「Phase SD-1 実装時の逸脱」）: release.sh の作業ツリーは**場所を固定して使い回す**
# （`git checkout --force` + `git clean -ffdx` で毎回その sha のきれいな checkout にする）。git は内容の変わった
# ファイルしか書き直さないので、共有の target と組み合わせると cargo は変わった crate だけを作り直す。
# `.build/<sha12>/` は gate が落ちたときの gate.json とログの置き場（ただのディレクトリ）として残る。
SD_BUILD_TREE="$SD_BUILD_ROOT/tree"
SD_CARGO_TARGET="$SD_RELEASES/.cargo-target"
# Phase SD-1: リリースの gui の本番依存（`pnpm install --prod --frozen-lockfile` の node_modules）の置き場。
# `<key>` = gui の package.json / pnpm-lock.yaml / pnpm-workspace.yaml と node / pnpm の版の sha256。
# リリースの `gui/node_modules` はここへの相対 symlink（NFS 上で 4,000 余のファイルを毎回書かないため）。
SD_PNPM_PROD_CACHE="$SD_RELEASES/.pnpm-prod-cache"
SD_STAGING="$CELERIS_STATE_DIR/staging"
SD_TOOLS="$CELERIS_STATE_DIR/tools"
SD_BACKUPS="${CELERIS_BACKUPS_DIR:-$CELERIS_STATE_DIR/backups}"
# Legacy deployments kept operational logs alongside backups. The explicit
# logs path separates them while preserving that old default.
SD_LOGS="${CELERIS_LOGS_DIR:-$SD_BACKUPS}"
SD_CURRENT="$CELERIS_STATE_DIR/current"
SD_PREVIOUS="$CELERIS_STATE_DIR/previous"
# 設定ファイル。`CELERIS_CONFIG` が立っていればそれが勝つ（移行のあいだ `verify.sh` に
# **旧い名前の**設定ファイルを読ませるため。ADR-0045 D3 の段取り 1）。
SD_CONFIG="${CELERIS_CONFIG:-$CELERIS_CONFIG_DIR/config.toml}"
SD_API_TOKEN_FILE="$CELERIS_CONFIG_DIR/api.token"

# DB は**設定ファイルの `db =` から読む**（ADR-0045 D2）。`CELERIS_DB` が立っていればそれが勝つ
# （移行のあいだ `verify.sh` に旧 DB を見せるため）。設定が無い／`db` を書いていないときは
# celeris 本体と同じ既定（$CELERIS_STATE_DIR/celeris.sqlite3）。
sd_db_from_config() {
  local v
  [ -f "$SD_CONFIG" ] || { printf '%s' "$CELERIS_STATE_DIR/celeris.sqlite3"; return 0; }
  # 最初の節（`[...]`）より前の、トップレベルの `db = "..."` だけを見る。
  v="$(sed -n '/^[[:space:]]*\[/q; s/^[[:space:]]*db[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p' "$SD_CONFIG" | head -n 1)"
  # ADR-0064 D1（Phase 110a）: `[db]` テーブル形式（`path = "..."`）も見る。`[db]` 見出しから次の見出しまで。
  if [ -z "$v" ]; then
    v="$(sed -n '/^[[:space:]]*\[db\][[:space:]]*$/,/^[[:space:]]*\[/{ s/^[[:space:]]*path[[:space:]]*=[[:space:]]*"\([^"]*\)".*$/\1/p; }' "$SD_CONFIG" | head -n 1)"
  fi
  if [ -z "$v" ]; then printf '%s' "$CELERIS_STATE_DIR/celeris.sqlite3"; return 0; fi
  case "$v" in
    "~/"*) printf '%s/%s' "$HOME" "${v#"~/"}" ;;
    /*) printf '%s' "$v" ;;
    *) printf '%s/%s' "$(dirname "$SD_CONFIG")" "$v" ;;   # 相対は設定ファイルのディレクトリ基準
  esac
}
SD_DB="${CELERIS_DB:-$(sd_db_from_config)}"

# 本番のポート。ここに bind してはいけない（読むだけ）。
SD_PROD_API="http://127.0.0.1:7710"
SD_PROD_GUI_PORT=7700
# staging のポート（D3）。
# Phase SD-1: テスト（`scripts/selfdeploy/tests/`）が空いているポートで verify.sh を回せるよう env で変えられる。
# 本番のポート（7710 / 7700）にはしないこと（verify.sh が起動前に拒む）。
SD_STAGING_API_PORT="${SD_STAGING_API_PORT:-7711}"
SD_STAGING_GUI_PORT="${SD_STAGING_GUI_PORT:-7701}"
SD_STAGING_OLD_API_PORT="${SD_STAGING_OLD_API_PORT:-7712}"

# corepack の pnpm（このホストでは PATH に無い）。
SD_PNPM_SHIM_DIR="${SD_PNPM_SHIM_DIR:-/usr/lib/node_modules/corepack/shims}"

# ---- ログ ------------------------------------------------------------------

SD_LOG_FILE="${SD_LOG_FILE:-}"

sd_ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }
sd_stamp() { date +%Y%m%d-%H%M%S; }
# 壁時計の秒（小数）と、そこからの経過秒（小数 3 桁）。gate.json / verify.json の `secs`（Phase SD-1）。
sd_now() { date +%s.%N; }
sd_secs_since() { awk -v a="$1" -v b="$(date +%s.%N)" 'BEGIN { printf "%.3f", b - a }'; }

sd_log() {
  local line
  line="$(sd_ts) [${SD_PROG:-selfdeploy}] $*"
  printf '%s\n' "$line" >&2
  if [ -n "$SD_LOG_FILE" ]; then printf '%s\n' "$line" >>"$SD_LOG_FILE"; fi
}

sd_die() {
  sd_log "ERROR: $*"
  exit 1
}

# ---- JSON（python3 を優先。無ければ jq） -----------------------------------

if command -v python3 >/dev/null 2>&1; then
  SD_JSON_TOOL=python3
elif command -v jq >/dev/null 2>&1; then
  SD_JSON_TOOL=jq
else
  SD_JSON_TOOL=none
fi

sd_require_json_tool() {
  [ "$SD_JSON_TOOL" = none ] && sd_die "need python3 or jq for JSON handling"
  return 0
}

# 文字列を JSON の文字列リテラルにする（前後の " 込み）。
sd_json_str() {
  case "$SD_JSON_TOOL" in
    python3) python3 -c 'import json,sys; sys.stdout.write(json.dumps(sys.argv[1], ensure_ascii=False))' "$1" ;;
    jq) jq -Rn --arg s "$1" '$s' ;;
    *) sd_die "need python3 or jq" ;;
  esac
}

# ファイルの JSON から dotted path の値を取り出す（`a.b.0.c`）。
# 見つかれば標準出力に（スカラーは素の値、配列/オブジェクトは JSON）、見つからなければ exit 1。
sd_json_get() {
  local file="$1" path="$2"
  [ -f "$file" ] || return 1
  case "$SD_JSON_TOOL" in
    python3)
      python3 - "$file" "$path" <<'PY'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        cur = json.load(fh)
except Exception:
    sys.exit(1)
for part in sys.argv[2].split("."):
    if part == "":
        continue
    if isinstance(cur, list):
        try:
            cur = cur[int(part)]
        except Exception:
            sys.exit(1)
    elif isinstance(cur, dict) and part in cur:
        cur = cur[part]
    else:
        sys.exit(1)
if cur is None:
    sys.stdout.write("null")
elif isinstance(cur, bool):
    sys.stdout.write("true" if cur else "false")
elif isinstance(cur, (int, float, str)):
    sys.stdout.write(str(cur))
else:
    sys.stdout.write(json.dumps(cur, ensure_ascii=False))
PY
      ;;
    jq)
      local filter=".$path"
      [ "$path" = "" ] && filter="."
      jq -e -r "$filter" "$file" 2>/dev/null
      ;;
    *) sd_die "need python3 or jq" ;;
  esac
}

# JSON ファイルが妥当かどうか（本文は捨てる）。
sd_json_valid() {
  case "$SD_JSON_TOOL" in
    python3) python3 -c 'import json,sys; json.load(open(sys.argv[1],encoding="utf-8"))' "$1" >/dev/null 2>&1 ;;
    jq) jq -e . "$1" >/dev/null 2>&1 ;;
    *) return 1 ;;
  esac
}

# ---- daemon_instances の読み方（ADR-0040 D4 付記 2026-10-05 / 付記 2026-10-05b）------------------
#
# 「生きている active」= 役割が `active` で、`drained_at` が無く、プロセス（pid）が生きている行
# （celeris 側の `is_live_active` と同じ定義。heartbeat では決めない）。draining・終了済み（drained_at）・
# プロセスが消えた行は active に数えない。pid が分からない行（0 / 欠落）は生きている側に倒す。

# `sd_pid_alive <pid>` — 同一ホストの /proc で見る。0 = 生きている、1 = 死んでいる、2 = 判定できない
# （pid が 0・空・数字でない、/proc が無い）。
sd_pid_alive() {
  local pid="$1"
  case "$pid" in
    '' | 0 | *[!0-9]*) return 2 ;;
  esac
  [ -d /proc ] || return 2
  if [ -e "/proc/$pid" ]; then return 0; else return 1; fi
}

# `sd_instances_rows <file>` — `{"instances":[…]}` の JSON から `release<TAB>role<TAB>pid<TAB>drained` の
# 行を出す（drained は drained_at があれば 1、無ければ 0。pid は無ければ 0）。JSON が読めない・
# `instances` が配列でないときは exit 1（何も出さない）。
sd_instances_rows() {
  local file="$1"
  [ -f "$file" ] || return 1
  case "$SD_JSON_TOOL" in
    python3)
      python3 - "$file" <<'PYROWS'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        rows = json.load(fh).get("instances")
except Exception:
    sys.exit(1)
if not isinstance(rows, list):
    sys.exit(1)
for r in rows:
    if not isinstance(r, dict):
        sys.exit(1)
    pid = r.get("pid")
    pid = pid if isinstance(pid, int) and pid > 0 else 0
    drained = 1 if r.get("drained_at") not in (None, "") else 0
    print("%s\t%s\t%d\t%d" % (r.get("release") or "", r.get("role") or "", pid, drained))
PYROWS
      ;;
    jq)
      jq -e -r '.instances | if type == "array" then .[] | "\(.release // "")\t\(.role // "")\t\(if (.pid|type) == "number" and .pid > 0 then .pid else 0 end)\t\(if (.drained_at // "") == "" then 0 else 1 end)" else error("no instances") end' "$file" 2>/dev/null
      ;;
    *) return 1 ;;
  esac
}

# `sd_instances_verdict <file> <sha12>` — 一語を標準出力に出す。`settled` のときだけ exit 0。
#   settled        生きている active が `sha12` の 1 つだけ（旧は draining・終了済み・消えた）
#   two_active     `sha12` が生きた active で、他にも生きた active がいる（二重 active）
#   new_not_active 生きた active はいるが `sha12` ではない（新は standby のまま、または行が無い）
#   no_active      生きた active が 1 つも無い
#   unreadable     本文が読めない（HTTP の失敗・JSON でない・`instances` が無い）
sd_instances_verdict() {
  local file="$1" sha12="$2" rows release role pid drained live_other=0 live_new=0 tab
  tab="$(printf '\t')"
  rows="$(sd_instances_rows "$file")" || { printf 'unreadable'; return 1; }
  while IFS="$tab" read -r release role pid drained; do
    [ -n "$release$role" ] || continue
    [ "$role" = active ] || continue
    [ "$drained" = 0 ] || continue
    if sd_pid_alive "$pid"; then :; elif [ $? -eq 1 ]; then continue; fi
    if [ "$release" = "$sha12" ]; then live_new=$((live_new + 1)); else live_other=$((live_other + 1)); fi
  done <<<"$rows"
  if [ "$live_new" -eq 1 ] && [ "$live_other" -eq 0 ]; then printf 'settled'; return 0; fi
  if [ "$live_new" -ge 1 ]; then printf 'two_active'; return 1; fi
  if [ "$live_other" -ge 1 ]; then printf 'new_not_active'; return 1; fi
  printf 'no_active'
  return 1
}

# `sd_instances_summary <file>` — ログ用の 1 行（`release:role pid=<pid> <alive|dead|pid?>[ drained]` を空白区切り）。
sd_instances_summary() {
  local file="$1" rows release role pid drained out="" alive tab
  tab="$(printf '\t')"
  rows="$(sd_instances_rows "$file")" || { printf 'unreadable'; return 0; }
  while IFS="$tab" read -r release role pid drained; do
    [ -n "$release$role" ] || continue
    if sd_pid_alive "$pid"; then alive=alive; elif [ $? -eq 1 ]; then alive=dead; else alive='pid?'; fi
    out="$out ${release:-?}:${role:-?} pid=$pid $alive"
    [ "$drained" = 0 ] || out="$out drained"
  done <<<"$rows"
  [ -n "$out" ] || out=" (no rows)"
  printf '%s' "${out# }"
}

# `sd_instances_settled <file> <sha12>` — `sd_instances_verdict` が `settled` なら 0（旧 API）。
sd_instances_settled() {
  [ "$(sd_instances_verdict "$1" "$2")" = settled ]
}

# `sd_instances_from_db <db> <outfile>` — API が読めないときの代替: `daemon_instances` を**読み取り専用**で
# 開き、`GET /api/v1/releases` と同じ形 `{"instances":[…]}` を `outfile` に書く（status.sh と同じ読み方）。
# 表が無い・sqlite3 が無い・DB が読めないときは exit 1。本番の DB には書かない。
sd_instances_from_db() {
  local db="$1" out="$2" rows
  command -v sqlite3 >/dev/null 2>&1 || return 1
  [ -r "$db" ] || return 1
  rows="$(sqlite3 "file:$db?mode=ro" -json \
    'SELECT instance_id, "release", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at
       FROM daemon_instances ORDER BY started_at' 2>/dev/null)" || return 1
  printf '{"instances": %s}' "${rows:-[]}" >"$out"
  sd_json_valid "$out"
}

# TSV（1 行目が `name:type name:type ...` のヘッダ。type は s/i/f/b）→ JSON の配列。
sd_tsv_to_json() {
  local file="$1"
  case "$SD_JSON_TOOL" in
    python3)
      python3 - "$file" <<'PY'
import json, sys
rows = []
with open(sys.argv[1], encoding="utf-8") as fh:
    lines = fh.read().split("\n")
spec = [f.split(":") for f in lines[0].split() if f]
for line in lines[1:]:
    if not line.strip():
        continue
    cells = line.split("\t")
    row = {}
    for i, (name, kind) in enumerate(spec):
        raw = cells[i] if i < len(cells) else ""
        if kind == "i":
            row[name] = int(raw or 0)
        elif kind == "f":
            row[name] = round(float(raw or 0), 3)
        elif kind == "b":
            row[name] = raw == "true"
        else:
            row[name] = raw
    rows.append(row)
sys.stdout.write(json.dumps(rows, ensure_ascii=False, indent=2))
PY
      ;;
    jq)
      local hdr spec
      hdr="$(head -n 1 "$file")"
      spec="$(printf '%s' "$hdr" | tr ' ' '\n' | jq -Rn '[inputs | select(length>0) | split(":") | {n: .[0], t: .[1]}]')"
      tail -n +2 "$file" | jq -Rn --argjson spec "$spec" '
        [ inputs | select(length > 0) | split("\t") as $r
          | reduce range(0; ($spec | length)) as $i ({};
              ($spec[$i].n) as $n | ($spec[$i].t) as $t | ($r[$i] // "") as $v
              | .[$n] = (if $t == "i" then ($v | tonumber | floor)
                         elif $t == "f" then ($v | tonumber)
                         elif $t == "b" then ($v == "true")
                         else $v end)) ]'
      ;;
    *) sd_die "need python3 or jq" ;;
  esac
}

# 1 行 1 要素のファイル → JSON の文字列配列（空行は落とす）。1 回の呼び出しで済ませる
# （`sd_json_str` をパスの数だけ呼ぶと python3 の起動が数百回になる）。
sd_lines_to_json_array() {
  local file="$1"
  [ -s "$file" ] || { printf '[]'; return 0; }
  case "$SD_JSON_TOOL" in
    python3)
      python3 - "$file" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8", errors="replace") as fh:
    items = [line for line in fh.read().split("\n") if line.strip()]
sys.stdout.write(json.dumps(items, ensure_ascii=False))
PY
      ;;
    jq) jq -R -s 'split("\n") | map(select(length > 0))' "$file" ;;
    *) sd_die "need python3 or jq" ;;
  esac
}

# ---- git -------------------------------------------------------------------

sd_sha12() {
  local ref="$1"
  git -C "$SD_REPO" rev-parse --short=12 "${ref}^{commit}" 2>/dev/null \
    || sd_die "cannot resolve git ref: $ref (repo $SD_REPO)"
}

sd_sha_full() {
  git -C "$SD_REPO" rev-parse "${1}^{commit}" 2>/dev/null || sd_die "cannot resolve git ref: $1"
}

# ---- HTTP ------------------------------------------------------------------

# `sd_http_get <url> [token-file]` — 本文を標準出力に。HTTP status を返す代わりに、
# 非 2xx なら exit 1（本文は捨てない）。
sd_http_get() {
  local url="$1" token_file="${2:-}" code body tmp
  tmp="$(mktemp)"
  if [ -n "$token_file" ] && [ -r "$token_file" ]; then
    code="$(curl -sS -o "$tmp" -w '%{http_code}' -m 30 -H "Authorization: Bearer $(tr -d '\r\n' <"$token_file")" "$url" || echo 000)"
  else
    code="$(curl -sS -o "$tmp" -w '%{http_code}' -m 30 "$url" || echo 000)"
  fi
  body="$(cat "$tmp")"
  rm -f "$tmp"
  printf '%s' "$body"
  case "$code" in
    2*) return 0 ;;
    *) return 1 ;;
  esac
}

# `sd_http_post <url> <token-file> <json-body>` — 本文を標準出力に。非 2xx なら exit 1
# （本文は捨てない。`problem+json` の説明をそのまま呼び出し側のログに出せる）。
# 本番には使わない（`verify.sh` の staging だけ。lib.sh の先頭の安全規則）。
sd_http_post() {
  local url="$1" token_file="${2:-}" body="${3:-}" code out tmp
  tmp="$(mktemp)"
  if [ -n "$token_file" ] && [ -r "$token_file" ]; then
    code="$(curl -sS -o "$tmp" -w '%{http_code}' -m 30 -X POST \
      -H "Authorization: Bearer $(tr -d '\r\n' <"$token_file")" \
      -H 'Content-Type: application/json' --data-binary "$body" "$url" || echo 000)"
  else
    code="$(curl -sS -o "$tmp" -w '%{http_code}' -m 30 -X POST \
      -H 'Content-Type: application/json' --data-binary "$body" "$url" || echo 000)"
  fi
  out="$(cat "$tmp")"
  rm -f "$tmp"
  printf '%s' "$out"
  case "$code" in
    2*) return 0 ;;
    *) return 1 ;;
  esac
}

# `sd_http_status <url> [token-file]` — status コードだけ。
sd_http_status() {
  local url="$1" token_file="${2:-}"
  if [ -n "$token_file" ] && [ -r "$token_file" ]; then
    curl -sS -o /dev/null -w '%{http_code}' -m 30 -H "Authorization: Bearer $(tr -d '\r\n' <"$token_file")" "$url" 2>/dev/null || echo 000
  else
    curl -sS -o /dev/null -w '%{http_code}' -m 30 "$url" 2>/dev/null || echo 000
  fi
}

# `sd_http_status_follow <url>` — リダイレクトを追って**最後の**status を返す。GUI の画面用
# （`/` は `/tasks` などへ 302 する。ADR-0040 D3 の「200」はリダイレクトの先のこと）。
sd_http_status_follow() {
  curl -sSL --max-redirs 5 -o /dev/null -w '%{http_code}' -m 30 "$1" 2>/dev/null || echo 000
}

# `sd_wait_http_200 <url> <timeout-secs> [token-file]`
sd_wait_http_200() {
  local url="$1" timeout="$2" token_file="${3:-}" waited=0 code
  while [ "$waited" -lt "$timeout" ]; do
    code="$(sd_http_status "$url" "$token_file")"
    [ "$code" = 200 ] && return 0
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

# celeris-web@.service の Environment= と EnvironmentFile= に対応する値を読む。
# web.env は KEY=VALUE のみを受け付け、シェルとして実行しない。
sd_web_load_env() {
  local file="${CELERIS_CONFIG_DIR}/web.env" line key value
  export NODE_ENV=production
  export CELERIS_WEB_BIND=127.0.0.1:7720
  export CELERIS_API_URL=http://127.0.0.1:7710
  export CELERIS_API_TOKEN_FILE="${CELERIS_CONFIG_DIR}/api.token"
  export CELERIS_WEB_SESSION_SECRET_FILE="${CELERIS_CONFIG_DIR}/web.session-secret"
  export CELERIS_WEB_PASSWORD_FILE="${CELERIS_CONFIG_DIR}/web.password"
  [ -f "$file" ] || return 0
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line#"${line%%[![:space:]]*}"}"
    case "$line" in ''|\#*|\;*) continue ;; esac
    key="${line%%=*}"
    [ "$key" != "$line" ] || continue
    key="${key%"${key##*[![:space:]]}"}"
    [[ "$key" =~ ^[A-Za-z_][A-Za-z_0-9]*$ ]] || continue
    value="${line#*=}"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    if [[ "$value" == \"*\" || "$value" == \'*\' ]]; then value="${value:1:${#value}-2}"; fi
    printf -v "$key" '%s' "$value"
    export "$key"
  done <"$file"
}

# /healthz は status と release の両方を見る。短い間隔で出来事を待ち、期限を越えたら失敗。
sd_web_health_wait() {
  local url="$1" sha="$2" timeout="${3:-30}" deadline code body
  deadline=$((SECONDS + timeout))
  while :; do
    body="$(curl --noproxy '*' -sS --connect-timeout 1 --max-time 2 -o /dev/stdout -w '\n%{http_code}' "$url/healthz" 2>/dev/null)" || body=''
    code="${body##*$'\n'}"
    body="${body%$'\n'*}"
    if [ "$code" = 200 ] && [ "$(printf '%s' "$body" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("release", ""))' 2>/dev/null)" = "$sha" ]; then
      return 0
    fi
    [ "$SECONDS" -lt "$deadline" ] || return 1
    sleep 0.2
  done
}

# `sd_web_app_probe <app_dir> <port> <sha12>` — 一時起動して応答を確認する。
# subshell の EXIT trap が成功・失敗・割込みのいずれでも起動した node を止める。
sd_web_app_probe() (
  local app_dir="$1" port="$2" sha="$3" pid='' log
  [[ "$port" =~ ^[0-9]+$ ]] && [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || return 1
  [ -f "$app_dir/server/index.js" ] && [ -d "$app_dir/node_modules" ] || return 1
  # 使用中の port の既存応答を誤って成功と判定しない。
  python3 - "$port" <<'PY' || return 1
import socket, sys
with socket.socket() as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(('127.0.0.1', int(sys.argv[1])))
PY
  log="$(mktemp)" || return 1
  trap 'if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || :; wait "$pid" 2>/dev/null || :; fi; rm -f "$log"' EXIT
  trap 'exit 1' INT TERM HUP
  sd_web_load_env
  # probe は本番の owner socket・信頼端末に触れない（ADR 2026-10-07-browser-trusted-devices D7）。
  unset CELERIS_WEB_OWNER_SOCKET
  export CELERIS_WEB_PROBE=1
  export CELERIS_WEB_BIND="127.0.0.1:$port" CELERIS_WEB_RELEASE="$sha"
  cd "$app_dir" || return 1
  "${SD_WEB_NODE:-node}" server/index.js >"$log" 2>&1 &
  pid=$!
  if sd_web_health_wait "http://127.0.0.1:$port" "$sha" "${SD_WEB_PROBE_TIMEOUT:-30}"; then return 0; fi
  sd_log "warning: web app probe failed for $sha: $(tail -n 5 "$log" | tr '\n' ' ')"
  return 1
)

# ---- ポート ----------------------------------------------------------------

# 誰かが LISTEN していれば 1（塞がっている）。
sd_port_free() {
  local port="$1"
  if ss -ltn "sport = :$port" 2>/dev/null | tail -n +2 | grep -q .; then
    return 1
  fi
  return 0
}

sd_require_port_free() {
  sd_port_free "$1" || sd_die "port $1 is already in use (need it for $2)"
}

# ---- リリース --------------------------------------------------------------

sd_release_dir() { printf '%s/%s' "$SD_RELEASES" "$1"; }

# `current` / `previous` が指している sha12（無ければ空文字）。
sd_link_target() {
  local link="$1" dest
  [ -L "$link" ] || return 0
  dest="$(readlink -f "$link" 2>/dev/null)" || return 0
  [ -n "$dest" ] || return 0
  basename "$dest"
}

sd_current_sha() { sd_link_target "$SD_CURRENT"; }
sd_previous_sha() { sd_link_target "$SD_PREVIOUS"; }

# `sd_set_link <link> <sha12>` — symlink を張り替える（同一 dir 内で mv するので原子的）。
sd_set_link() {
  local link="$1" sha="$2" tmp
  tmp="$link.tmp.$$"
  ln -sfn "releases/$sha" "$tmp"
  mv -T "$tmp" "$link"
}

# ---- 昇格中の印（ADR-0040 付記 2026-10-02「handoff と migration の認可」）--------
#
# `celeris@<sha12>` を start する**直前**に `releases/<sha12>/promoting.json` を置き、`current` を
# 付け替えた直後（と EXIT トラップ）で消す。daemon は `current` か 900 秒以内のこの印が自分の
# release と一致するときだけ DB を開いて migrate し、handoff を要求する（それ以外は exit 4）。
# 印を書くのも消すのもこの 2 つだけ（daemon は読むだけ）。
sd_promoting_path() { printf '%s/promoting.json' "$(sd_release_dir "$1")"; }

# `sd_write_promoting <sha12> <script> <mode>` — 一時ファイル → mv で原子的に置く。
sd_write_promoting() {
  local sha="$1" script="$2" mode="$3" path tmp
  path="$(sd_promoting_path "$sha")"
  tmp="$path.tmp.$$"
  {
    printf '{\n'
    printf '  "sha12": %s,\n' "$(sd_json_str "$sha")"
    printf '  "script": %s,\n' "$(sd_json_str "$script")"
    printf '  "mode": %s,\n' "$(sd_json_str "$mode")"
    printf '  "pid": %s,\n' "$$"
    printf '  "started_at": %s\n' "$(sd_json_str "$(sd_ts)")"
    printf '}\n'
  } >"$tmp"
  mv -f "$tmp" "$path"
  sd_log "promoting marker: $path (script=$script mode=$mode)"
}

# `sd_clear_promoting <sha12>` — 無くてもよい（EXIT トラップから何度呼ばれても害が無い）。
sd_clear_promoting() {
  local path
  path="$(sd_promoting_path "$1")"
  if [ -e "$path" ]; then
    rm -f "$path"
    sd_log "promoting marker removed: $path"
  fi
}

# `pub const SCHEMA_VERSION: u32 = N;` を読む。ADR-0079 の分割で store.rs は
# crates/task-core/src/store/migrations.rs に分かれた。store/mod.rs、旧 store.rs の順に
# 探す（分割前の木や別の再配置でも読めるように）。
sd_schema_version_of_tree() {
  # `local a="$1" b="$a"` は全部の語を先に展開してから代入するので `$a` はまだ無い（set -u で落ちる）。
  # 参照する変数は別の `local` に分ける。
  local tree="$1"
  local candidate file n
  for candidate in \
    "$tree/crates/task-core/src/store/migrations.rs" \
    "$tree/crates/task-core/src/store/mod.rs" \
    "$tree/crates/task-core/src/store.rs"
  do
    [ -f "$candidate" ] || continue
    file="$candidate"
    n="$(sed -n 's/^[[:space:]]*pub const SCHEMA_VERSION: u32 = \([0-9][0-9]*\);.*$/\1/p' "$file" | head -n 1)"
    [ -n "$n" ] || continue
    printf '%s' "$n"
    return 0
  done
  printf '0'
  return 1
}

# 本番 DB のスキーマ版数（read-only。`schema_migrations` の最大 `version`）。
sd_db_schema_version() {
  local db="${1:-$SD_DB}" v
  v="$(sqlite3 "file:${db}?mode=ro" 'SELECT COALESCE(MAX(version), 0) FROM schema_migrations;' 2>/dev/null)" || return 1
  printf '%s' "$v"
}

sd_mkdirs() {
  mkdir -p "$SD_RELEASES" "$SD_BUILD_ROOT" "$SD_BACKUPS"
}

# pnpm（corepack の shim）を PATH に入れる。
sd_use_pnpm() {
  case ":$PATH:" in
    *":$SD_PNPM_SHIM_DIR:"*) ;;
    *) PATH="$SD_PNPM_SHIM_DIR:$PATH"; export PATH ;;
  esac
  command -v pnpm >/dev/null 2>&1 || sd_die "pnpm not found (looked in $SD_PNPM_SHIM_DIR)"
}

# ---- scratch pool の lease（ADR-0075 D7、Phase G1。Phase SD-1 で owner を共有に） ----

# release ゲートの `CARGO_TARGET_DIR` は scratch pool の lease から取る。
# `celerisctl` が無い・scratch が無効（`[scratch] enabled = false`・NFS 上）・lease に失敗したときは
# 従来どおり `$SD_RELEASES/.cargo-target`（上の `SD_CARGO_TARGET` の既定）のまま。
#
# Phase SD-1（ADR-0075 追記「Phase SD-1 実装時の逸脱」）: owner は sha ごとの `release-<sha12>` ではなく
# **全リリースで 1 つ**の `release-build`（`SD_RELEASE_SCRATCH_OWNER`）。場所を固定した作業ツリー
# （`SD_BUILD_TREE`）と組み合わせて、cargo の fingerprint で変わった crate だけを作り直す。
# 2 本の release.sh が同じ target を同時に触らないのは `$SD_RELEASES/.lock-release`（release.sh 全体を包む
# flock）による。終了時は `release` せず `touch` する: `released_at` を書くと P3 になり、daemon の
# `scratch_gc` が次の tick で即回収する（release-* の P3 は seed でなければ即回収。ADR-0075 D2）ため。
# 代わりに lease の TTL を `SD_RELEASE_TARGET_TTL`（既定 172800 = 48h）にし、最後のリリースから TTL の間は
# P0、過ぎれば P3 で GC がそのまま回収する（GC の規則は変えていない）。
#
# `celerisctl` の場所: `SD_CELERISCTL`（テストの偽物）> PATH の `celerisctl` > `$SD_CURRENT/bin/celerisctl`。
SD_SCRATCH_OWNER=""
SD_RELEASE_SCRATCH_OWNER="${SD_RELEASE_SCRATCH_OWNER:-release-build}"
SD_RELEASE_TARGET_TTL="${SD_RELEASE_TARGET_TTL:-172800}"

sd_celerisctl_bin() {
  if [ -n "${SD_CELERISCTL:-}" ]; then
    [ -x "$SD_CELERISCTL" ] && { printf '%s' "$SD_CELERISCTL"; return 0; }
    return 1
  fi
  if command -v celerisctl >/dev/null 2>&1; then
    command -v celerisctl
    return 0
  fi
  [ -x "$SD_CURRENT/bin/celerisctl" ] && { printf '%s' "$SD_CURRENT/bin/celerisctl"; return 0; }
  return 1
}

# `sd_scratch_lease <owner> <sha_full> <build_worktree>` — 成功すれば `SD_CARGO_TARGET` と `SD_SCRATCH_OWNER` を書き換える。
# 引き継ぎ（adopt）の安全条件の checkout 時刻は `<build_worktree>/.git` の mtime。固定の作業ツリーでも、
# その中のファイルは全て `.git` を書いた後に書かれている（`git worktree add` → 以後の checkout）ので条件は保たれる。
sd_scratch_lease() {
  local owner="$1" sha="$2" tree="$3" ctl out
  ctl="$(sd_celerisctl_bin)" || { sd_log "scratch: celerisctl not found; using $SD_CARGO_TARGET"; return 1; }
  if ! out="$("$ctl" scratch lease --config "$SD_CONFIG" --owner "$owner" \
    --repo "$SD_REPO" --worktree "$tree" --base "$sha" --ttl "$SD_RELEASE_TARGET_TTL" 8>&- 9>&-)"; then
    sd_log "scratch: lease failed (disabled or unavailable); using $SD_CARGO_TARGET"
    return 1
  fi
  [ -n "$out" ] || return 1
  SD_CARGO_TARGET="$out"
  SD_SCRATCH_OWNER="$owner"
  return 0
}

# browser-ledger shares release.sh's release-build lease. If scratch is unavailable,
# only use the local fallback when it is a real directory: a symlink may point at
# a reclaimed lease. A dangling fallback link is safe to unlink, but never follow.
sd_release_build_target() {
  local sha="$1" tree="$2" link_tmp
  if sd_scratch_lease "$SD_RELEASE_SCRATCH_OWNER" "$sha" "$tree"; then
    if [ -L "$SD_RELEASES/.cargo-target" ]; then
      link_tmp="$SD_RELEASES/.cargo-target.tmp.$$"
      if ln -s "$SD_CARGO_TARGET" "$link_tmp" && mv -Tf -- "$link_tmp" "$SD_RELEASES/.cargo-target"; then
        sd_log "updated cargo target link to current lease: $SD_CARGO_TARGET"
      else
        rm -f -- "$link_tmp"
        sd_log "could not update cargo target link; using leased target directly"
      fi
    fi
    return 0
  fi
  if [ -L "$SD_CARGO_TARGET" ]; then
    if [ ! -e "$SD_CARGO_TARGET" ]; then
      rm -f -- "$SD_CARGO_TARGET" || return 1
      sd_log "removed dangling cargo target link: $SD_CARGO_TARGET"
    else
      sd_log "scratch unavailable and cargo target is a symlink; refusing stale target: $SD_CARGO_TARGET"
      return 1
    fi
  fi
  return 0
}

# 長い step の前（と release.sh の終了時）に lease の mtime を今にする（TTL 切れで GC に回収されないように）。
# 失敗しても続ける。
sd_scratch_touch() {
  [ -n "$SD_SCRATCH_OWNER" ] || return 0
  local ctl
  ctl="$(sd_celerisctl_bin)" || return 0
  "$ctl" scratch touch --config "$SD_CONFIG" --owner "$SD_SCRATCH_OWNER" >/dev/null 2>&1 8>&- 9>&- || true
}

# Release-build target pruning. Dependency libraries and build-script outputs are deliberately untouched.
sd_release_prune_record_start() {
  local marker="$1/.celeris-release-build-start" tmp now
  now="${SD_RELEASE_NOW:-$(date +%s)}"
  tmp="$marker.tmp.$$"
  printf '%s\n' "$now" >"$tmp"
  mv -f "$tmp" "$marker"
}

sd_release_prune_size() {
  python3 - "$1" <<'PY'
import os, subprocess, sys
target = sys.argv[1]
try:
    result = subprocess.run(
        ['btrfs', 'filesystem', 'du', '-s', '--raw', target],
        check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    )
    # Raw output columns are Total, Exclusive, Set shared. The latter two
    # represent the target's unique footprint without charging shared extents twice.
    for line in reversed(result.stdout.splitlines()):
        fields = line.split(None, 3)
        if len(fields) == 4 and fields[3] == target:
            print(int(fields[1]) + int(fields[2]))
            break
    else:
        raise ValueError('unrecognized btrfs du output')
except (OSError, subprocess.SubprocessError, ValueError):
    total = 0
    seen = set()
    for root, dirs, files in os.walk(target):
        for name in files:
            try:
                st = os.stat(os.path.join(root, name), follow_symlinks=False)
                inode = (st.st_dev, st.st_ino)
                if inode in seen: continue
                seen.add(inode)
                total += st.st_blocks * 512
            except OSError: pass
    print(total)
PY
}

# Candidates are every workspace target name (cargo metadata: all packages, all target kinds,
# '-' -> '_') plus package names, so integration tests (tests/<file>.rs -> <file>-<hash>) and
# members outside crates/ (tests/e2e) are included. A candidate whose .d references a registry
# or git checkout source is a dependency crate and is kept. SD_RELEASE_PRUNE_DRY_RUN=1 only reports.
sd_release_prune_stale_test_binaries() {
  local target="$1" tree="$2" start="$3" report verb=removed
  [ -d "$target" ] || return 0
  report="$(python3 - "$target" "$tree" "$start" <<'PY'
import glob, json, os, re, subprocess, sys
target, tree, start = sys.argv[1], os.path.realpath(sys.argv[2]), float(sys.argv[3])
dry = os.environ.get('SD_RELEASE_PRUNE_DRY_RUN') == '1'
names, source = set(), 'metadata'
try:
    out = subprocess.run(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1', '--offline',
         '--manifest-path', os.path.join(tree, 'Cargo.toml')],
        check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL, timeout=120,
    ).stdout
    for package in json.loads(out)['packages']:
        names.add(package['name'].replace('-', '_'))
        for t in package.get('targets', []):
            names.add(t['name'].replace('-', '_'))
except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError):
    # Fallback: package names and conventional target files of every member manifest.
    source = 'fallback'
    for root, dirs, files in os.walk(tree):
        dirs[:] = [d for d in dirs if d not in ('target', '.git', 'node_modules')]
        if 'Cargo.toml' not in files: continue
        try: text = open(os.path.join(root, 'Cargo.toml'), encoding='utf-8').read()
        except OSError: continue
        m = re.search(r'^\[package\][^\[]*?^name\s*=\s*["\']([^"\']+)', text, re.M | re.S)
        if not m: continue
        names.add(m.group(1).replace('-', '_'))
        for pattern in ('tests/*.rs', 'src/bin/*.rs', 'benches/*.rs', 'examples/*.rs'):
            for path in glob.glob(os.path.join(root, pattern)):
                names.add(os.path.basename(path)[:-3].replace('-', '_'))
        for pattern in ('tests/*/main.rs', 'src/bin/*/main.rs', 'benches/*/main.rs', 'examples/*/main.rs'):
            for path in glob.glob(os.path.join(root, pattern)):
                names.add(os.path.basename(os.path.dirname(path)).replace('-', '_'))
count = size = kept_dependency = 0
deps = os.path.join(target, 'debug', 'deps')
if os.path.isdir(deps):
    for name in sorted(os.listdir(deps)):
        if not name.endswith('.d'): continue
        dep = os.path.join(deps, name)
        stem = name[:-2]
        m = re.match(r'^(.+)-([0-9a-f]{8,})$', stem)
        if not m or m.group(1) not in names: continue
        files = [p for p in (os.path.join(deps, stem), os.path.join(deps, 'lib' + stem + '.rlib'),
                             os.path.join(deps, 'lib' + stem + '.rmeta')) if os.path.isfile(p)]
        try:
            if os.stat(dep).st_mtime >= start or not files: continue
            if any(os.stat(p).st_mtime >= start for p in files): continue
            text = open(dep, encoding='utf-8', errors='replace').read()
        except OSError: continue
        if '/registry/src/' in text or '/git/checkouts/' in text:
            kept_dependency += 1
            continue
        for p in files + [dep]:
            try:
                blocks = os.stat(p, follow_symlinks=False).st_blocks * 512
                if not dry: os.unlink(p)
                count += 1; size += blocks
            except OSError: pass
print(f'{count} {size} {kept_dependency} {source}')
PY
)" || report="0 0 0 error"
  set -- $report
  [ "${SD_RELEASE_PRUNE_DRY_RUN:-0}" = 1 ] && verb="dry run: would remove"
  sd_log "release prune: ${verb} $1 files ($2 bytes allocated) of stale workspace targets and .d files (names from $4; kept $3 dependency-crate matches)"
}

sd_release_prune_enforce_limit() {
  local target="$1" limit size seed tmp
  limit="${SD_RELEASE_TARGET_MAX_BYTES:-68719476736}"
  case "$limit" in *[!0-9]*|'') sd_die "SD_RELEASE_TARGET_MAX_BYTES must be an integer" ;; esac
  size="$(sd_release_prune_size "$target")"
  [ "$size" -le "$limit" ] && return 0
  sd_log "release prune: lease size ${size} bytes exceeds ${limit}; recreating target"
  seed="${SD_RELEASE_TARGET_SEED:-}"
  tmp="${target}.recreate.$$"
  rm -rf "$tmp"
  mkdir -p "$tmp"
  if [ -n "$seed" ] && [ -d "$seed" ]; then cp -a "$seed/." "$tmp/"; fi
  rm -rf "$target"
  mv "$tmp" "$target"
  sd_release_prune_record_start "$target"
}

# lease を返す（P3 に落ち、GC が即回収する）。Phase SD-1 から release.sh の終了時には呼ばない（上の説明）。
# 共有の target を今すぐ手放したいとき（人の手作業）に使う。
sd_scratch_release() {
  [ -n "$SD_SCRATCH_OWNER" ] || return 0
  local ctl
  ctl="$(sd_celerisctl_bin)" || return 0
  "$ctl" scratch release --config "$SD_CONFIG" --owner "$SD_SCRATCH_OWNER" >/dev/null 2>&1 8>&- 9>&- || true
}

# ---- 直列化（flock。ADR-0041 D2） -----------------------------------------

# `verify.sh` は `$SD_STAGING` とポート 7711 / 7701 / 7712 を固定で使うので、2 本同時に走ると
# 必ず壊れる（スナップショットを作り直す側が、もう一方の DB を消す）。`release.sh` は同じ sha の
# worktree競合に加え、異なるshaでも共有Cargo成果物が競合するため全体を直列化する。
# どちらも `flock` の**ファイル記述子**で直列化する
# （プロセスが死ねばカーネルが外すので、残骸のロックファイルは無害）。
#
# EX_TEMPFAIL（sysexits.h の 75）= 「いまは無理。あとでもう一度」。呼び出し側（人・ワーカー・CI）が
# 「壊れた（1）」と「混んでいる（75）」を区別できるようにする。
SD_EX_TEMPFAIL=75

# `verify.sh` がロックを待つ上限（秒）。既定 1800（ADR-0041 D2）。
SD_VERIFY_LOCK_WAIT="${SD_VERIFY_LOCK_WAIT:-1800}"

# `sd_lock_or_tempfail <fd> <lockfile> <wait-secs> <what>`
#   `<wait-secs>` が 0 なら `flock -n`（待たずに諦める）。取れなければ exit 75。
#   取れたロックは**このシェルが終わるまで**（fd が閉じるまで）持ち続ける。
#
# 実機 2026-09-21 09:19: `cargo test` の中で起こされた `podman info`（container の runtime probe）が
# 応答せずに残り、release.sh から**継承した lock の fd** を握ったまま PID 1 の子になった。release.sh 自体は
# 終わっているのに次の release.sh が 1800 秒待って exit 75 になった。対策は 2 つ:
#   1. 子プロセスに lock の fd を継がせない（release.sh の run_step 等で `8>&- 9>&-`）。
#   2. それでも「持ち主が selfdeploy のスクリプトではない」ロックは**漏れた fd**なので、持ち主を記録して
#      ロックファイルを `<file>.leaked-<日時>` に退け、新しい inode で取り直す（`sd_lock_leaked_holders`）。
#      生きている release.sh / verify.sh が 1 つでも fd を持っていれば退けない（本物の直列化はそのまま）。

# `sd_lock_leaked_holders <lockfile>`: そのファイルを開いている全プロセスの `pid cmd` を 1 行ずつ出す。
# 1 つでも selfdeploy のスクリプト（bash …/selfdeploy/*.sh）が含まれていれば何も出さない（＝本物の持ち主がいる）。
sd_lock_leaked_holders() {
  local file="$1" abs pid target cmd holders="" script_alive=false ppid
  abs="$(readlink -f "$file" 2>/dev/null)" || return 0
  for pid in $(ls /proc 2>/dev/null | grep -E '^[0-9]+$'); do
    # 自分自身（スクリプト本体・この関数を動かすサブシェル）と、その直接の子（`flock` など）は持ち主として数えない。
    [ "$pid" = "$$" ] || [ "$pid" = "$BASHPID" ] && continue
    ppid="$(awk '/^PPid:/{print $2}' "/proc/$pid/status" 2>/dev/null)"
    [ "$ppid" = "$$" ] || [ "$ppid" = "$BASHPID" ] && continue
    for target in /proc/"$pid"/fd/*; do
      [ "$(readlink "$target" 2>/dev/null)" = "$abs" ] || continue
      cmd="$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | cut -c1-120)"
      case "$cmd" in *selfdeploy/*.sh*) script_alive=true ;; esac
      holders="${holders}${pid} ${cmd}
"
      break
    done
  done
  [ "$script_alive" = true ] && return 0
  printf '%s' "$holders"
}

sd_lock_or_tempfail() {
  local fd="$1" file="$2" wait="$3" what="$4" leaked
  command -v flock >/dev/null 2>&1 \
    || sd_die "flock not found (util-linux); refusing to run without serialization"
  mkdir -p "$(dirname "$file")"
  : >>"$file"
  eval "exec $fd>>\"\$file\""
  if ! flock -n "$fd"; then
    leaked="$(sd_lock_leaked_holders "$file")"
    if [ -n "$leaked" ]; then
      sd_log "lock $file is held only by processes that are not selfdeploy scripts (the script that took it has exited; the fd leaked to a child):"
      printf '%s' "$leaked" | while IFS= read -r line; do [ -n "$line" ] && sd_log "  leaked holder: $line"; done
      local aside="$file.leaked-$(date -u +%Y%m%dT%H%M%SZ)"
      mv "$file" "$aside" && sd_log "moved the leaked lock aside to $aside; taking a fresh lock"
      : >>"$file"
      eval "exec $fd>>\"\$file\""
    fi
  fi
  if [ "$wait" = 0 ]; then
    if flock -n "$fd"; then return 0; fi
    sd_log "another $what is already running (lock: $file); nothing was changed"
    exit "$SD_EX_TEMPFAIL"
  fi
  if flock -w "$wait" "$fd"; then return 0; fi
  sd_log "another $what is running and did not finish within ${wait}s (lock: $file)"
  sd_log "exit $SD_EX_TEMPFAIL (EX_TEMPFAIL) = busy, not broken; try again later or raise SD_VERIFY_LOCK_WAIT"
  exit "$SD_EX_TEMPFAIL"
}

# ---- 安全に関わる変更（ADR-0041 D4） ---------------------------------------

# 昇格の前に人へ**赤く**見せるパスの一覧。**ここが唯一の定義**（`release.sh` が `changes.json` の
# `sensitive` に書き、`GET /releases` と GUI はその結果を読むだけ。判断を 2 か所に置かない）。
# リポジトリ相対のパスに対する**前方一致**（ディレクトリは末尾 `/`、ファイルはそのまま）。
#
# 選んだ理由: これらを変えたリリースは「昇格の仕組みそのもの」「本番の設定」「エージェントへの
# 指示文」を変える。壊れると次の昇格で直せるとは限らないので、人が必ず中身を見てから押す。
SD_SENSITIVE_PATTERNS=(
  "scripts/selfdeploy/"
  "deploy/"
  "crates/celeris/src/instance.rs"
  "crates/celeris/src/releases.rs"
  "crates/task-api/src/releases.rs"
  "crates/task-core/migrations/"
  "CLAUDE.md"
  "gui/CLAUDE.md"
  ".claude/"
  "config/"
  "agent-docs/adr/0040-"
  "agent-docs/adr/0041-"
)

# `sd_is_sensitive <repo-relative-path>` — 上のどれかに前方一致すれば 0。
sd_is_sensitive() {
  local path="$1" pat
  for pat in "${SD_SENSITIVE_PATTERNS[@]}"; do
    case "$path" in "$pat"*) return 0 ;; esac
  done
  return 1
}

# ---- 旧 celeris の pid（Phase 119 D3） ------------------------------------
#
# 実機 2026-09-24: `promote.sh` の停止→起動が、drain したまま終了しない前回の昇格の残骸
# （`celeris@0e20e1b2a058` 等、複数の draining インスタンス）の中から**最も古い無関係な pid**に
# SIGTERM を送り、300 秒待って「old celeris is still serving after 300s」で失敗した。本来は
# `current` symlink が指す release の unit の MainPID を対象にすべき（ADR-0040 追記）。
#
# 以下 3 つは `promote.sh`/`status.sh` からだけでなく `scripts/selfdeploy/tests/` からも直接
# source して呼べるよう、`/proc` 相当のディレクトリと自分の pid を引数で受け取れるようにしてある
# （既定はどちらも本物。本物の `/proc`・本物の `systemctl` に触れるのは実引数を省略したときだけ）。

# `sd_find_old_celeris_pid [proc_dir] [self_pid]` — 本番の celeris の pid を**設定パス
# （`$SD_CONFIG`）まで含めた完全一致**で `/proc` を走査して探す。`pgrep -f` は自分のシェルの
# コマンドラインにも当たるので使わない（過去に踏んだ）。
#
# **これは systemd 管理化にない場合の最終手段**（初回の移行。`sd_resolve_old_daemon_pid` を使うこと）。
# 複数の draining インスタンスが同じ `--config` を持つ状態では、この関数は「最初に見つかった」
# プロセスを返すだけなので、どれが `current` かは分からない。
sd_find_old_celeris_pid() {
  local proc_dir="${1:-/proc}" self_pid="${2:-$$}"
  local pid_dir pid argv i matched skip
  for pid_dir in "$proc_dir"/[0-9]*; do
    pid="${pid_dir##*/}"
    if [ "$pid" = "$self_pid" ]; then continue; fi
    if [ ! -r "$pid_dir/cmdline" ]; then continue; fi
    argv=()
    mapfile -d '' -t argv <"$pid_dir/cmdline" 2>/dev/null || continue
    if [ "${#argv[@]}" -lt 3 ]; then continue; fi
    # argv[0] が `celeris` そのものでなければ対象外（`grep`、`bash -c`、`sh -c` はここで落ちる）。
    if [ "$(basename -- "${argv[0]}")" != celeris ]; then continue; fi
    # staging（verify.sh が起こす `--mode verify --db … --listen …`）は本番ではない。
    skip=false
    for i in "${argv[@]}"; do
      case "$i" in --mode | --db | --listen | --workspace-root | --token-file) skip=true ;; esac
    done
    if [ "$skip" = true ]; then continue; fi
    matched=false
    for i in $(seq 0 $((${#argv[@]} - 2))); do
      if [ "${argv[$i]}" = "--config" ] && [ "${argv[$((i + 1))]}" = "$SD_CONFIG" ]; then matched=true; fi
    done
    if [ "$matched" != true ]; then continue; fi
    printf '%s' "$pid"
    return 0
  done
  return 1
}

# `sd_resolve_old_daemon_pid <old_sha> [proc_dir] [self_pid]` — Phase 119 D3。
#
# `<old_sha>`（= `current` symlink が指す release）が systemd 管理下で生きているなら、**その unit の
# MainPID だけ**を対象にする（実機で確認した事故の再発防止: 複数の draining インスタンスが同じ
# `--config` を持つときに、`sd_find_old_celeris_pid` の /proc 走査は最初に見つかった無関係な pid
# （最も古い残骸であることが多い）を拾ってしまう）。systemd がまだこの release を知らない
# （初回の移行、または unit がまだ無い）ときだけ `sd_find_old_celeris_pid` にフォールバックする。
sd_resolve_old_daemon_pid() {
  local old_sha="$1" proc_dir="${2:-/proc}" self_pid="${3:-$$}" pid
  if [ -n "$old_sha" ] && systemctl --user is-active --quiet "celeris@$old_sha" 2>/dev/null; then
    pid="$(systemctl --user show -p MainPID --value "celeris@$old_sha" 2>/dev/null || echo 0)"
    if [ -n "$pid" ] && [ "$pid" != 0 ]; then
      printf '%s' "$pid"
      return 0
    fi
  fi
  sd_find_old_celeris_pid "$proc_dir" "$self_pid"
}

# `sd_list_stale_celeris_units <new_sha> <old_sha>` — Phase 119 D3。
#
# `<new_sha>`/`<old_sha>` 以外に**まだ active な** `celeris@*.service` の sha12 を 1 行ずつ返す
# （前回までの昇格で drain したまま終了しなかった残骸。ADR-0040 追記「drain 後にプロセスが終了しない」）。
# systemd が無い環境では何も返さない（クラッシュしない）。
sd_list_stale_celeris_units() {
  local new_sha="$1" old_sha="$2" sha
  command -v systemctl >/dev/null 2>&1 || return 0
  systemctl --user list-units --type=service --state=active --no-legend --plain 'celeris@*.service' \
    2>/dev/null \
    | awk '{print $1}' \
    | sed -n 's/^celeris@\(.*\)\.service$/\1/p' \
    | while IFS= read -r sha; do
        [ -n "$sha" ] || continue
        [ "$sha" = "$new_sha" ] && continue
        [ "$sha" = "$old_sha" ] && continue
        printf '%s\n' "$sha"
      done
}

# ---- browser 適合台帳（ADR 2026-10-08-browser-prod-enablement D1.2 / D1.5）---------------------------------
#
# `sd_browser_ledger <build worktree> <out dir> <sha12>` — release の browser 適合台帳を作って `<out>/browser/` に置く。
# release.sh の段と `browser-ledger.sh`（D1.5 の作り直し）が共有する。**失敗しても release を落とさない**ので、
# 呼ぶ側は戻り値（成功 0 / 台帳なし 1）を無視してよい。結果は `<out>/browser/ledger-status.json`（失敗でも
# `{ok:false, code}`）に書く。ログは標準出力・標準エラーへ（呼ぶ側が gate-logs/browser-ledger.log へ向ける）。
# 差し替え（試験用）: SD_BROWSER_LEDGER_RUNNER（生成器。既定 <build>/scripts/browser-conformance.py を python3 で）、
#   SD_AGENT_BROWSER（既定 agent-browser）、SD_BROWSER_LEDGER_CHECK_BIN（既定 <out>/bin/celerisctl）、
#   SD_BROWSER_AGENT_VERSION（既定 0.38.1）、SD_BROWSER_LEDGER_TIMEOUT（既定 3600 秒。全体の上限）、
#   SD_BROWSER_LAUNCHER_SOCKET（launcher credential 証拠の socket。既定 /run/celeris-browser/launcher.sock。
#   無ければ空にして launcher_unavailable と理由を残す。空文字で明示的に無効化できる）。
# 結果の code: ok / agent_browser_missing / agent_browser_version / generator_failed / check_failed / timeout /
#   または `celerisctl browser ledger check` の code（stale_release・invalid など）。
# 結果の変数: SD_BROWSER_LEDGER_OK（true/false）と SD_BROWSER_LEDGER_CODE（呼んだ shell に残る。subshell で呼ぶと消える）。
SD_BROWSER_AGENT_VERSION="${SD_BROWSER_AGENT_VERSION:-0.38.1}"
SD_BROWSER_LEDGER_OK=false
SD_BROWSER_LEDGER_CODE=""

# `_sd_bl_run <deadline-epoch> <cmd…>` — 残り時間で打ち切って実行する（超過は 124 / 137）。
_sd_bl_run() {
  local deadline="$1" left
  shift
  left=$((deadline - $(date +%s)))
  [ "$left" -ge 1 ] || return 124
  timeout --kill-after=30s "$left" "$@"
}

_sd_bl_write_status() {
  # $1=out/browser $2=ok $3=code $4=agent-browser version $5=p4b $6=check JSON $7=credential ok $8=code $9=reason
  python3 - "$1/ledger-status.json" "$2" "$3" "$4" "$5" "$6" "${7:-false}" "${8:-not_reached}" "${9:-}" <<'PY'
import json, sys, datetime
path, ok, code, ab, p4b, chk, cred_ok, cred_code, cred_reason = sys.argv[1:10]
try:
    c = json.loads(chk) if chk.strip() else {}
except Exception:
    c = {}
doc = {
    "ok": ok == "true",
    "code": code,
    "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "agent_browser": ab or None,
    "p4b": p4b == "true",
    "backends": c.get("backends") or [],
    "credential_backends": c.get("credential_backends") or [],
    "credential_evidence": {"ok": cred_ok == "true", "code": cred_code, "reason": cred_reason},
}
with open(path, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, ensure_ascii=False)
    fh.write("\n")
PY
}

sd_browser_ledger() {
  local build="$1" out="$2" sha12="$3"
  local partial="$out/browser.partial" final="$out/browser"
  local timeout_s="${SD_BROWSER_LEDGER_TIMEOUT:-3600}" deadline
  local ab="${SD_AGENT_BROWSER:-agent-browser}" ab_out ab_ver="" p4b=false
  local check_bin="${SD_BROWSER_LEDGER_CHECK_BIN:-$out/bin/celerisctl}"
  local sha_full chk_json="" chk_rc rc cred_ok=false cred_code=not_reached cred_reason="" cred_json=""
  local -a runner
  deadline=$(($(date +%s) + timeout_s))
  SD_BROWSER_LEDGER_OK=false
  SD_BROWSER_LEDGER_CODE=""
  rm -rf "$partial"
  mkdir -p "$partial"

  _sd_bl_fail() { # <code> — 台帳を置かず ledger-status.json だけを残す
    SD_BROWSER_LEDGER_CODE="$1"
    sd_log "browser-ledger: no ledger placed (code=$1)"
    rm -rf "$partial" "$final"
    mkdir -p "$final"
    _sd_bl_write_status "$final" false "$1" "$ab_ver" "$p4b" "" false not_reached ""
    return 1
  }

  # 1. agent-browser の版
  if ! ab_out="$(_sd_bl_run "$deadline" "$ab" --version 2>&1)"; then
    _sd_bl_fail agent_browser_missing
    return 1
  fi
  ab_ver="$(printf '%s' "$ab_out" | tail -n 1 | awk '{print $NF}')"
  if [ "$ab_ver" != "$SD_BROWSER_AGENT_VERSION" ]; then
    sd_log "browser-ledger: agent-browser is '$ab_ver', want $SD_BROWSER_AGENT_VERSION"
    _sd_bl_fail agent_browser_version
    return 1
  fi

  # 2. 公開能力の台帳（protocol-scripted）
  if [ -n "${SD_BROWSER_LEDGER_RUNNER:-}" ]; then
    runner=("$SD_BROWSER_LEDGER_RUNNER")
  else
    runner=(python3 "$build/scripts/browser-conformance.py")
  fi
  sha_full="$(git -C "$build" rev-parse HEAD 2>/dev/null || true)"
  set -- --protocol-scripted --fallback-scenario --agent-browser "$ab" --celeris-release "$sha12"
  if [ -n "$sha_full" ] && [ "${#sha_full}" -eq 40 ]; then set -- "$@" --celeris-sha "$sha_full"; fi
  sd_log "browser-ledger: ${runner[*]} $* --output-dir $partial"
  _sd_bl_run "$deadline" "${runner[@]}" "$@" --output-dir "$partial" 8>&- 9>&-
  rc=$?
  if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then _sd_bl_fail timeout; return 1; fi
  if [ "$rc" -ne 0 ] || [ ! -f "$partial/conformance.json" ]; then
    sd_log "browser-ledger: generator exit $rc"
    _sd_bl_fail generator_failed
    return 1
  fi

  # 3. P4-B 証拠（失敗しても公開能力の台帳は残す。ADR-0112 の fail closed）
  _sd_bl_run "$deadline" "${runner[@]}" --agent-browser "$ab" --p4b-evidence "$partial/conformance.json" \
    --p4b-backend claude-code --p4b-backend browser-specialist --output-dir "$partial" 8>&- 9>&-
  rc=$?
  if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then _sd_bl_fail timeout; return 1; fi
  if [ "$rc" -eq 0 ]; then p4b=true; else sd_log "browser-ledger: p4b evidence exit $rc (public-capability ledger kept)"; fi

  # 3b. credential 証拠（P4-B 完了時だけ。失敗しても公開台帳を維持）。launcher runtime で取る
  #     （ADR 2026-10-09-browser-launcher-credential-release）。launcher socket が無い host は生成器が
  #     launcher_unavailable と理由を出し、credential を空にした台帳を返す（release は止めない）。
  if [ "$p4b" = true ]; then
    mkdir -p "$partial/credential"
    local launcher_sock="${SD_BROWSER_LAUNCHER_SOCKET-/run/celeris-browser/launcher.sock}"
    [ -S "$launcher_sock" ] || launcher_sock=""
    _sd_bl_run "$deadline" env CELERIS_USERNS_TESTS=1 CELERIS_BROWSER_LAUNCHER_SOCKET="$launcher_sock" "${runner[@]}" \
        --credential-evidence "$partial/conformance.json" --credential-runtime launcher \
        --credential-backend claude-code --credential-backend browser-specialist \
        --output-dir "$partial/credential" 8>&- 9>&-
    rc=$?
    cred_json="$partial/credential/credential-evidence.json"
    if [ -f "$cred_json" ]; then
        cred_code="$(python3 - "$cred_json" "$rc" <<'PY'
import json,sys
try: print(json.load(open(sys.argv[1])).get('code') or ('ok' if sys.argv[2]=='0' else 'generator_failed'))
except Exception: print('ok' if sys.argv[2]=='0' else 'generator_failed')
PY
)"
        cred_reason="$(python3 - "$cred_json" <<'PY'
import json,sys
try: print(str(json.load(open(sys.argv[1])).get('reason') or '').replace('\\n',' ')[:500])
except Exception: pass
PY
)"
      elif [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then cred_code=timeout; cred_reason="credential evidence timed out"
      elif [ "$rc" -eq 0 ]; then cred_code=generator_failed; cred_reason="credential evidence output missing"
      else cred_code=generator_failed; cred_reason="credential evidence generator exit $rc"
      fi
    if [ "$rc" -eq 0 ] && [ -f "$partial/credential/conformance.json" ]; then
        mv -f "$partial/credential/conformance.json" "$partial/conformance.json"
        cred_ok=true
        cred_code=ok
        cred_reason=""
    elif [ -f "$partial/credential/conformance.json" ]; then
        # 失敗（launcher 無し等）の生成器の台帳は credential の claim を消している。それを採って credential を空にする。
        mv -f "$partial/credential/conformance.json" "$partial/conformance.json"
    fi
  else
    cred_code=p4b_incomplete
    cred_reason="P4-B evidence incomplete"
  fi
  sd_log "browser-ledger: credential evidence exit ${rc:-not-run} (code=$cred_code)${cred_reason:+: $cred_reason}"

  # 4. daemon と同じ判定で検査
  if [ ! -x "$check_bin" ]; then sd_log "browser-ledger: check binary $check_bin is not executable"; _sd_bl_fail check_failed; return 1; fi
  chk_json="$(_sd_bl_run "$deadline" "$check_bin" browser ledger check --file "$partial/conformance.json" \
    --release "$sha12" --no-host-probe --json 2>>/dev/stderr)"
  chk_rc=$?
  if [ "$chk_rc" -eq 124 ] || [ "$chk_rc" -eq 137 ]; then _sd_bl_fail timeout; return 1; fi
  if [ "$chk_rc" -ne 0 ]; then
    local code
    code="$(printf '%s' "$chk_json" | python3 -c 'import json,sys
try: print(json.load(sys.stdin).get("code",""))
except Exception: print("")' 2>/dev/null || true)"
    sd_log "browser-ledger: ledger check exit $chk_rc (code=${code:-?})"
    _sd_bl_fail "${code:-check_failed}"
    return 1
  fi

  # 5. 置く（status は partial の中に書いてから rename する）
  _sd_bl_write_status "$partial" true ok "$ab_ver" "$p4b" "$chk_json" "$cred_ok" "$cred_code" "$cred_reason"
  rm -rf "$final"
  mv -T "$partial" "$final" || { _sd_bl_fail check_failed; return 1; }
  SD_BROWSER_LEDGER_OK=true
  SD_BROWSER_LEDGER_CODE=ok
  sd_log "browser-ledger: placed $final/conformance.json (p4b=$p4b)"
  return 0
}
