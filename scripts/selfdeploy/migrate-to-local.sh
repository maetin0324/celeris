#!/usr/bin/env bash
# scripts/selfdeploy/migrate-to-local.sh <stage> [--dry-run] [rollback options] — ADR-0136「人が実行する移行の順序」。
#
#   サービスの hot データの正本を /local（btrfs）へ移す。**人だけが素のコマンドで実行する**
#   （実装エージェントは本番で実行しない。ADR-0040 D5 / ADR-0095 付記 D-d）。段は次の順に 1 つずつ呼ぶ:
#
#     presync   停止前の初回コピー。/local が btrfs の mount point で書けて自分の持ち物かを確かめ、
#               空の DB ディレクトリに `chattr +C` を付け（`lsattr -d` で確認）、hot の項目を rsync で写す。
#               DB family は写さない（稼働中の生コピーをしない）。容量が足りなければ gc を促して止まる。
#     gc        終端 task の workspace・target と scratch の回収候補を出す（`celerisctl workspace prune` /
#               `celerisctl scratch gc` の --dry-run）と、コピー対象の容量合計・/local の空き。--dry-run なしなら
#               その 2 つを実際に回す（running/reviewing と lease の保護は celerisctl 側。ADR-0129 の所有を
#               重複実装しない）。
#     stop      in-flight（running + reviewing）が 0 で release.sh / verify.sh が走っていないことを確かめ、
#               celeris@ / celeris-gui@ / celeris-credentiald@ <current> を止める。旧 DB を
#               `wal_checkpoint(TRUNCATE)` と `integrity_check` で検査する。
#     delta     停止中に差分を rsync --delete で写し直し、DB は SQLite backup API で `+C` の DB ディレクトリへ
#               写して `integrity_check`。`current`・`previous` が新しい tree の中で解決することを確かめる。
#     switch    config.toml の path の key と `paths.env` を書き換え（どちらも `.bak-migrate-<ts>` を残す）、
#               旧 hot の場所（~/.local/celeris/<項目>・/var/lib/celeris/<項目>）を `.bak-migrate-<ts>` に
#               退けて /local への symlink に替え（git worktree の gitdir や DB に残る旧い絶対 path を
#               解決させるため）、install-units.sh で unit を新しい根で設置する（旧 unit は記録に控える）。
#     start     /local の mount を確かめて daemon-reload し、stop で止めた unit を起こす。
#     verify    health・認証つき /api/v1/config の db・新 DB の integrity_check・current と gui の依存の
#               解決先・DB ディレクトリの `C`・空き容量・旧 path を指す symlink の残りを確かめる。
#     rollback  切り替え前の config・paths.env・unit・symlink（.bak の実体）に戻して旧 unit を起こす。
#               start 後（新 DB に書込みがあり得る）は `--restore-db-from-new`（新 DB を backup API で旧側へ
#               写す）か `--discard-new-writes`（新 DB の書込みを捨てる）を明示しない限り何もしない。
#
#   --dry-run  何も書かず・止めず・起こさずに、読むだけの確認と実行予定だけを出す（log も残さない）。
#
#   消さないもの: 旧 source（.bak-migrate-<ts> に退けるだけ）、/local の写し（rollback でも消さない）、
#   `~/.local/celeris/build-cache-nfs` と `~/.local/celeris/cache/sccache-l2`（写さず・消さず、verify が
#   人の削除手順を出すだけ）。account（claude-accounts・codex-accounts）、KB、backups の履歴、
#   ~/.config/celeris は home に残す（写さない）。
#
#   記録: `$MIGRATE_OLD_STATE_DIR/migrate-to-local/`（home 側。/local が壊れても rollback が読める）に
#   段ごとの log と、止めた unit・旧 DB の path・切り替えた項目の一覧を置く。
#
# env（試験と、既定と違う host のため）:
#   MIGRATE_LOCAL_ROOT       既定 /local（新しい根。state は <根>/celeris/state、data は <根>/celeris/data）
#   MIGRATE_OLD_STATE_DIR    既定 $HOME/.local/celeris
#   MIGRATE_OLD_VAR_DIR      既定 /var/lib/celeris
#   MIGRATE_MIN_FREE_GIB     既定 30（切り替え後に /local に残す空き。ADR-0136）
#   MIGRATE_VERIFY_TIMEOUT   既定 60（verify が health を待つ秒）
#   CELERISCTL               既定 <current>/bin/celerisctl、無ければ PATH の celerisctl
#   CELERIS_CONFIG_DIR / CELERIS_CONFIG / SD_UNIT_DIR は lib.sh・install-units.sh と同じ。
set -euo pipefail

# 旧い側の根を lib.sh に渡す（呼び出し側の env が既に /local を指していても旧 state を基準にする）。
MIGRATE_OLD_STATE_DIR="${MIGRATE_OLD_STATE_DIR:-$HOME/.local/celeris}"
export CELERIS_STATE_DIR="$MIGRATE_OLD_STATE_DIR"
unset CELERIS_BACKUPS_DIR CELERIS_LOGS_DIR

SD_PROG=migrate-to-local
# shellcheck source=lib.sh
SD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$SD_DIR/lib.sh"

usage() {
  cat >&2 <<'EOF'
usage: migrate-to-local.sh <stage> [--dry-run] [--restore-db-from-new | --discard-new-writes]

  <stage>  presync | gc | stop | delta | switch | start | verify | rollback（ADR-0136 の順）
  --dry-run              何も書かずに確認と実行予定だけを出す。
  --restore-db-from-new  rollback: start 後の新 DB を旧 DB の位置へ backup API で写してから戻す。
  --discard-new-writes   rollback: start 後の新 DB の書込みを捨てて旧 DB で戻す。
EOF
  exit 2
}

STAGE=""
DRY_RUN=false
RB_MODE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --restore-db-from-new) RB_MODE=restore ;;
    --discard-new-writes) RB_MODE=discard ;;
    -h | --help) usage ;;
    presync | gc | stop | delta | switch | start | verify | rollback)
      [ -z "$STAGE" ] || usage
      STAGE="$1"
      ;;
    *) usage ;;
  esac
  shift
done
[ -n "$STAGE" ] || usage
if [ -n "$RB_MODE" ] && [ "$STAGE" != rollback ]; then usage; fi

command -v python3 >/dev/null 2>&1 || sd_die "python3 is required (config rewrite and the SQLite backup API)"
SD_JSON_TOOL=python3

# ---- 場所（ADR-0136「path の契約」） -------------------------------------------

LOCAL_ROOT="${MIGRATE_LOCAL_ROOT:-/local}"
OLD_STATE="$MIGRATE_OLD_STATE_DIR"
OLD_VAR="${MIGRATE_OLD_VAR_DIR:-/var/lib/celeris}"
NEW_STATE="$LOCAL_ROOT/celeris/state"
NEW_DATA="$LOCAL_ROOT/celeris/data"
NEW_DB_DIR="$NEW_DATA/db"
NEW_DB="$NEW_DB_DIR/celeris.sqlite3"
MIN_FREE_BYTES=$(( ${MIGRATE_MIN_FREE_GIB:-30} * 1024 * 1024 * 1024 ))
PATHS_ENV="$CELERIS_CONFIG_DIR/paths.env"
UNIT_DIR="${SD_UNIT_DIR:-$HOME/.config/systemd/user}"
REC="$OLD_STATE/migrate-to-local"

# home の hot 項目（ADR-0136 の表）。current・previous は symlink のまま写す。
STATE_ITEMS="releases tools staging logs credentiald current previous"
# 写さない（home に残す・人が消す）。/var/lib/celeris 側は DB family 以外の全項目を data/<同名> へ写す。
NEVER_COPY_NOTE="claude-accounts codex-accounts backups build-cache-nfs cache/sccache-l2 (and ~/.local/share/celeris/knowledge)"

TS="$(sd_stamp)"

# 旧 DB: stop が記録した path を優先（switch 後は config が新 DB を指すため）。
if [ -f "$REC/old-db" ]; then OLD_DB="$(cat "$REC/old-db")"; else OLD_DB="$SD_DB"; fi
OLD_DB_BASE="$(basename "$OLD_DB")"

# ---- 道具 ------------------------------------------------------------------------

# 実行するか、dry-run なら予定として出すだけ。
run() {
  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: would run: $*"
  else
    sd_log "run: $*"
    "$@"
  fi
}

# 前提が満たされないとき: 本実行なら止まり、dry-run なら「止まるはず」と出して続ける。
need() {
  local msg="$1"
  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: would stop here: $msg"
  else
    sd_die "$msg"
  fi
}

begin_log() {
  [ "$DRY_RUN" = true ] && return 0
  mkdir -p "$REC/logs"
  SD_LOG_FILE="$REC/logs/$STAGE-$TS.log"
}

mark() { [ "$DRY_RUN" = true ] || printf '%s\n' "$(sd_ts)" >"$REC/$1"; }

bytes_of() {
  local p="$1"
  [ -e "$p" ] || [ -L "$p" ] || { echo 0; return 0; }
  du -sb "$p" 2>/dev/null | awk '{ print $1 }' | head -n 1
}

local_avail() { df -B1 --output=avail "$1" 2>/dev/null | tail -n 1 | tr -d ' '; }

human() { awk -v b="$1" 'BEGIN { printf "%.1fG", b / 1024 / 1024 / 1024 }'; }

# /var/lib/celeris の直下で data/<同名> へ写す項目（DB family と過去の退避は除く）。
var_items() {
  [ -d "$OLD_VAR" ] || return 0
  local p n
  for p in "$OLD_VAR"/* "$OLD_VAR"/.[!.]*; do
    [ -e "$p" ] || [ -L "$p" ] || continue
    n="$(basename "$p")"
    case "$n" in
      "$OLD_DB_BASE" | "$OLD_DB_BASE"-wal | "$OLD_DB_BASE"-shm | "$OLD_DB_BASE"-journal) continue ;;
      *.bak-migrate-* | *.moved-* | lost+found) continue ;;
    esac
    printf '%s\n' "$n"
  done
}

# 写す組を `src<TAB>dst` で出す。
copy_pairs() {
  local i n
  for i in $STATE_ITEMS; do
    if [ -e "$OLD_STATE/$i" ] || [ -L "$OLD_STATE/$i" ]; then printf '%s\t%s\n' "$OLD_STATE/$i" "$NEW_STATE/$i"; fi
  done
  for n in $(var_items); do printf '%s\t%s\n' "$OLD_VAR/$n" "$NEW_DATA/$n"; done
}

planned_bytes() {
  local total=0 src dst b
  while IFS="$(printf '\t')" read -r src dst; do
    b="$(bytes_of "$src")"
    total=$((total + ${b:-0}))
  done < <(copy_pairs)
  echo "$total"
}

# rsync 1 組。releases は停止中の web 配布物（<sha>/web/app）を除く（ADR-0136、人の方針 5）。
sync_pair() {
  local src="$1" dst="$2" args=(-aHA --numeric-ids --delete)
  case "$src" in "$OLD_STATE/releases") args+=(--exclude '/*/web/app/') ;; esac
  if [ -L "$src" ]; then
    # current・previous は相対 symlink（releases/<sha>）のまま張り直す（rsync は dst の symlink を辿る）。
    run ln -sfn "$(readlink "$src")" "$dst"
  elif [ ! -d "$src" ]; then
    run rsync -aHA --numeric-ids "$src" "$dst"
  else
    run mkdir -p "$dst"
    run rsync "${args[@]}" "$src/" "$dst/"
  fi
}

sync_all() {
  local src dst
  run mkdir -p "$NEW_STATE" "$NEW_DATA" "$NEW_STATE/backups" "$NEW_STATE/logs"
  while IFS="$(printf '\t')" read -r src dst; do sync_pair "$src" "$dst"; done < <(copy_pairs)
  if [ -d "$NEW_STATE/credentiald" ] || [ -d "$OLD_STATE/credentiald" ]; then run chmod 0700 "$NEW_STATE/credentiald"; fi
}

check_local_mount() {
  local fstype
  [ -d "$LOCAL_ROOT" ] || { need "$LOCAL_ROOT does not exist (is the bind mount there?); never create it on the rootfs"; return 0; }
  fstype="$(findmnt -n -o FSTYPE -M "$LOCAL_ROOT" 2>/dev/null | head -n 1 || true)"
  if [ -z "$fstype" ]; then
    need "$LOCAL_ROOT is not a mount point (findmnt -M); a directory on the rootfs is not /local"
  elif [ "$fstype" != btrfs ]; then
    need "$LOCAL_ROOT is $fstype, expected btrfs"
  else
    sd_log "ok: $LOCAL_ROOT is a btrfs mount point"
  fi
  [ -w "$LOCAL_ROOT" ] || need "$LOCAL_ROOT is not writable by $(id -un)"
  [ "$(stat -c %u "$LOCAL_ROOT")" = "$(id -u)" ] || need "$LOCAL_ROOT is not owned by uid $(id -u)"
}

db_dir_has_nocow() { lsattr -d "$NEW_DB_DIR" 2>/dev/null | awk '{ print $1 }' | grep -q C; }

# python の sqlite3（backup API・pragma）。
py_sqlite() {
  python3 - "$@" <<'PY'
import sqlite3, sys
op = sys.argv[1]
if op == "checkpoint":
    con = sqlite3.connect(sys.argv[2])
    row = con.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
    con.close()
    # (busy, log, checkpointed): busy=1 means a reader/writer blocked the full checkpoint.
    sys.exit(1 if row is not None and row[0] != 0 else 0)
elif op == "integrity":
    con = sqlite3.connect("file:" + sys.argv[2] + "?mode=ro", uri=True)
    rows = [r[0] for r in con.execute("PRAGMA integrity_check").fetchall()]
    con.close()
    print("\n".join(rows))
    sys.exit(0 if rows == ["ok"] else 1)
elif op == "backup":
    src = sqlite3.connect("file:" + sys.argv[2] + "?mode=ro", uri=True)
    dst = sqlite3.connect(sys.argv[3])
    src.backup(dst)
    dst.close()
    src.close()
else:
    sys.exit(2)
PY
}

remove_db_family() { rm -f "$1" "$1-wal" "$1-shm" "$1-journal"; }

celerisctl_bin() {
  if [ -n "${CELERISCTL:-}" ]; then printf '%s' "$CELERISCTL"; return 0; fi
  if [ -x "$SD_CURRENT/bin/celerisctl" ]; then printf '%s' "$SD_CURRENT/bin/celerisctl"; return 0; fi
  command -v celerisctl 2>/dev/null || true
}

service_units() {
  local cur="$1"
  printf '%s\n' "celeris@$cur" "celeris-gui@$cur" "celeris-credentiald@$cur"
}

# 旧い設定の hot path（解決済み）を `key<TAB>path` で出す。
old_config_paths() {
  [ -f "$SD_CONFIG" ] || return 0
  python3 - "$SD_CONFIG" "$HOME" <<'PY'
import os, sys, tomllib
cfg, home = sys.argv[1], sys.argv[2]
with open(cfg, "rb") as fh:
    data = tomllib.load(fh)
base = os.path.dirname(os.path.abspath(cfg))
def res(v):
    if not isinstance(v, str):
        return None
    if v.startswith("~/"):
        return os.path.join(home, v[2:])
    return v if os.path.isabs(v) else os.path.normpath(os.path.join(base, v))
def sect(name):
    v = data.get(name)
    return v if isinstance(v, dict) else {}
for key, val in (
    ("workspace_root", data.get("workspace_root")),
    ("workspace.build_cache_dir", sect("workspace").get("build_cache_dir")),
    ("scratch.dir", sect("scratch").get("dir")),
    ("containers.build_dir", sect("containers").get("build_dir")),
    ("memory.dir", sect("memory").get("dir")),
):
    p = res(val)
    if p:
        print(f"{key}\t{p}")
PY
}

# ---- 段 ----------------------------------------------------------------------------

stage_presync() {
  local need_b avail have key p
  sd_log "presync: $OLD_STATE + $OLD_VAR -> $LOCAL_ROOT/celeris (dry_run=$DRY_RUN)"
  check_local_mount
  while IFS="$(printf '\t')" read -r key p; do
    [ -e "$p" ] || continue
    case "$(readlink -f "$p")/" in
      "$(readlink -f "$OLD_VAR" 2>/dev/null || printf '%s' "$OLD_VAR")"/*) ;;
      *)
        case "$key" in
          workspace_root | memory.dir) need "configured $key=$p is outside $OLD_VAR; move it there first or migrate it by hand" ;;
          *) sd_log "warning: configured $key=$p is outside $OLD_VAR and will not be copied (regenerable)" ;;
        esac
        ;;
    esac
  done < <(old_config_paths)

  need_b="$(planned_bytes)"
  have="$(bytes_of "$LOCAL_ROOT/celeris")"
  avail="$(local_avail "$LOCAL_ROOT" || echo 0)"
  sd_log "planned copy (du, reflinks counted twice): $(human "$need_b"); already on $LOCAL_ROOT: $(human "$have"); free: $(human "${avail:-0}"); keep free: $(human "$MIN_FREE_BYTES")"
  if [ "$need_b" -gt $(( ${avail:-0} + have - MIN_FREE_BYTES )) ]; then
    need "not enough space on $LOCAL_ROOT; run 'migrate-to-local.sh gc' (and the ADR-0129 scratch GC) first"
  fi
  sd_log "not copied (home or deleted by a human): $NEVER_COPY_NOTE; the db family is copied in 'delta'"

  # DB ディレクトリは空のうちに +C（後から付けても既存 extent は変わらない）。
  if [ ! -d "$NEW_DB_DIR" ]; then
    run mkdir -p "$NEW_DB_DIR"
    run chmod 0750 "$NEW_DB_DIR"
    run chattr +C "$NEW_DB_DIR"
    if [ "$DRY_RUN" = false ]; then db_dir_has_nocow || sd_die "lsattr -d $NEW_DB_DIR does not show C after chattr +C"; fi
  elif ! db_dir_has_nocow; then
    if [ -z "$(ls -A "$NEW_DB_DIR")" ]; then
      run chattr +C "$NEW_DB_DIR"
    else
      need "$NEW_DB_DIR is not empty and has no C attribute; recreate it empty and rerun presync"
    fi
  else
    sd_log "ok: $NEW_DB_DIR already has the C attribute"
  fi

  sync_all
  sd_log "presync done (old sources untouched)"
}

stage_gc() {
  local ctl total avail
  sd_log "gc: candidates for terminal-task workspaces/targets and scratch (dry_run=$DRY_RUN)"
  ctl="$(celerisctl_bin)"
  if [ -z "$ctl" ]; then
    need "no celerisctl (set CELERISCTL or keep $SD_CURRENT/bin/celerisctl)"
  else
    sd_log "candidates: $ctl --db $OLD_DB workspace prune --config $SD_CONFIG --dry-run"
    "$ctl" --db "$OLD_DB" workspace prune --config "$SD_CONFIG" --dry-run >&2 || need "workspace prune --dry-run failed"
    sd_log "candidates: $ctl --db $OLD_DB scratch gc --config $SD_CONFIG --dry-run"
    "$ctl" --db "$OLD_DB" scratch gc --config "$SD_CONFIG" --dry-run >&2 || need "scratch gc --dry-run failed"
  fi
  if [ -d "$OLD_VAR/workspaces" ]; then
    sd_log "largest workspaces:"
    du -sb "$OLD_VAR"/workspaces/* 2>/dev/null | sort -rn | head -n 20 | while read -r b p; do sd_log "  $(human "$b") $p"; done
  fi
  total="$(planned_bytes)"
  avail="$(local_avail "$LOCAL_ROOT" 2>/dev/null || echo 0)"
  sd_log "total to copy now: $(human "$total") (target <= 240G per ADR-0136); free on $LOCAL_ROOT: $(human "${avail:-0}")"
  if [ -n "$ctl" ]; then
    run "$ctl" --db "$OLD_DB" workspace prune --config "$SD_CONFIG"
    run "$ctl" --db "$OLD_DB" scratch gc --config "$SD_CONFIG"
  fi
  sd_log "gc done; re-measure with 'gc --dry-run' and rerun presync"
}

in_flight_or_die() {
  local tasks running=0 reviewing=0
  if [ "$(sd_http_status "$SD_PROD_API/api/v1/health")" = 200 ]; then
    tasks="$(mktemp)"
    if sd_http_get "$SD_PROD_API/api/v1/tasks?limit=1" "$SD_API_TOKEN_FILE" >"$tasks" 2>/dev/null; then
      running="$(sd_json_get "$tasks" counts_by_status.running 2>/dev/null || echo 0)"
      reviewing="$(sd_json_get "$tasks" counts_by_status.reviewing 2>/dev/null || echo 0)"
      rm -f "$tasks"
    else
      rm -f "$tasks"
      need "could not read GET /api/v1/tasks; refusing to guess in-flight is 0"
      return 0
    fi
  else
    sd_log "celeris API is not reachable at $SD_PROD_API (already stopped?)"
  fi
  sd_log "in-flight: running=${running:-0} reviewing=${reviewing:-0}"
  if [ $(( ${running:-0} + ${reviewing:-0} )) -gt 0 ]; then
    need "in-flight is not 0; wait for it to drain and retry"
  fi
}

locks_free_or_die() {
  local f
  for f in "$SD_RELEASES/.lock-release" "$SD_STAGING/.lock"; do
    [ -f "$f" ] || continue
    if ! flock -n "$f" true 2>/dev/null; then need "$f is held (release.sh / verify.sh running); wait for it"; fi
  done
}

stage_stop() {
  local cur u active=()
  sd_log "stop (dry_run=$DRY_RUN)"
  in_flight_or_die
  locks_free_or_die
  cur="$(sd_current_sha)"
  [ -n "$cur" ] || need "no current release under $OLD_STATE"
  for u in $(service_units "$cur"); do
    if systemctl --user is-active --quiet "$u" 2>/dev/null; then active+=("$u"); fi
  done
  sd_log "units to stop: ${active[*]:-none}"
  if [ "$DRY_RUN" = false ]; then
    mkdir -p "$REC"
    printf '%s\n' "${active[@]}" >"$REC/stopped-units"
    printf '%s\n' "$OLD_DB" >"$REC/old-db"
  fi
  if [ "${#active[@]}" -gt 0 ]; then run systemctl --user stop "${active[@]}"; fi
  if [ -f "$OLD_DB" ]; then
    if [ "$DRY_RUN" = true ]; then
      sd_log "dry-run: would run PRAGMA wal_checkpoint(TRUNCATE) and integrity_check on $OLD_DB"
    else
      py_sqlite checkpoint "$OLD_DB" || sd_die "wal_checkpoint(TRUNCATE) on $OLD_DB was blocked; something still has it open. start again: systemctl --user start ${active[*]:-}"
      py_sqlite integrity "$OLD_DB" >/dev/null || sd_die "integrity_check on $OLD_DB is not ok; do not migrate. start again: systemctl --user start ${active[*]:-}"
      sd_log "ok: $OLD_DB checkpointed and integrity_check ok"
    fi
  else
    need "old db $OLD_DB does not exist"
  fi
  mark stopped
  sd_log "stop done; next: delta"
}

stage_delta() {
  local l t
  sd_log "delta (dry_run=$DRY_RUN)"
  [ -f "$REC/stopped" ] || need "stop has not run (no $REC/stopped)"
  [ ! -f "$REC/switched" ] || need "already switched; the new db may hold writes (use rollback first)"
  [ "$(sd_http_status "$SD_PROD_API/api/v1/health")" != 200 ] || need "celeris still answers on $SD_PROD_API; stop it first"
  check_local_mount
  if [ -d "$NEW_DB_DIR" ]; then
    db_dir_has_nocow || need "$NEW_DB_DIR has no C attribute (lsattr -d); rerun presync on an empty directory"
  else
    need "$NEW_DB_DIR does not exist; run presync first"
  fi
  sync_all
  # 前回の delta の写しは自分のもの（まだ switch していない）なので作り直す。
  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: would copy $OLD_DB -> $NEW_DB with the SQLite backup API and run integrity_check"
  else
    remove_db_family "$NEW_DB.tmp"
    remove_db_family "$NEW_DB"
    py_sqlite backup "$OLD_DB" "$NEW_DB.tmp" || sd_die "SQLite backup $OLD_DB -> $NEW_DB.tmp failed"
    py_sqlite integrity "$NEW_DB.tmp" >/dev/null || { remove_db_family "$NEW_DB.tmp"; sd_die "integrity_check on the copy is not ok; removed it"; }
    mv "$NEW_DB.tmp" "$NEW_DB"
    sd_log "ok: $NEW_DB (backup API copy, integrity_check ok)"
  fi
  for l in current previous; do
    [ -L "$OLD_STATE/$l" ] || continue
    t="$(readlink "$OLD_STATE/$l")"
    case "$t" in /*) need "$OLD_STATE/$l is an absolute symlink ($t); it would point outside $NEW_STATE" ;; esac
    if [ "$DRY_RUN" = false ] && [ ! -d "$NEW_STATE/$l/" ]; then sd_die "$NEW_STATE/$l does not resolve to a release in $NEW_STATE/releases"; fi
  done
  mark delta
  sd_log "delta done; next: switch"
}

# config.toml の path の key を新しい置き場に書き換えて <out> に書く（TOML として読めることも確かめる）。
rewrite_config() {
  python3 - "$1" "$2" "$NEW_DB" "$NEW_DATA" "$NEW_STATE" <<'PY'
import json, re, sys, tomllib
src, out, new_db, data, state = sys.argv[1:6]
lines = open(src, encoding="utf-8").read().splitlines(keepends=True)
HEADER = re.compile(r"^\s*\[")
TABLE = re.compile(r"^\s*\[([^\[\]]+)\]\s*(#.*)?$")

def first_header():
    for i, l in enumerate(lines):
        if HEADER.match(l):
            return i
    return len(lines)

def find_section(name):
    for i, l in enumerate(lines):
        m = TABLE.match(l)
        if m and m.group(1).strip() == name:
            j = i + 1
            while j < len(lines) and not HEADER.match(lines[j]):
                j += 1
            return i, j
    return None

def kv(key, value):
    return f"{key} = {json.dumps(value)}\n"

def set_in(start, end, key, value, insert_at):
    pat = re.compile(r"^\s*" + re.escape(key) + r"\s*=")
    for k in range(start, end):
        if pat.match(lines[k]):
            lines[k] = kv(key, value)
            return
    lines.insert(insert_at, kv(key, value))

def set_top(key, value):
    set_in(0, first_header(), key, value, 0)

def set_section(name, key, value, only_if_present=False):
    span = find_section(name)
    if span is None:
        if only_if_present:
            return
        if lines and not lines[-1].endswith("\n"):
            lines[-1] += "\n"
        lines.extend(["\n", f"[{name}]\n"])
        lines.append(kv(key, value))
        return
    i, j = span
    set_in(i + 1, j, key, value, i + 1)

# 旧い top-level `db = "..."` は [db] 表と両立しないので消して [db].path に寄せる。
top = re.compile(r"^\s*db\s*=")
for k in range(first_header()):
    if top.match(lines[k]):
        del lines[k]
        break
set_top("workspace_root", f"{data}/workspaces")
set_section("db", "path", new_db)
set_section("db", "backup_dir", f"{state}/backups")
set_section("workspace", "build_cache_dir", f"{data}/build-cache")
set_section("scratch", "dir", f"{data}/scratch")
set_section("containers", "build_dir", f"{data}/containers")
set_section("memory", "dir", f"{data}/memory", only_if_present=True)
set_section("selfdeploy", "releases_dir", f"{state}/releases")
text = "".join(lines)
tomllib.loads(text)
with open(out, "w", encoding="utf-8") as fh:
    fh.write(text)
PY
}

# 旧い場所 <old> を <old>.bak-migrate-<ts> に退けて <new> への symlink にする。
swap_to_link() {
  local old="$1" new="$2"
  [ -e "$old" ] || [ -L "$old" ] || return 0
  if [ -L "$old" ] && [ "$(readlink "$old")" = "$new" ]; then return 0; fi
  [ -e "$new" ] || [ -L "$new" ] || need "$new is missing; rerun delta"
  run mv -T "$old" "$old.bak-migrate-$TS"
  run ln -s "$new" "$old"
  [ "$DRY_RUN" = true ] || printf '%s\t%s\n' "$old" "$old.bak-migrate-$TS" >>"$REC/switched-items"
}

stage_switch() {
  local i n f
  sd_log "switch (dry_run=$DRY_RUN)"
  [ -f "$REC/delta" ] || need "delta has not run (no $REC/delta)"
  [ ! -f "$REC/switched" ] || need "already switched"
  [ "$(sd_http_status "$SD_PROD_API/api/v1/health")" != 200 ] || need "celeris still answers on $SD_PROD_API; stop it first"
  [ -f "$SD_CONFIG" ] || need "$SD_CONFIG does not exist"
  check_local_mount

  local tmp
  tmp="$(mktemp)"
  if [ -f "$SD_CONFIG" ]; then rewrite_config "$SD_CONFIG" "$tmp" || { rm -f "$tmp"; need "could not rewrite $SD_CONFIG"; }; fi
  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: config.toml would become (diff):"
    diff -u "$SD_CONFIG" "$tmp" >&2 || true
    sd_log "dry-run: would write $PATHS_ENV with CELERIS_STATE_DIR=$NEW_STATE"
    rm -f "$tmp"
  else
    : >"$REC/switched-items"
    printf '%s\n' "$TS" >"$REC/switch-ts"
    cp -p "$SD_CONFIG" "$SD_CONFIG.bak-migrate-$TS"
    cat "$tmp" >"$SD_CONFIG"
    rm -f "$tmp"
    sd_log "rewrote $SD_CONFIG (backup: $SD_CONFIG.bak-migrate-$TS)"
    if [ -f "$PATHS_ENV" ]; then cp -p "$PATHS_ENV" "$PATHS_ENV.bak-migrate-$TS"; else : >"$REC/paths-env-absent"; fi
    {
      printf '# ADR-0136: hot data roots (written by migrate-to-local.sh switch %s). No secrets here.\n' "$TS"
      printf 'CELERIS_STATE_DIR=%s\n' "$NEW_STATE"
      printf 'CELERIS_BACKUPS_DIR=%s\n' "$NEW_STATE/backups"
      printf 'CELERIS_LOGS_DIR=%s\n' "$NEW_STATE/logs"
      printf 'CELERIS_CREDENTIALD_DATA_DIR=%s\n' "$NEW_STATE/credentiald"
    } >"$PATHS_ENV"
    sd_log "wrote $PATHS_ENV"
    # 旧 unit を控える（rollback がそのまま戻す）。
    mkdir -p "$REC/units-before"
    for f in "$UNIT_DIR"/celeris*; do [ -e "$f" ] && cp -p "$f" "$REC/units-before/"; done
  fi

  for i in $STATE_ITEMS; do swap_to_link "$OLD_STATE/$i" "$NEW_STATE/$i"; done
  for n in $(var_items); do swap_to_link "$OLD_VAR/$n" "$NEW_DATA/$n"; done
  # 旧 DB family は退けるだけ（symlink にしない。誤って旧 path で開いても新 DB を壊さない）。
  for f in "$OLD_DB" "$OLD_DB-wal" "$OLD_DB-shm"; do
    [ -e "$f" ] || continue
    run mv -T "$f" "$f.bak-migrate-$TS"
    [ "$DRY_RUN" = true ] || printf '%s\t%s\n' "$f" "$f.bak-migrate-$TS" >>"$REC/switched-db"
  done

  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: would run CELERIS_STATE_DIR=$NEW_STATE $SD_DIR/install-units.sh"
  else
    CELERIS_STATE_DIR="$NEW_STATE" SD_UNIT_DIR="$UNIT_DIR" bash "$SD_DIR/install-units.sh" \
      || sd_die "install-units.sh failed; run 'migrate-to-local.sh rollback'"
  fi
  mark switched
  sd_log "switch done; next: start"
}

stage_start() {
  local units=() u
  sd_log "start (dry_run=$DRY_RUN)"
  [ -f "$REC/switched" ] || need "switch has not run"
  check_local_mount
  if [ -f "$REC/stopped-units" ]; then
    while read -r u; do [ -n "$u" ] && units+=("$u"); done <"$REC/stopped-units"
  fi
  run systemctl --user daemon-reload
  if [ "${#units[@]}" -gt 0 ]; then run systemctl --user start "${units[@]}"; else sd_log "no units were recorded by stop; start them by hand"; fi
  mark started
  sd_log "start done; next: verify"
}

stage_verify() {
  local bad=0 got cfg cur avail stale gui
  sd_log "verify (read-only)"
  if [ "$DRY_RUN" = true ]; then
    sd_log "dry-run: verify only reads; running the same checks"
  fi
  if sd_wait_http_200 "$SD_PROD_API/api/v1/health" "${MIGRATE_VERIFY_TIMEOUT:-60}"; then
    sd_log "ok: health 200"
    cfg="$(mktemp)"
    if sd_http_get "$SD_PROD_API/api/v1/config" "$SD_API_TOKEN_FILE" >"$cfg" 2>/dev/null; then
      got="$(sd_json_get "$cfg" db 2>/dev/null || echo "")"
      if [ "$got" = "$NEW_DB" ]; then sd_log "ok: /api/v1/config db = $got"; else sd_log "NG: /api/v1/config db = '$got', expected $NEW_DB"; bad=1; fi
    else
      sd_log "NG: could not read GET /api/v1/config"
      bad=1
    fi
    rm -f "$cfg"
  else
    sd_log "NG: health did not answer 200 on $SD_PROD_API"
    bad=1
  fi
  if [ -f "$NEW_DB" ] && py_sqlite integrity "$NEW_DB" >/dev/null 2>&1; then sd_log "ok: integrity_check $NEW_DB"; else sd_log "NG: integrity_check $NEW_DB"; bad=1; fi
  if db_dir_has_nocow; then sd_log "ok: $NEW_DB_DIR has C"; else sd_log "NG: $NEW_DB_DIR has no C attribute"; bad=1; fi
  cur="$(readlink -f "$NEW_STATE/current" 2>/dev/null || true)"
  case "$cur" in
    "$(readlink -f "$NEW_STATE")"/releases/*) sd_log "ok: current -> $cur" ;;
    *) sd_log "NG: $NEW_STATE/current resolves to '$cur'"; bad=1 ;;
  esac
  if [ -e "$NEW_STATE/current/gui/node_modules" ] || [ -L "$NEW_STATE/current/gui/node_modules" ]; then
    gui="$(readlink -f "$NEW_STATE/current/gui/node_modules" 2>/dev/null || true)"
    case "$gui" in
      "$(readlink -f "$LOCAL_ROOT")"/*) sd_log "ok: gui/node_modules -> $gui" ;;
      *) sd_log "NG: gui/node_modules resolves to '$gui' (outside $LOCAL_ROOT)"; bad=1 ;;
    esac
  fi
  avail="$(local_avail "$LOCAL_ROOT" || echo 0)"
  if [ "${avail:-0}" -ge "$MIN_FREE_BYTES" ]; then sd_log "ok: free on $LOCAL_ROOT $(human "$avail")"; else sd_log "NG: free on $LOCAL_ROOT $(human "${avail:-0}") < $(human "$MIN_FREE_BYTES")"; bad=1; fi
  stale="$(find "$NEW_STATE" "$NEW_DATA" -maxdepth 4 -type l \( -lname "$OLD_STATE/*" -o -lname "$OLD_VAR/*" \) 2>/dev/null | head -n 20 || true)"
  if [ -n "$stale" ]; then
    sd_log "warning: symlinks under $LOCAL_ROOT still point at the old places (they resolve through the switch symlinks):"
    printf '%s\n' "$stale" | while read -r l; do sd_log "  $l"; done
  fi
  sd_log "left for a human (not done by this script):"
  sd_log "  - copy verified new backups from $NEW_STATE/backups to the home archive"
  sd_log "  - after confirming nothing references them: rm -rf $OLD_STATE/build-cache-nfs $OLD_STATE/cache/sccache-l2 (ADR-0129)"
  sd_log "  - delete the *.bak-migrate-* sources only after the new layout has run for a while"
  if [ "$bad" -ne 0 ]; then need "verify found problems (see NG above); 'migrate-to-local.sh rollback' restores the old layout"; fi
  [ "$DRY_RUN" = true ] || mark verified
  sd_log "verify ok"
}

stage_rollback() {
  local units=() u old bak f
  sd_log "rollback (dry_run=$DRY_RUN, mode=${RB_MODE:-none})"
  if [ -f "$REC/started" ] && [ -z "$RB_MODE" ]; then
    need "the new instance was started; $NEW_DB may hold writes. choose --restore-db-from-new or --discard-new-writes"
  fi
  if [ -f "$REC/stopped-units" ]; then
    while read -r u; do [ -n "$u" ] && units+=("$u"); done <"$REC/stopped-units"
  fi
  local active=()
  for u in "${units[@]}"; do
    if systemctl --user is-active --quiet "$u" 2>/dev/null; then active+=("$u"); fi
  done
  if [ "${#active[@]}" -gt 0 ]; then run systemctl --user stop "${active[@]}"; fi

  if [ -f "$REC/switched-items" ]; then
    while IFS="$(printf '\t')" read -r old bak; do
      [ -n "$old" ] || continue
      if [ -L "$old" ]; then run rm -f "$old"; fi
      if [ -e "$bak" ] || [ -L "$bak" ]; then run mv -T "$bak" "$old"; fi
    done <"$REC/switched-items"
  fi
  if [ -f "$REC/switched-db" ]; then
    while IFS="$(printf '\t')" read -r old bak; do
      if [ -e "$bak" ]; then run mv -T "$bak" "$old"; fi
    done <"$REC/switched-db"
  fi
  if [ "$RB_MODE" = restore ]; then
    if [ "$DRY_RUN" = true ]; then
      sd_log "dry-run: would copy $NEW_DB -> $OLD_DB with the SQLite backup API (old db kept as $OLD_DB.pre-rollback-$TS)"
    else
      for f in "$OLD_DB" "$OLD_DB-wal" "$OLD_DB-shm"; do [ -e "$f" ] && mv -T "$f" "${f/$OLD_DB/$OLD_DB.pre-rollback-$TS}"; done
      py_sqlite backup "$NEW_DB" "$OLD_DB" || sd_die "SQLite backup $NEW_DB -> $OLD_DB failed (old db is $OLD_DB.pre-rollback-$TS)"
      py_sqlite integrity "$OLD_DB" >/dev/null || sd_die "integrity_check on restored $OLD_DB is not ok (old db is $OLD_DB.pre-rollback-$TS)"
      sd_log "ok: restored $OLD_DB from $NEW_DB"
    fi
  fi

  if [ -f "$REC/switch-ts" ]; then
    local sts
    sts="$(cat "$REC/switch-ts")"
    if [ -f "$SD_CONFIG.bak-migrate-$sts" ]; then run cp -p "$SD_CONFIG.bak-migrate-$sts" "$SD_CONFIG"; fi
    if [ -f "$REC/paths-env-absent" ]; then
      run rm -f "$PATHS_ENV"
    elif [ -f "$PATHS_ENV.bak-migrate-$sts" ]; then
      run cp -p "$PATHS_ENV.bak-migrate-$sts" "$PATHS_ENV"
    fi
    if [ -d "$REC/units-before" ]; then
      for f in "$UNIT_DIR"/celeris*; do [ -e "$f" ] && run rm -f "$f"; done
      for f in "$REC/units-before"/*; do [ -e "$f" ] && run cp -p "$f" "$UNIT_DIR/"; done
    fi
  fi
  run systemctl --user daemon-reload
  if [ "${#units[@]}" -gt 0 ]; then run systemctl --user start "${units[@]}"; fi
  if [ "$DRY_RUN" = false ] && [ -d "$REC" ]; then
    mkdir -p "$REC/history"
    for f in stopped stopped-units old-db delta switched switch-ts switched-items switched-db paths-env-absent started verified; do
      [ -e "$REC/$f" ] && mv "$REC/$f" "$REC/history/$f.rolled-back-$TS"
    done
    [ -d "$REC/units-before" ] && mv "$REC/units-before" "$REC/history/units-before.rolled-back-$TS"
  fi
  sd_log "rollback done; the copies under $LOCAL_ROOT/celeris are kept (delete by hand if unwanted)"
}

begin_log
case "$STAGE" in
  presync) stage_presync ;;
  gc) stage_gc ;;
  stop) stage_stop ;;
  delta) stage_delta ;;
  switch) stage_switch ;;
  start) stage_start ;;
  verify) stage_verify ;;
  rollback) stage_rollback ;;
esac
if [ "$DRY_RUN" = true ]; then sd_log "dry-run: nothing was changed"; fi
