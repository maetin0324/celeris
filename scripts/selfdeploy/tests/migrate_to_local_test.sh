#!/usr/bin/env bash
# scripts/selfdeploy/tests/migrate_to_local_test.sh — ADR-0136「人が実行する移行の順序」:
#
#   1. 全段の --dry-run が tempdir の home・/var/lib/celeris 相当・/local 相当を 1 byte も変えず、
#      systemctl の stop/start/daemon-reload も celerisctl の本実行もしないこと。
#   2. presync → gc → stop → delta → switch → start → verify が通り、hot が /local 側に揃い
#      （web/app・account・build-cache-nfs・sccache-l2 は写さない）、DB が +C の DB ディレクトリに写ること。
#   3. start 後の rollback は書込みの扱いを明示しないと何もせず、--discard-new-writes で切り替え前の
#      config・paths.env・unit・symlink（実体）に戻ること。
#   4. --restore-db-from-new が新 DB の書込みを旧 DB に持ち帰ること。
#
# 本番には触れない: HOME・/local・/var/lib/celeris 相当は一時ディレクトリ。systemctl・curl・celerisctl・
# findmnt・chattr・lsattr は PATH 先頭（celerisctl は偽の release の bin）の偽物。外部ネットワークに出ない。
# 実行: bash scripts/selfdeploy/tests/migrate_to_local_test.sh（引数なし）
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$(cd "$HERE/.." && pwd)/migrate-to-local.sh"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

SHA=aaaaaaaaaaaa
PREV=bbbbbbbbbbbb

write_stubs() {
  local bin="$1"
  mkdir -p "$bin"
  cat >"$bin/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$STUB_STATE/systemctl.log"
[ "${1:-}" = --user ] && shift
cmd="${1:-}"; shift || true
case "$cmd" in
  is-active)
    [ "${1:-}" = --quiet ] && shift
    grep -qxF "$1" "$STUB_STATE/active" ;;
  stop)
    for u in "$@"; do grep -vxF "$u" "$STUB_STATE/active" >"$STUB_STATE/active.new" || true; mv "$STUB_STATE/active.new" "$STUB_STATE/active"; done ;;
  start)
    for u in "$@"; do echo "$u" >>"$STUB_STATE/active"; done ;;
  *) exit 0 ;;
esac
EOF
  cat >"$bin/curl" <<'EOF'
#!/usr/bin/env bash
out=/dev/stdout; fmt=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -w) fmt="$2"; shift ;;
    -m | -H | -X | --data-binary | --max-redirs) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
echo "$url" >>"$STUB_STATE/curl.log"
grep -q '^celeris@' "$STUB_STATE/active" 2>/dev/null || exit 7
case "$url" in
  */api/v1/health) body='{"status":"ok"}' ;;
  */api/v1/tasks*) body='{"counts_by_status":{"running":0,"reviewing":0}}' ;;
  */api/v1/config)
    body="$(python3 -c '
import json, sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
db = d.get("db") or d.get("db", {})
if isinstance(db, dict):
    db = db.get("path", "")
print(json.dumps({"db": db}))' "$HOME/.config/celeris/config.toml")" ;;
  *) exit 7 ;;
esac
printf '%s' "$body" >"$out"
[ -z "$fmt" ] || printf '200'
EOF
  cat >"$bin/findmnt" <<'EOF'
#!/usr/bin/env bash
echo btrfs
EOF
  cat >"$bin/chattr" <<'EOF'
#!/usr/bin/env bash
[ "$1" = +C ] || exit 1
echo "$2" >>"$STUB_STATE/nocow"
EOF
  cat >"$bin/lsattr" <<'EOF'
#!/usr/bin/env bash
[ "$1" = -d ] || exit 1
if grep -qxF "$2" "$STUB_STATE/nocow" 2>/dev/null; then echo "---------------C------ $2"; else echo "---------------------- $2"; fi
EOF
  chmod +x "$bin"/*
}

# setup <dir>: home・/var/lib/celeris・/local の相当と偽物を作る。
setup() {
  local d="$1" st var
  mkdir -p "$d/stub-state"
  write_stubs "$d/bin"
  st="$d/home/.local/celeris"
  var="$d/var-lib-celeris"
  mkdir -p "$st/releases/$SHA/bin" "$st/releases/$SHA/gui" "$st/releases/$SHA/web/app" "$st/releases/$PREV/bin" \
    "$st/releases/.pnpm-prod-cache/k1/node_modules" "$st/releases/.build/tree" \
    "$st/tools/ldr/bin" "$st/staging" "$st/logs" "$st/credentiald" "$st/claude-accounts" "$st/codex-accounts" \
    "$st/backups" "$st/build-cache-nfs" "$st/cache/sccache-l2" \
    "$var/workspaces/T1/repos/r" "$var/scratch/pool" "$var/build-cache" "$var/web" "$var/memory" "$var/release-build" \
    "$d/local" "$d/home/.config/celeris" "$d/home/.config/systemd/user"
  echo x >"$st/releases/.pnpm-prod-cache/k1/node_modules/x.js"
  ln -s ../../.pnpm-prod-cache/k1/node_modules "$st/releases/$SHA/gui/node_modules"
  echo app >"$st/releases/$SHA/web/app/index.html"
  echo tree >"$st/releases/.build/tree/README"
  cat >"$st/releases/$SHA/bin/celerisctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$STUB_STATE/celerisctl.log"
echo "candidate: workspaces/T0/repos/r/target"
EOF
  chmod +x "$st/releases/$SHA/bin/celerisctl"
  ln -s "releases/$SHA" "$st/current"
  ln -s "releases/$PREV" "$st/previous"
  echo '#!/bin/sh' >"$st/tools/ldr/bin/ldr"
  chmod +x "$st/tools/ldr/bin/ldr"
  echo s >"$st/staging/s"
  echo l >"$st/logs/l.log"
  echo vault >"$st/credentiald/vault"
  chmod 0700 "$st/credentiald"
  echo secret >"$st/claude-accounts/cred.json"
  echo secret >"$st/codex-accounts/auth.json"
  echo b >"$st/backups/old.sqlite3"
  echo nfs >"$st/build-cache-nfs/blob"
  echo l2 >"$st/cache/sccache-l2/blob"
  echo w >"$var/workspaces/T1/repos/r/file.rs"
  echo p >"$var/scratch/pool/x"
  echo bc >"$var/build-cache/x"
  echo web >"$var/web/x"
  echo m >"$var/memory/m"
  echo rb >"$var/release-build/x"
  python3 - "$var/celeris.sqlite3" <<'PY'
import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
con.execute("PRAGMA journal_mode=WAL")
con.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
con.executemany("INSERT INTO t(v) VALUES (?)", [("a",), ("b",), ("c",)])
con.commit()
con.close()
PY
  cat >"$d/home/.config/celeris/config.toml" <<EOF
workspace_root = "$var/workspaces"

[db]
path = "$var/celeris.sqlite3"

[workspace]
build_cache_dir = "$var/build-cache"

[memory]
dir = "$var/memory"

[[providers]]
id = "x"
adapter = "fake"
EOF
  echo token >"$d/home/.config/celeris/api.token"
  printf '[Service]\nExecStart=%%h/.local/celeris/releases/%%i/bin/celeris\n' >"$d/home/.config/systemd/user/celeris@.service"
  printf 'celeris@%s\nceleris-gui@%s\n' "$SHA" "$SHA" >"$d/stub-state/active"
}

# mig <dir> <args...>: 一時環境で台本を走らせる（本番の env は持ち込まない）。
mig() {
  local d="$1"
  shift
  env -u CELERIS_STATE_DIR -u CELERIS_CONFIG_DIR -u CELERIS_CONFIG -u CELERIS_DB -u CELERIS_BACKUPS_DIR \
    -u CELERIS_LOGS_DIR -u SD_UNIT_DIR -u CELERISCTL \
    HOME="$d/home" PATH="$d/bin:$PATH" STUB_STATE="$d/stub-state" \
    MIGRATE_LOCAL_ROOT="$d/local" MIGRATE_OLD_VAR_DIR="$d/var-lib-celeris" \
    MIGRATE_MIN_FREE_GIB=0 MIGRATE_VERIFY_TIMEOUT=3 \
    bash "${MIGRATE_TEST_SCRIPT:-$SCRIPT}" "$@" >>"$d/run.log" 2>&1
}

# 全部（mtime・mode・中身・symlink の先）の写真。
snapshot_all() {
  local d="$1"
  (cd "$d" && find home var-lib-celeris local -printf '%p %y %m %s %T@ %l\n' | sort
    find home var-lib-celeris local -type f -print0 | sort -z | xargs -0 md5sum)
}

# rollback で戻るべきものの写真（記録・.bak・DB family の bytes・ディレクトリの mtime は除く）。
snapshot_restorable() {
  local d="$1"
  (cd "$d" && find home var-lib-celeris \
    -path home/.local/celeris/migrate-to-local -prune -o -name '*.bak-migrate-*' -prune -o \
    -name 'celeris.sqlite3*' -prune -o -type d -printf '%p d %m\n' -o -printf '%p %y %m %s %T@ %l\n' | sort
    find home var-lib-celeris -path home/.local/celeris/migrate-to-local -prune -o -name '*.bak-migrate-*' -prune -o \
      -name 'celeris.sqlite3*' -prune -o -type f -print0 | sort -z | xargs -0 md5sum)
}

db_dump() { python3 -c 'import sqlite3,sys; print("\n".join(sqlite3.connect(sys.argv[1]).iterdump()))' "$1"; }

# ---- 1. dry-run は何も変えない -----------------------------------------------------

A="$WORK/a"
setup "$A"
snapshot_all "$A" >"$WORK/a.before"
for stage in presync gc stop delta switch start verify rollback; do
  if mig "$A" "$stage" --dry-run; then ok "dry-run $stage exits 0"; else ng "dry-run $stage failed"; tail -n 20 "$A/run.log" >&2; fi
done
snapshot_all "$A" >"$WORK/a.after"
if diff -u "$WORK/a.before" "$WORK/a.after" >"$WORK/a.diff"; then ok "dry-run changed nothing (home, var, local)"; else ng "dry-run changed files"; head -n 40 "$WORK/a.diff" >&2; fi
if grep -Eq -- '--user (stop|start|daemon-reload)' "$A/stub-state/systemctl.log"; then ng "dry-run called systemctl stop/start/daemon-reload"; else ok "dry-run only asked systemctl is-active"; fi
if grep -v -- '--dry-run' "$A/stub-state/celerisctl.log" | grep -q .; then ng "dry-run ran celerisctl without --dry-run"; else ok "dry-run ran celerisctl only with --dry-run"; fi
grep -q 'would run: rsync' "$A/run.log" && ok "dry-run printed the rsync plan" || ng "dry-run did not print the rsync plan"

# ---- 2. 全段の本実行 -----------------------------------------------------------------

B="$WORK/b"
setup "$B"
snapshot_restorable "$B" >"$WORK/b.before"
db_dump "$B/var-lib-celeris/celeris.sqlite3" >"$WORK/b.db.before"
for stage in presync gc stop delta switch start verify; do
  if mig "$B" "$stage"; then ok "$stage exits 0"; else ng "$stage failed"; tail -n 30 "$B/run.log" >&2; fi
done
L="$B/local/celeris"
H="$B/home/.local/celeris"
V="$B/var-lib-celeris"
[ "$(readlink "$H/releases")" = "$L/state/releases" ] && ok "home releases is a symlink to /local" || ng "home releases not switched"
[ "$(readlink "$V/workspaces")" = "$L/data/workspaces" ] && ok "var workspaces is a symlink to /local" || ng "var workspaces not switched"
[ -f "$L/data/workspaces/T1/repos/r/file.rs" ] && [ -f "$L/data/scratch/pool/x" ] && [ -f "$L/data/memory/m" ] \
  && ok "var hot data copied" || ng "var hot data missing"
[ -x "$L/state/tools/ldr/bin/ldr" ] && [ -f "$L/state/releases/.build/tree/README" ] && ok "tools and build tree copied" || ng "tools/build tree missing"
[ "$(readlink "$L/state/current")" = "releases/$SHA" ] && [ -d "$L/state/current/" ] && ok "current kept relative and resolves in /local" || ng "current wrong"
[ "$(readlink -f "$L/state/current/gui/node_modules")" = "$(readlink -f "$L/state/releases/.pnpm-prod-cache/k1/node_modules")" ] \
  && ok "gui deps resolve inside /local" || ng "gui deps resolve elsewhere"
[ ! -e "$L/state/releases/$SHA/web/app" ] && ok "web/app not copied" || ng "web/app copied"
for n in claude-accounts codex-accounts build-cache-nfs cache backups; do
  if [ -e "$L/state/$n" ] && [ "$n" != backups ]; then ng "$n copied to /local"; fi
done
[ -z "$(ls -A "$L/state/backups")" ] && ok "old backups stay in home (new backups dir is empty)" || ng "backups copied"
[ -f "$H/build-cache-nfs/blob" ] && [ -f "$H/cache/sccache-l2/blob" ] && [ -f "$H/claude-accounts/cred.json" ] \
  && ok "build-cache-nfs, sccache-l2 and accounts untouched in home" || ng "home cold/config data changed"
[ "$(stat -c %a "$L/state/credentiald")" = 700 ] && ok "credentiald vault is 0700" || ng "credentiald mode wrong"
grep -qxF "$L/data/db" "$B/stub-state/nocow" && ok "chattr +C applied to the db dir" || ng "no chattr +C on the db dir"
[ "$(db_dump "$L/data/db/celeris.sqlite3")" = "$(cat "$WORK/b.db.before")" ] && ok "new db has the same rows" || ng "new db differs"
grep -q "path = \"$L/data/db/celeris.sqlite3\"" "$B/home/.config/celeris/config.toml" \
  && grep -q "releases_dir = \"$L/state/releases\"" "$B/home/.config/celeris/config.toml" \
  && grep -q "workspace_root = \"$L/data/workspaces\"" "$B/home/.config/celeris/config.toml" \
  && ok "config points at /local" || ng "config not rewritten"
grep -qxF "CELERIS_STATE_DIR=$L/state" "$B/home/.config/celeris/paths.env" && ok "paths.env written" || ng "paths.env missing"
grep -q "$L/state/releases" "$B/home/.config/systemd/user/celeris@.service" && ok "unit installed with the /local root" || ng "unit not reinstalled"
grep -qxF "celeris@$SHA" "$B/stub-state/active" && ok "units started again" || ng "units not running after start"

# ---- 3. rollback ---------------------------------------------------------------------

cp "$B/home/.config/celeris/config.toml" "$WORK/b.config.switched"
if mig "$B" rollback; then ng "rollback after start ran without choosing what to do with new writes"; else ok "rollback after start refuses without a choice"; fi
cmp -s "$B/home/.config/celeris/config.toml" "$WORK/b.config.switched" && ok "refused rollback changed nothing" || ng "refused rollback changed config"
if mig "$B" rollback --discard-new-writes; then ok "rollback --discard-new-writes exits 0"; else ng "rollback failed"; tail -n 30 "$B/run.log" >&2; fi
snapshot_restorable "$B" >"$WORK/b.after"
if diff -u "$WORK/b.before" "$WORK/b.after" >"$WORK/b.diff"; then ok "config, paths.env, units and symlinks are back"; else ng "rollback left differences"; head -n 40 "$WORK/b.diff" >&2; fi
[ "$(db_dump "$V/celeris.sqlite3")" = "$(cat "$WORK/b.db.before")" ] && ok "old db is back with its rows" || ng "old db not restored"
[ ! -L "$H/releases" ] && [ -d "$H/releases/$SHA" ] && [ ! -e "$B/home/.config/celeris/paths.env" ] && ok "real dirs restored, paths.env removed" || ng "symlink or paths.env remained"
grep -qxF "celeris@$SHA" "$B/stub-state/active" && ok "old units started after rollback" || ng "old units not started"
[ -f "$L/data/db/celeris.sqlite3" ] && ok "rollback kept the /local copy" || ng "rollback deleted /local data"

# ---- 4. rollback --restore-db-from-new ----------------------------------------------

C="$WORK/c"
setup "$C"
for stage in presync stop delta switch start; do
  mig "$C" "$stage" || { ng "scenario c: $stage failed"; tail -n 30 "$C/run.log" >&2; }
done
python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute("INSERT INTO t(v) VALUES (?)", ("after-switch",)); c.commit(); c.close()' \
  "$C/local/celeris/data/db/celeris.sqlite3"
if mig "$C" rollback --restore-db-from-new; then ok "rollback --restore-db-from-new exits 0"; else ng "restore rollback failed"; tail -n 30 "$C/run.log" >&2; fi
db_dump "$C/var-lib-celeris/celeris.sqlite3" | grep -q "after-switch" && ok "write made after switch is kept in the old db" || ng "new write lost"
grep -q "path = \"$C/var-lib-celeris/celeris.sqlite3\"" "$C/home/.config/celeris/config.toml" && ok "config points back at the old db" || ng "config not restored"

# ---- 5. drop-in directories survive switch → rollback ----------------------------

D="$WORK/d"
setup "$D"
DROPIN="$D/home/.config/systemd/user/celeris-web@$SHA.service.d"
mkdir -p "$DROPIN"
printf '[Service]\nEnvironment=CELERIS_WEB_TEST=before\n' >"$DROPIN/override.conf"
cp -a "$DROPIN" "$WORK/dropin.before"
if mig "$D" presync && mig "$D" stop && mig "$D" delta && mig "$D" switch; then
  ok "drop-in switch exits 0"
else
  ng "drop-in switch failed"
  tail -n 30 "$D/run.log" >&2
fi
if mig "$D" rollback; then ok "drop-in rollback exits 0"; else ng "drop-in rollback failed"; tail -n 30 "$D/run.log" >&2; fi
if diff -r "$WORK/dropin.before" "$DROPIN"; then ok "drop-in directory and contents restored"; else ng "drop-in differs after rollback"; fi

# ---- 6. install-units.sh 失敗でも switch 前へ自動復元 ----------------------------

E="$WORK/e"
setup "$E"
printf 'CELERIS_STATE_DIR=old-value\n' >"$E/home/.config/celeris/paths.env"
E_DROPIN="$E/home/.config/systemd/user/celeris-web@$SHA.service.d"
mkdir -p "$E_DROPIN"
printf '[Service]\nEnvironment=BEFORE=1\n' >"$E_DROPIN/override.conf"
cp -p "$E/home/.config/celeris/config.toml" "$WORK/e.config.before"
cp -p "$E/home/.config/celeris/paths.env" "$WORK/e.paths.before"
cp -a "$E_DROPIN" "$WORK/e.dropin.before"
mkdir -p "$WORK/fail-script"
cp "$SCRIPT" "$HERE/../lib.sh" "$WORK/fail-script/"
cat >"$WORK/fail-script/install-units.sh" <<'EOF'
#!/usr/bin/env bash
printf 'changed\n' >"$SD_UNIT_DIR/celeris@.service"
rm -rf "$SD_UNIT_DIR/celeris-web@aaaaaaaaaaaa.service.d"
exit 1
EOF
if mig "$E" presync && mig "$E" stop && mig "$E" delta; then
  :
else
  ng "switch-fail-install: setup failed"
  tail -n 30 "$E/run.log" >&2
fi
if MIGRATE_TEST_SCRIPT="$WORK/fail-script/migrate-to-local.sh" mig "$E" switch; then
  ng "switch-fail-install: switch unexpectedly succeeded"
else
  if cmp -s "$WORK/e.config.before" "$E/home/.config/celeris/config.toml" \
    && cmp -s "$WORK/e.paths.before" "$E/home/.config/celeris/paths.env" \
    && cmp -s "$WORK/e.dropin.before/override.conf" "$E_DROPIN/override.conf" \
    && grep -q 'ExecStart=%h/.local/celeris' "$E/home/.config/systemd/user/celeris@.service" \
    && [ ! -L "$E/home/.local/celeris/releases" ] \
    && [ -f "$E/home/.local/celeris/releases/$SHA/bin/celerisctl" ] \
    && [ ! -L "$E/var-lib-celeris/workspaces" ] \
    && [ -f "$E/var-lib-celeris/workspaces/T1/repos/r/file.rs" ] \
    && [ -f "$E/var-lib-celeris/celeris.sqlite3" ] \
    && [ ! -e "$E/home/.local/celeris/migrate-to-local/switched" ] \
    && [ ! -e "$E/home/.local/celeris/migrate-to-local/switched-items" ]; then
    ok "switch-fail-install: config, paths.env, units, symlinks and DB restored"
  else
    ng "switch-fail-install: state was not restored"
  fi
fi

# ---- 7. delta 後の新 tree 欠損による swap 失敗でも自動復元 -------------------------

F="$WORK/f"
setup "$F"
cp -p "$F/home/.config/celeris/config.toml" "$WORK/f.config.before"
if mig "$F" presync && mig "$F" stop && mig "$F" delta; then
  :
else
  ng "switch-fail-swap: setup failed"
  tail -n 30 "$F/run.log" >&2
fi
rm -rf "$F/local/celeris/state/tools"
if mig "$F" switch; then
  ng "switch-fail-swap: switch unexpectedly succeeded"
else
  if cmp -s "$WORK/f.config.before" "$F/home/.config/celeris/config.toml" \
    && [ ! -e "$F/home/.config/celeris/paths.env" ] \
    && [ ! -L "$F/home/.local/celeris/releases" ] \
    && [ -f "$F/home/.local/celeris/releases/$SHA/bin/celerisctl" ] \
    && [ ! -L "$F/home/.local/celeris/tools" ] \
    && [ -x "$F/home/.local/celeris/tools/ldr/bin/ldr" ] \
    && [ -f "$F/var-lib-celeris/celeris.sqlite3" ] \
    && [ ! -e "$F/home/.local/celeris/migrate-to-local/switched" ] \
    && [ ! -e "$F/home/.local/celeris/migrate-to-local/switched-items" ]; then
    ok "switch-fail-swap: config, absent paths.env, symlinks and DB restored"
  else
    ng "switch-fail-swap: state was not restored"
  fi
fi

if [ "$FAIL" -ne 0 ]; then
  echo "migrate_to_local_test: FAILED" >&2
  exit 1
fi
echo "migrate_to_local_test: all ok"
