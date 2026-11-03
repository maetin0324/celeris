#!/bin/sh
# bundle を読む本物の host の試験（crates/celeris/tests/script_compare_*）が、古い dist・dist の無い環境で
# 黙って SKIP せず、誤って落ちないことを確かめる（境界 ADR 2026-11-02-ts-control-layer-boundary 付記 S5-9）。引数なしで実行する。
#
# 1. 本物の試験（cargo nextest）を、control-ts の写し（$TMPDIR。git の repo にし、node_modules は作業ツリーへの
#    symlink）に向けて走らせる（CELERIS_SCRIPT_CONTROL_TS_DIR。作業ツリーの control-ts/dist には触れない）。
#    試験は source から $TMPDIR に bundle を作って読み、control-ts/dist は読まない:
#    (a) dist が無い: exit 0、SKIP 0。CELERIS_SCRIPT_REQUIRE_BUNDLE=1 でも exit 0。
#    (b) 偽の古い dist（領域・capability は config と同じ、本文は常に例外を投げる）: exit 0、SKIP 0、REQUIRE=1 でも exit 0。
#    (c) 同じ偽の dist を CELERIS_SCRIPT_BUNDLE_DIR で渡すと試験が落ちる（(b) で偽の dist が読まれていないことの裏付け）。
#    (d) node が動かない（bundle を作れない）: SKIP して exit 0、SKIP は CELERIS_SCRIPT_SKIP_LOG に残る。REQUIRE=1 では非 0。
# 2. scripts/dev/test-affected.sh の結線を、小さな git fixture と偽の cargo・node で確かめる:
#    bundle を使い捨ての dir に作って CELERIS_SCRIPT_BUNDLE_DIR・REQUIRE=1 を cargo に渡す、control-ts/dist を変えない、
#    作った bundle で SKIP したら非 0、bundle に届く変更で作れなければ非 0、届かない変更なら渡さずに進む、
#    celeris・celeris-script を選ばなければ作らない。
set -eu

here="$(cd "$(dirname "$0")/../../.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/script-bundle-fresh.XXXXXX")"
trap 'rm -rf "$work"' EXIT
unset CELERIS_SCRIPT_BUNDLE_DIR CELERIS_SCRIPT_REQUIRE_BUNDLE CELERIS_SCRIPT_SKIP_LOG CELERIS_SCRIPT_CONTROL_TS_DIR
fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok: $*"; }

# ---- 1. 本物の試験 ----
if [ ! -f "$here/control-ts/node_modules/.modules.yaml" ]; then
  pm="corepack pnpm@$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["packageManager"].split("@",1)[1])' "$here/control-ts/package.json")"
  command -v pnpm >/dev/null 2>&1 && pm=pnpm
  (cd "$here/control-ts" && $pm install --offline --frozen-lockfile >/dev/null 2>&1) ||
    fail "control-ts の pnpm install --offline が失敗した"
fi
copy="$work/root"
cts="$copy/control-ts"
mkdir -p "$cts" "$copy/docs/api/script-host"
cp -R "$here/control-ts/src" "$here/control-ts/scripts" "$cts/"
cp "$here/control-ts/bundle.config.json" "$here/control-ts/package.json" "$here/control-ts/pnpm-lock.yaml" "$cts/"
cp "$here/docs/api/script-host/host-v1.schema.json" "$copy/docs/api/script-host/"
ln -s "$here/control-ts/node_modules" "$cts/node_modules"
printf 'node_modules\ndist\n' >"$copy/.gitignore"
(cd "$copy" && git init -q -b main . && git add -A &&
  git -c user.email=t@example.com -c user.name=t commit -q -m copy) || fail "写しを git の repo にできない"

(cd "$here" && cargo nextest run --no-run -p celeris -E 'binary(/^script_compare_/)' >/dev/null 2>&1) ||
  fail "cargo nextest --no-run が失敗した"
real() { # name env...（cargo の exit を返す。出力は $work/<name>.out、SKIP は $work/<name>.skip）
  name="$1"; shift
  : >"$work/$name.skip"
  mkdir -p "$work/tmp-$name"
  set +e
  (cd "$here" && env TMPDIR="$work/tmp-$name" CELERIS_SCRIPT_CONTROL_TS_DIR="$cts" \
    CELERIS_SCRIPT_SKIP_LOG="$work/$name.skip" "$@" \
    cargo nextest run --no-fail-fast -p celeris -E 'binary(/^script_compare_/)' >"$work/$name.out" 2>&1)
  rc=$?
  set -e
  return "$rc"
}
built() { # name（source から作った bundle が $TMPDIR に在る）
  ls "$work/tmp-$1/celeris-script-bundle/"*/manifest.json >/dev/null 2>&1
}

# (a) dist が無い。
real a-missing || fail "(a) dist が無いのに試験が落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/a-missing.out")"
[ ! -s "$work/a-missing.skip" ] || fail "(a) SKIP した: $(head -1 "$work/a-missing.skip")"
built a-missing || fail "(a) source から bundle を作っていない"
pass "(a) dist が無い: source から作った bundle で exit 0、SKIP 0"
real a-missing-req CELERIS_SCRIPT_REQUIRE_BUNDLE=1 ||
  fail "(a) REQUIRE=1 で落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/a-missing-req.out")"
pass "(a) REQUIRE=1 でも exit 0"

# (b) 偽の古い dist。常に例外を投げる本文で、manifest は今の config と同じ領域・capability。
python3 -I - "$cts" "$copy/docs/api/script-host/host-v1.schema.json" <<'PY2'
import hashlib, json, os, sys
cts, schema = sys.argv[1], sys.argv[2]
cfg = json.load(open(os.path.join(cts, "bundle.config.json")))
code = b'globalThis.__celerisDispatch = () => { throw new Error("stale dist"); };\n'
dist = os.path.join(cts, "dist")
os.makedirs(dist)
open(os.path.join(dist, "bundle.mjs"), "wb").write(code)
manifest = {
    "schema": "celeris.script-manifest/1",
    "bundle_sha256": hashlib.sha256(code).hexdigest(),
    "host_api": cfg["host_api"],
    "schema_sha256": hashlib.sha256(open(schema, "rb").read()).hexdigest(),
    "toolchain": {"typescript": "0", "esbuild": "0", "node": "0"},
    "source_commit": "stale",
    "domains": sorted(cfg["domains"]),
    "capabilities": sorted({c for caps in cfg["domains"].values() for c in caps}),
    "checkpoint_versions": cfg["checkpoint_versions"],
}
open(os.path.join(dist, "manifest.json"), "w").write(json.dumps(manifest))
PY2
stale_sum="$(cat "$cts/dist/bundle.mjs" "$cts/dist/manifest.json" | cksum)"
real b-stale || fail "(b) 古い dist で試験が落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/b-stale.out")"
[ ! -s "$work/b-stale.skip" ] || fail "(b) SKIP した: $(head -1 "$work/b-stale.skip")"
real b-stale-req CELERIS_SCRIPT_REQUIRE_BUNDLE=1 ||
  fail "(b) REQUIRE=1 で古い dist で落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/b-stale-req.out")"
[ "$(cat "$cts/dist/bundle.mjs" "$cts/dist/manifest.json" | cksum)" = "$stale_sum" ] || fail "(b) 試験が dist を書き換えた"
pass "(b) 偽の古い dist: exit 0、SKIP 0、REQUIRE=1 でも exit 0、dist は不変"

# (c) 同じ偽の dist を読ませると落ちる（(b) が偽の dist を読んでいないことの裏付け）。
if real c-used CELERIS_SCRIPT_BUNDLE_DIR="$cts/dist"; then fail "(c) 例外を投げる bundle を読ませたのに通った"; fi
grep -q 'stale dist' "$work/c-used.out" || fail "(c) 偽の dist の例外で落ちていない: $(grep -m3 -E 'FAIL|panicked' "$work/c-used.out")"
pass "(c) 偽の dist を CELERIS_SCRIPT_BUNDLE_DIR で読ませると落ちる"

# (d) node が動かない（bundle を作れない）。
nonode="$work/nonode"
mkdir -p "$nonode"
printf '#!/bin/sh\necho "node: disabled by script-bundle-fresh" >&2\nexit 127\n' >"$nonode/node"
chmod +x "$nonode/node"
real d-nonode PATH="$nonode:$PATH" || fail "(d) bundle を作れないのに試験が落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/d-nonode.out")"
grep -q 'build-bundle.mjs failed' "$work/d-nonode.skip" || fail "(d) SKIP の理由が log に無い: $(head -1 "$work/d-nonode.skip")"
pass "(d) bundle を作れない: exit 0、SKIP $(grep -c . "$work/d-nonode.skip") 件を log に記録"
if real d-nonode-req PATH="$nonode:$PATH" CELERIS_SCRIPT_REQUIRE_BUNDLE=1; then fail "(d) REQUIRE=1 で bundle を作れないのに通った"; fi
pass "(d) REQUIRE=1 では失敗"

# ---- 2. test-affected.sh の結線 ----
fx="$work/fx"
mkdir -p "$fx" && cd "$fx"
git init -q -b main .
git config user.email t@example.com
git config user.name t
mkdir -p crates/a/src crates/b/src crates/script-runtime/src control-ts/node_modules control-ts/dist control-ts/scripts scripts/dev
printf '[workspace]\nmembers = ["crates/a", "crates/b", "crates/script-runtime"]\nresolver = "2"\n' >Cargo.toml
mk() { # dir name [dep]
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' "$2" >"crates/$1/Cargo.toml"
  [ -z "${3:-}" ] || printf '%s = { path = "../%s" }\n' "$3" "$3" >>"crates/$1/Cargo.toml"
  echo "" >"crates/$1/src/lib.rs"
}
mk a a
mk b b
mk script-runtime celeris-script a
echo '{"packageManager":"pnpm@1.0.0"}' >control-ts/package.json
echo '' >control-ts/scripts/build-bundle.mjs
printf 'node_modules\ndist\n' >.gitignore
printf '#!/bin/sh\nexit 0\n' >scripts/dev/check-health.sh
printf '#!/bin/sh\necho test-ts-verify >>"$FX_CALLS"\n' >scripts/dev/test-ts-verify.sh
git add -A
git commit -q -m base
base="$(git rev-parse HEAD)"
: >control-ts/node_modules/.modules.yaml
echo 'stale' >control-ts/dist/bundle.mjs
stale_sum="$(cksum control-ts/dist/bundle.mjs)"

bin="$work/bin"
mkdir -p "$bin"
real_cargo="$(command -v cargo)"
cat >"$bin/cargo" <<SH
#!/bin/sh
[ "\$1" = metadata ] && exec "$real_cargo" "\$@"
echo "cargo \$1 dir=\${CELERIS_SCRIPT_BUNDLE_DIR:-} require=\${CELERIS_SCRIPT_REQUIRE_BUNDLE:-}" >>"\$FX_CALLS"
if [ "\$1" = nextest ] && [ -n "\${FX_SKIP:-}" ]; then printf 'some_test\tstale\n' >>"\$CELERIS_SCRIPT_SKIP_LOG"; fi
if [ "\$1" = nextest ] && [ -n "\${CELERIS_SCRIPT_BUNDLE_DIR:-}" ]; then test -f "\$CELERIS_SCRIPT_BUNDLE_DIR/manifest.json" || exit 9; fi
exit 0
SH
cat >"$bin/node" <<'SH'
#!/bin/sh
echo "node out=$TEST_AFFECTED_OUT" >>"$FX_CALLS"
[ -z "${FX_NODE_FAIL:-}" ] || { echo "bundle: boom" >&2; exit 1; }
mkdir -p "$TEST_AFFECTED_OUT"
echo '{}' >"$TEST_AFFECTED_OUT/manifest.json"
echo 'x' >"$TEST_AFFECTED_OUT/bundle.mjs"
SH
chmod +x "$bin/cargo" "$bin/node"
export FX_CALLS="$work/calls"
ta() { # env...（test-affected.sh の exit を返す。stderr は $work/ta.err）
  : >"$FX_CALLS"
  set +e
  env CELERIS_WU_BASE="$base" PATH="$bin:$PATH" "$@" bash "$here/scripts/dev/test-affected.sh" >/dev/null 2>"$work/ta.err"
  rc=$?
  set -e
  return "$rc"
}

# script-runtime（bundle に届く変更）: 作った bundle を渡す。
echo '// x' >>crates/script-runtime/src/lib.rs
ta || fail "(t1) exit $?: $(tail -3 "$work/ta.err")"
grep -q '^node out=/' "$FX_CALLS" || fail "(t1) bundle を作っていない: $(cat "$FX_CALLS")"
grep -q '^cargo nextest dir=/.*script-bundle require=1$' "$FX_CALLS" || fail "(t1) cargo に bundle を渡していない: $(cat "$FX_CALLS")"
grep -q 'control-ts/dist' "$FX_CALLS" && fail "(t1) control-ts/dist を使った: $(cat "$FX_CALLS")"
[ "$(cksum control-ts/dist/bundle.mjs)" = "$stale_sum" ] || fail "(t1) control-ts/dist が変わった"
grep -q 'script bundle SKIP: 0 件' "$work/ta.err" || fail "(t1) SKIP の件数が出ない: $(tail -3 "$work/ta.err")"
pass "(t1) bundle を使い捨ての dir に作って CELERIS_SCRIPT_BUNDLE_DIR・REQUIRE=1 を渡す。control-ts/dist は不変"

# 作った bundle で SKIP した試験があれば失敗。
if ta FX_SKIP=1; then fail "(t2) 作った bundle で SKIP したのに通った"; fi
grep -q 'script bundle SKIP: 1 件' "$work/ta.err" || fail "(t2) SKIP の件数が出ない: $(tail -3 "$work/ta.err")"
pass "(t2) 作った bundle で SKIP 1 件 → 非 0"

# bundle に届く変更で bundle を作れなければ失敗（cargo の試験の前に止まる）。
if ta FX_NODE_FAIL=1; then fail "(t3) bundle を作れないのに通った"; fi
grep -q 'build-bundle.mjs が失敗した' "$work/ta.err" || fail "(t3) 理由が出ない: $(tail -3 "$work/ta.err")"
grep -q '^cargo nextest' "$FX_CALLS" && fail "(t3) cargo の試験を走らせた"
pass "(t3) bundle に届く変更で作れない → 非 0"
git checkout -q -- .

# bundle に届かない変更（a。celeris-script は逆依存で選ばれる）で作れなければ、渡さずに進む。SKIP は数えるが落とさない。
echo '// x' >>crates/a/src/lib.rs
ta FX_NODE_FAIL=1 FX_SKIP=1 || fail "(t4) exit $?: $(tail -3 "$work/ta.err")"
grep -q '^cargo nextest dir= require=$' "$FX_CALLS" || fail "(t4) 渡していないはずの変数がある: $(cat "$FX_CALLS")"
grep -q 'CELERIS_SCRIPT_BUNDLE_DIR を渡さない' "$work/ta.err" || fail "(t4) 理由が出ない: $(tail -3 "$work/ta.err")"
grep -q 'script bundle SKIP: 1 件' "$work/ta.err" || fail "(t4) SKIP の件数が出ない"
pass "(t4) bundle に届かない変更で作れない → 渡さずに進む（SKIP 1 件を報告）"
git checkout -q -- .

# celeris・celeris-script を選ばない変更では作らない。
echo '// x' >>crates/b/src/lib.rs
ta || fail "(t5) exit $?: $(tail -3 "$work/ta.err")"
grep -q '^node' "$FX_CALLS" && fail "(t5) bundle を作った: $(cat "$FX_CALLS")"
grep -q 'script bundle SKIP' "$work/ta.err" && fail "(t5) SKIP を数えた"
pass "(t5) celeris・celeris-script を選ばなければ作らない"
git checkout -q -- .

echo "script-bundle-fresh: all ok"
