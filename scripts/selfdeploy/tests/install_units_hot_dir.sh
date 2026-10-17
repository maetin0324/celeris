#!/usr/bin/env bash
# scripts/selfdeploy/tests/install_units_hot_dir.sh — ADR-0136「path の契約」:
#
#   install-units.sh が hot の根（CELERIS_STATE_DIR、または ~/.config/celeris/paths.env）を指定されると、
#   celeris@・celeris-web@・celeris-credentiald@ の unit に
#   `%h/.local/celeris` を残さず根の絶対 path から読むこと、`~/.config/celeris` の参照は home のまま、
#   未指定なら従来と同一（テンプレートと byte 一致）であることを確かめる。
#
# 本番には触れない: HOME・CELERIS_CONFIG_DIR・SD_UNIT_DIR は一時ディレクトリ、`systemctl` は PATH 先頭の偽物。
# 実行: bash scripts/selfdeploy/tests/install_units_hot_dir.sh（引数なし）
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SD="$(cd "$HERE/.." && pwd)"
SRC="$(cd "$SD/../.." && pwd)/deploy/systemd"

FAIL=0
ok() { echo "ok: $*"; }
ng() { echo "FAIL: $*" >&2; FAIL=1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FAKE_BIN="$WORK/fakebin"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_LOG"
exit 0
EOF
chmod +x "$FAKE_BIN/systemctl"

# 1 回分の環境で install-units.sh を走らせる。$1 = 名前、残りは env に渡す代入。
run_install() {
  local name="$1"; shift
  local home="$WORK/$name/home"
  mkdir -p "$home/.config/celeris" "$WORK/$name/units"
  env -u CELERIS_STATE_DIR -u CELERIS_CONFIG_DIR -u CELERIS_CONFIG -u SD_UNIT_DIR \
    HOME="$home" PATH="$FAKE_BIN:$PATH" FAKE_LOG="$WORK/$name/systemctl.log" \
    SD_UNIT_DIR="$WORK/$name/units" "$@" \
    bash "$SD/install-units.sh" ${INSTALL_ARGS:-} >"$WORK/$name/out.log" 2>&1
}

TEMPLATED="celeris-credentiald@.service celeris@.service celeris-web@.service celeris-web-lan.socket celeris-web-lan.service"
HOT_UNITS="celeris@.service celeris-web@.service celeris-credentiald@.service"

# ---- 1. 未指定: 従来と同一 ---------------------------------------------------
if run_install default; then ok "default: install-units.sh exit 0"; else ng "default: exit non-zero: $(cat "$WORK/default/out.log")"; fi
for u in $TEMPLATED; do
  if cmp -s "$SRC/$u" "$WORK/default/units/$u"; then ok "default: $u identical to the template"; else ng "default: $u differs from the template"; fi
done
for u in celeris-sccache.service celeris-scratch-cache.service; do
  if [ -e "$WORK/default/units/$u" ]; then ng "default: obsolete $u installed"; else ok "default: obsolete $u absent"; fi
done
if grep -q "daemon-reload" "$WORK/default/systemctl.log"; then ok "default: daemon-reload via the stub"; else ng "default: no daemon-reload"; fi

# LAN socket の雛形は FreeBind=yes を持つ（起動時に address 未割り当てでも落ちない）。
if grep -qx 'FreeBind=yes' "$SRC/celeris-web-lan.socket" && grep -qx 'FreeBind=yes' "$WORK/default/units/celeris-web-lan.socket"; then
  ok "celeris-web-lan.socket has FreeBind=yes"
else
  ng "celeris-web-lan.socket lacks FreeBind=yes"
fi

# 根が従来の場所そのものなら、指定なしと同じ。
if run_install same CELERIS_STATE_DIR="$WORK/same/home/.local/celeris"; then :; else ng "same: exit non-zero"; fi
for u in $TEMPLATED; do
  cmp -s "$SRC/$u" "$WORK/same/units/$u" || ng "same: $u differs although the root is the old place"
done
ok "same-as-default root: templates unchanged"

# ---- 2. 指定あり（環境変数）と paths.env -----------------------------------
check_hot() {
  local name="$1" root="$2" u f
  for u in $HOT_UNITS; do
    f="$WORK/$name/units/$u"
    [ -f "$f" ] || { ng "$name: $u not installed"; continue; }
    if grep -q '%h/\.local/celeris' "$f"; then ng "$name: $u still has %h/.local/celeris"; else ok "$name: $u has no %h/.local/celeris"; fi
    grep -qx "Environment=CELERIS_STATE_DIR=$root" "$f" || ng "$name: $u lacks CELERIS_STATE_DIR=$root"
    grep -qx 'EnvironmentFile=-%h/.config/celeris/paths.env' "$f" || ng "$name: $u lacks EnvironmentFile paths.env"
    grep -qx "AssertPathIsDirectory=$root" "$f" || ng "$name: $u lacks AssertPathIsDirectory"
    # ~/.config/celeris の参照（config・token・password・secret）はテンプレートと同じ行のまま。
    if [ "$(grep -o '%h/\.config/celeris/[^ ]*' "$SRC/$u" | sort)" = \
         "$(grep -o '%h/\.config/celeris/[^ ]*' "$f" | grep -v 'paths.env$' | sort)" ]; then
      ok "$name: $u keeps the ~/.config/celeris references"
    else
      ng "$name: $u changed a ~/.config/celeris reference"
    fi
  done
  f="$WORK/$name/units/celeris@.service"
  grep -qx "WorkingDirectory=$root" "$f" || ng "$name: celeris@ WorkingDirectory is not $root"
  grep -q "^ExecStart=$root/releases/%i/bin/celeris --config %h/.config/celeris/config.toml " "$f" ||
    ng "$name: celeris@ ExecStart does not read $root/releases"
  grep -q "exec $root/releases/%i/bin/celeris-credentiald serve" "$WORK/$name/units/celeris-credentiald@.service" ||
    ng "$name: credentiald ExecStart does not read $root/releases"
  grep -qx "Environment=CELERIS_CREDENTIALD_DATA_DIR=$root/credentiald" "$WORK/$name/units/celeris-credentiald@.service" ||
    ng "$name: credentiald data dir is not $root/credentiald"
  grep -qx "WorkingDirectory=$root/releases/%i/web/app" "$WORK/$name/units/celeris-web@.service" ||
    ng "$name: web WorkingDirectory does not read the hot release"

}

ROOT=/local/celeris/"state"
if run_install envroot CELERIS_STATE_DIR="$ROOT/"; then ok "env root: exit 0"; else ng "env root: exit non-zero: $(cat "$WORK/envroot/out.log")"; fi
check_hot envroot "$ROOT"

mkdir -p "$WORK/pathsenv/home/.config/celeris"
printf '# hot root\nCELERIS_STATE_DIR="%s"\n' "$ROOT" >"$WORK/pathsenv/home/.config/celeris/paths.env"
if run_install pathsenv; then ok "paths.env root: exit 0"; else ng "paths.env root: exit non-zero: $(cat "$WORK/pathsenv/out.log")"; fi
check_hot pathsenv "$ROOT"

# 既存の unit と違えば控えを残す（従来の動作）。
if ls "$WORK/pathsenv/units/" | grep -q '\.bak-'; then ng "pathsenv: unexpected .bak on a fresh dir"; fi
mkdir -p "$WORK/upgrade/units"
cp "$SRC/celeris@.service" "$WORK/upgrade/units/celeris@.service"
# 最後の credentiald unit だけの違いでも他 unit を書き換えず、reload もしない。
cp "$SRC/celeris-credentiald@.service" "$WORK/upgrade/units/celeris-credentiald@.service"
sed -i 's/serve --config %h\/.config\/celeris\/config.toml/serve --launcher-uid=995/' "$WORK/upgrade/units/celeris-credentiald@.service"
cp "$WORK/upgrade/units/celeris-credentiald@.service" "$WORK/upgrade/credentiald-before"
if run_install upgrade CELERIS_STATE_DIR="$ROOT"; then ng "upgrade: unreviewed replacement accepted"; else ok "upgrade: diff requires review"; fi
cmp -s "$SRC/celeris@.service" "$WORK/upgrade/units/celeris@.service" || ng "upgrade: partial write before refusal"
cmp -s "$WORK/upgrade/credentiald-before" "$WORK/upgrade/units/celeris-credentiald@.service" || ng "upgrade: lost manual launcher UID"
[ ! -e "$WORK/upgrade/systemctl.log" ] || ng "upgrade: systemctl called before review"
[ ! -e "$WORK/upgrade/units/celeris-web@.service" ] || ng "upgrade: new unit written before review"
if ls "$WORK/upgrade/units/" | grep -q '\.bak-'; then ng "upgrade: backup written before review"; fi
grep -q -- '--launcher-uid=995' "$WORK/upgrade/out.log" || ng "upgrade: missing removed argument in diff"
INSTALL_ARGS=--replace-existing run_install upgrade CELERIS_STATE_DIR="$ROOT" || ng "upgrade: reviewed replacement failed"
grep -q 'serve --config %h/.config/celeris/config.toml' "$WORK/upgrade/units/celeris-credentiald@.service" || ng "upgrade: broker does not read daemon config"
if grep -q -- '--launcher-uid' "$WORK/upgrade/units/celeris-credentiald@.service"; then ng "upgrade: old UID argument survived"; fi
cmp -s "$WORK/upgrade/credentiald-before" "$(echo "$WORK/upgrade/units/"celeris-credentiald@.service.bak-*)" || ng "upgrade: old broker unit backup differs"
run_install upgrade CELERIS_STATE_DIR="$ROOT" || ng "upgrade: identical units require review again"
if ls "$WORK/upgrade/units/" | grep -q '^celeris@\.service\.bak-'; then ok "upgrade: kept the old celeris@ as .bak"; else ng "upgrade: no .bak for the old celeris@"; fi

# mask/symlink は通常の雛形との差分として扱わず、明示的な置換でも止める。
mkdir -p "$WORK/masked/units"
ln -s /dev/null "$WORK/masked/units/celeris-credentiald@.service"
if INSTALL_ARGS=--replace-existing run_install masked; then ng "masked unit overwritten"; else ok "masked unit requires manual review"; fi
[ "$(readlink "$WORK/masked/units/celeris-credentiald@.service")" = /dev/null ] || ng "mask lost"
[ ! -e "$WORK/masked/units/celeris@.service" ] || ng "masked: partial install"
[ ! -e "$WORK/masked/systemctl.log" ] || ng "masked: systemctl called"

# ---- 3. 受け付けない根 ---------------------------------------------------------
if run_install relative CELERIS_STATE_DIR=local/celeris; then ng "relative root accepted"; else ok "relative root rejected"; fi
if run_install percent CELERIS_STATE_DIR=/local/50%; then ng "root with % accepted"; else ok "root with % rejected"; fi

if [ "$FAIL" -ne 0 ]; then
  echo "install_units_hot_dir: FAIL" >&2
  exit 1
fi
echo "install_units_hot_dir: ok"
