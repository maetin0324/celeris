#!/bin/sh
# scripts/selfdeploy/prune-backups.sh <backups_dir> — ADR 2026-10-10-local-disk-growth-paths D4（backup 保持）。
#
# promote.sh の成功末尾から呼ばれ、`backups/` の 2 型だけを新しい順に刈る:
#   - `<YYYYMMDD-HHMMSS>-pre-<12 桁 hex>.sqlite3`（promote 前）: 既定 10 本（CELERIS_PROMOTE_BACKUP_KEEP）
#   - `<YYYYMMDD-HHMMSS>-pre-rollback.sqlite3`（rollback 前）:   既定 3 本（CELERIS_ROLLBACK_BACKUP_KEEP）
# 順序は名前の時刻（sd_stamp 形式は辞書順＝時刻順）。消す候補がある型は、消す前に残す最新 1 個を
# read-only で `PRAGMA integrity_check` し、出力が厳密に `ok` でない・検査が失敗したら**何も消さず**
# 理由を stderr に出して非 0 で終える。他の名前（celeris-*.sqlite3・*.json・*.log・*-pre-celeris*・
# 単独の -wal/-shm 等）には触れない。消す本体と対の -wal/-shm/-journal は一緒に消す。
#
# 実行: sh scripts/selfdeploy/prune-backups.sh <backups_dir>（POSIX sh）
set -eu

PROG=prune-backups
say() { printf '%s: %s\n' "$PROG" "$*" >&2; }
die() { say "error: $*"; exit 1; }

[ "$#" -eq 1 ] || { echo "usage: prune-backups.sh <backups_dir>" >&2; exit 2; }
DIR="$1"
[ -d "$DIR" ] || die "not a directory: $DIR"

check_keep() { # <name> <value>
  case "$2" in
    '' | *[!0-9]*) die "$1 must be a positive integer (got '$2')" ;;
  esac
  [ "$2" -ge 1 ] || die "$1 must be >= 1 (got '$2')"
}
PROMOTE_KEEP="${CELERIS_PROMOTE_BACKUP_KEEP:-10}"
ROLLBACK_KEEP="${CELERIS_ROLLBACK_BACKUP_KEEP:-3}"
check_keep CELERIS_PROMOTE_BACKUP_KEEP "$PROMOTE_KEEP"
check_keep CELERIS_ROLLBACK_BACKUP_KEEP "$ROLLBACK_KEEP"

STAMP='[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9]'
HEX='[0-9a-f]'
HEX12="$HEX$HEX$HEX$HEX$HEX$HEX$HEX$HEX$HEX$HEX$HEX$HEX"

# <kind> の型に合う名前を新しい順に 1 行ずつ出す（kind: promote | rollback）。
list_kind() {
  for f in "$DIR"/*.sqlite3; do
    [ -f "$f" ] || continue
    b="${f##*/}"
    # shellcheck disable=SC2254
    case "$1:$b" in
      promote:$STAMP-pre-$HEX12.sqlite3) printf '%s\n' "$b" ;;
      rollback:$STAMP-pre-rollback.sqlite3) printf '%s\n' "$b" ;;
    esac
  done | LC_ALL=C sort -r
}

integrity_ok() { # <basename>
  out="$(sqlite3 "file:$DIR/$1?mode=ro" 'PRAGMA integrity_check;' 2>&1)" || {
    say "integrity_check failed on $1: $out"
    return 1
  }
  if [ "$out" != ok ]; then
    say "integrity_check of $1 is not ok: $out"
    return 1
  fi
}

PROMOTE_LIST="$(list_kind promote)"
ROLLBACK_LIST="$(list_kind rollback)"
count() { if [ -z "$1" ]; then echo 0; else printf '%s\n' "$1" | wc -l | tr -d ' '; fi; }
PROMOTE_N="$(count "$PROMOTE_LIST")"
ROLLBACK_N="$(count "$ROLLBACK_LIST")"

# 消す前に、消す候補がある型の最新 1 個を全て検査する（1 つでも不合格なら何も消さない）。
BAD=0
if [ "$PROMOTE_N" -gt "$PROMOTE_KEEP" ]; then
  integrity_ok "$(printf '%s\n' "$PROMOTE_LIST" | head -n 1)" || BAD=1
fi
if [ "$ROLLBACK_N" -gt "$ROLLBACK_KEEP" ]; then
  integrity_ok "$(printf '%s\n' "$ROLLBACK_LIST" | head -n 1)" || BAD=1
fi
if [ "$BAD" -ne 0 ]; then
  say "refusing to delete any backup in $DIR: the newest backup to keep did not pass integrity_check"
  exit 1
fi

prune() { # <list> <keep> <label>
  [ -n "$1" ] || return 0
  printf '%s\n' "$1" | tail -n "+$(($2 + 1))" | while IFS= read -r b; do
    [ -n "$b" ] || continue
    rm -f -- "$DIR/$b" "$DIR/$b-wal" "$DIR/$b-shm" "$DIR/$b-journal"
    say "removed $3 backup $b"
  done
}
prune "$PROMOTE_LIST" "$PROMOTE_KEEP" promote
prune "$ROLLBACK_LIST" "$ROLLBACK_KEEP" rollback
say "kept promote=$([ "$PROMOTE_N" -gt "$PROMOTE_KEEP" ] && echo "$PROMOTE_KEEP" || echo "$PROMOTE_N")/$PROMOTE_KEEP rollback=$([ "$ROLLBACK_N" -gt "$ROLLBACK_KEEP" ] && echo "$ROLLBACK_KEEP" || echo "$ROLLBACK_N")/$ROLLBACK_KEEP in $DIR"
