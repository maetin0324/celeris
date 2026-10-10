#!/usr/bin/env bash
# scripts/selfdeploy/tests/promote_backup_retention.sh — ADR 2026-10-10-local-disk-growth-paths D4
# 「promote 前・rollback 前 backup の保持と削除前 integrity_check」（prune-backups.sh）を固定する。
#
# 本番には触れない: 一時ディレクトリと sqlite3 だけを使う（systemctl も本番 path も呼ばない）。
# 実行: bash scripts/selfdeploy/tests/promote_backup_retention.sh（引数なし）
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"
PRUNE="$SD/prune-backups.sh"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }
assert_eq() { if [ "$2" = "$3" ]; then ok "$1"; else ng "$1 (want=[$2] got=[$3])"; fi; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/promote-backup-retention.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/home"
export HOME="$WORK/home" SD_PRUNE_ALLOWED_ROOT="$WORK"

# 本物の小さな SQLite DB を 1 つ作り、backup はその写しにする。
SEED="$WORK/seed.sqlite3"
sqlite3 "$SEED" 'CREATE TABLE t(x); INSERT INTO t VALUES (1);'

new_dir() { local d="$WORK/$1"; mkdir -p "$d"; printf '%s' "$d"; }
# 2026-10-01 00:00:NN から n 本の promote 前 backup（sha は本ごとに違う 12 桁 hex）。
make_promote() { # <dir> <n>
  local i
  for i in $(seq 1 "$2"); do
    cp "$SEED" "$1/$(printf '20261001-0000%02d-pre-%012x.sqlite3' "$i" "$((0xabc000 + i))")"
  done
}
make_rollback() { # <dir> <n>
  local i
  for i in $(seq 1 "$2"); do cp "$SEED" "$1/$(printf '20261002-0000%02d-pre-rollback.sqlite3' "$i")"; done
}
count_glob() { local n=0 f; for f in "$1"/$2; do [ -e "$f" ] && n=$((n + 1)); done; echo "$n"; }
listing() { (cd "$1" && LC_ALL=C ls -1); }

promote_backup_retention_default_keeps_10() {
  local d; d="$(new_dir default)"
  make_promote "$d" 14
  # 消える最古の 1 本に対の -wal/-shm を置く（一緒に消えること）
  : >"$d/20261001-000001-pre-000000abc001.sqlite3-wal"
  : >"$d/20261001-000001-pre-000000abc001.sqlite3-shm"
  sh "$PRUNE" "$d" 2>"$WORK/default.err"
  assert_eq "default: exit 0" 0 "$?"
  assert_eq "default: 10 promote backups kept" 10 "$(count_glob "$d" '*-pre-*.sqlite3')"
  assert_eq "default: oldest kept is #5" "20261001-000005-pre-000000abc005.sqlite3" "$(listing "$d" | head -n 1)"
  assert_eq "default: newest kept" yes "$([ -f "$d/20261001-000014-pre-000000abc00e.sqlite3" ] && echo yes || echo no)"
  assert_eq "default: paired -wal/-shm removed" "0 0" "$(count_glob "$d" '*-wal') $(count_glob "$d" '*-shm')"
}

promote_backup_retention_env_override() {
  local d; d="$(new_dir env)"
  make_promote "$d" 6
  CELERIS_PROMOTE_BACKUP_KEEP=2 sh "$PRUNE" "$d" 2>/dev/null
  assert_eq "env: exit 0" 0 "$?"
  assert_eq "env: 2 promote backups kept" "20261001-000005-pre-000000abc005.sqlite3 20261001-000006-pre-000000abc006.sqlite3" \
    "$(listing "$d" | tr '\n' ' ' | sed 's/ $//')"
  local before; before="$(listing "$d")"
  CELERIS_PROMOTE_BACKUP_KEEP=0 sh "$PRUNE" "$d" 2>/dev/null
  [ "$?" -ne 0 ] && ok "env: keep=0 rejected" || ng "env: keep=0 accepted"
  CELERIS_ROLLBACK_BACKUP_KEEP=abc sh "$PRUNE" "$d" 2>/dev/null
  [ "$?" -ne 0 ] && ok "env: keep=abc rejected" || ng "env: keep=abc accepted"
  assert_eq "env: rejected keep deletes nothing" "$before" "$(listing "$d")"
}

promote_backup_retention_rollback_separate_3() {
  local d; d="$(new_dir rollback)"
  make_promote "$d" 4
  make_rollback "$d" 5
  sh "$PRUNE" "$d" 2>/dev/null
  assert_eq "rollback: exit 0" 0 "$?"
  assert_eq "rollback: 3 rollback backups kept" 3 "$(count_glob "$d" '*-pre-rollback.sqlite3')"
  assert_eq "rollback: promote backups untouched (under 10)" 4 "$(count_glob "$d" '*-pre-[0-9a-f]*[0-9a-f].sqlite3')"
  assert_eq "rollback: oldest rollback kept is #3" yes "$([ -f "$d/20261002-000003-pre-rollback.sqlite3" ] && [ ! -e "$d/20261002-000002-pre-rollback.sqlite3" ] && echo yes || echo no)"
}

promote_backup_retention_other_files_untouched() {
  local d; d="$(new_dir other)"
  make_promote "$d" 12
  make_rollback "$d" 4
  local others="celeris-20261001-000000.sqlite3 celeris-20261001-000001.sqlite3 promote-20261001-000001.health-before.json
promote-20261001-000001.log 20260901-000000-pre-celeris.sqlite3 20260901-000000-pre-celeris-v2.sqlite3
20261001-000099-pre-orphanwal00.sqlite3-wal 20261001-000098-pre-000000abc0ff.sqlite3-shm
20261001-000000-pre-ABCDEF012345.sqlite3 20261001-000000-pre-abc.sqlite3 2026101-000000-pre-000000abc001.sqlite3
notes.txt 20261001-000000-pre-000000abc001.sqlite3.bak"
  local f
  for f in $others; do printf 'x' >"$d/$f"; done
  local sums_before; sums_before="$(cd "$d" && for f in $others; do cksum "$f"; done)"
  sh "$PRUNE" "$d" 2>/dev/null
  assert_eq "other: exit 0" 0 "$?"
  assert_eq "other: files of other types unchanged" "$sums_before" "$(cd "$d" && for f in $others; do cksum "$f" 2>&1; done)"
  assert_eq "other: promote pruned to 10" 10 "$(listing "$d" | grep -cE '^[0-9]{8}-[0-9]{6}-pre-[0-9a-f]{12}\.sqlite3$')"
  assert_eq "other: rollback pruned to 3" 3 "$(count_glob "$d" '*-pre-rollback.sqlite3')"
}

promote_backup_retention_integrity_failure_deletes_nothing() {
  local d before rc
  # promote 型: 最新を壊れた file にする
  d="$(new_dir corrupt-promote)"
  make_promote "$d" 13
  make_rollback "$d" 5
  printf 'this is not a sqlite database\n%.0s' $(seq 1 200) >"$d/20261001-000013-pre-000000abc00d.sqlite3"
  before="$(listing "$d")"
  sh "$PRUNE" "$d" 2>"$WORK/corrupt.err"; rc=$?
  [ "$rc" -ne 0 ] && ok "integrity(promote): non-zero exit ($rc)" || ng "integrity(promote): exit 0"
  assert_eq "integrity(promote): nothing deleted (rollback too)" "$before" "$(listing "$d")"
  grep -q 'integrity_check' "$WORK/corrupt.err" && ok "integrity(promote): reason on stderr" ||
    ng "integrity(promote): no reason on stderr: $(cat "$WORK/corrupt.err")"
  # rollback 型: 最新を空ページの壊れた DB にする
  d="$(new_dir corrupt-rollback)"
  make_promote "$d" 12
  make_rollback "$d" 4
  head -c 4096 /dev/zero >"$d/20261002-000004-pre-rollback.sqlite3"
  before="$(listing "$d")"
  sh "$PRUNE" "$d" 2>/dev/null; rc=$?
  [ "$rc" -ne 0 ] && ok "integrity(rollback): non-zero exit ($rc)" || ng "integrity(rollback): exit 0"
  assert_eq "integrity(rollback): nothing deleted (promote too)" "$before" "$(listing "$d")"
}

promote_backup_retention_promote_calls_prune() {
  if grep -q 'prune-backups\.sh' "$SD/promote.sh" &&
    awk '/sd_log "promoted \$SHA12/{p=1} p && /sh "\$PRUNE_BACKUPS" "\$SD_BACKUPS"/{f=1} END{exit !f}' "$SD/promote.sh"; then
    ok "promote.sh calls prune-backups.sh after 'promoted \$SHA12'"
  else
    ng "promote.sh does not call prune-backups.sh at its successful end"
  fi
  grep -q 'warning: prune-backups.sh' "$SD/promote.sh" && ok "promote.sh only warns on prune failure" ||
    ng "promote.sh has no warning path for prune failure"
  assert_eq "prune-backups.sh mode 100755" 100755 "$(git -C "$SD" ls-files -s prune-backups.sh 2>/dev/null | awk '{print $1}' || true)"
}

promote_backup_retention_default_keeps_10
promote_backup_retention_env_override
promote_backup_retention_rollback_separate_3
promote_backup_retention_other_files_untouched
promote_backup_retention_integrity_failure_deletes_nothing
promote_backup_retention_promote_calls_prune

if [ "$FAIL" -ne 0 ]; then echo "promote_backup_retention: FAILED" >&2; exit 1; fi
echo "promote_backup_retention: all ok"
