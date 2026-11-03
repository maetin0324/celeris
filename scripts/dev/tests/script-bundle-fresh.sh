#!/bin/sh
# bundle を読む本物の host の試験（crates/celeris/tests/script_compare_*）が、古い dist・dist の無い環境で
# 黙って誤らないことを確かめる（境界 ADR 2026-11-02-ts-control-layer-boundary 付記 S5-9）。引数なしで実行する。
#
# 1. 本物の試験（cargo nextest）を、control-ts の source の写し（$TMPDIR）に向けて走らせる
#    （CELERIS_SCRIPT_CONTROL_TS_DIR。作業ツリーの control-ts/dist には触れない）:
#    (a) dist が無い: SKIP して exit 0、SKIP は CELERIS_SCRIPT_SKIP_LOG に残る。CELERIS_SCRIPT_REQUIRE_BUNDLE=1 では非 0。
#    (b) 偽の古い dist（領域・capability は config と同じ、本文は常に例外を投げる、inputs.sha256 は source と合わない）:
#        SKIP して exit 0（古い本文で落ちない）。REQUIRE=1 では非 0。
#    (c) 同じ偽の dist の inputs.sha256 を source の値にする: 古さの判定を通るので使われ、試験が落ちる
#        （(b) の SKIP が inputs.sha256 の照合によることを確かめる）。
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
cts="$work/control-ts"
mkdir -p "$cts"
cp -R "$here/control-ts/src" "$cts/src"
cp "$here/control-ts/bundle.config.json" "$here/control-ts/package.json" "$here/control-ts/pnpm-lock.yaml" "$cts/"

(cd "$here" && cargo nextest run --no-run -p celeris -E 'binary(/^script_compare_/)' >/dev/null 2>&1) ||
  fail "cargo nextest --no-run が失敗した"
real() { # name env...（cargo の exit を返す。出力は $work/<name>.out）
  name="$1"; shift
  : >"$work/$name.skip"
  set +e
  (cd "$here" && env CELERIS_SCRIPT_CONTROL_TS_DIR="$cts" CELERIS_SCRIPT_SKIP_LOG="$work/$name.skip" "$@" \
    cargo nextest run --no-fail-fast -p celeris -E 'binary(/^script_compare_/)' >"$work/$name.out" 2>&1)
  rc=$?
  set -e
  return "$rc"
}

# (a) dist が無い。
real a-missing || fail "(a) dist が無いのに試験が落ちた: $(tail -5 "$work/a-missing.out")"
[ -s "$work/a-missing.skip" ] || fail "(a) SKIP が log に残っていない"
grep -q 'control-ts/dist: .*manifest.json' "$work/a-missing.skip" || fail "(a) SKIP の理由が無い: $(head -1 "$work/a-missing.skip")"
pass "(a) dist が無い: exit 0、SKIP $(grep -c . "$work/a-missing.skip") 件を log に記録"
if real a-missing-req CELERIS_SCRIPT_REQUIRE_BUNDLE=1; then fail "(a) REQUIRE=1 で dist が無いのに通った"; fi
pass "(a) REQUIRE=1 では失敗"

# (b) 偽の古い dist。常に例外を投げる本文で、manifest は今の config と同じ領域・capability。
python3 -I - "$cts" "$here/docs/api/script-host/host-v1.schema.json" <<'PY'
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
open(os.path.join(dist, "inputs.sha256"), "w").write("0" * 64 + "\n")
PY
real b-stale || fail "(b) 古い dist で試験が落ちた: $(grep -m3 -E 'FAIL|panicked' "$work/b-stale.out")"
grep -q 'stale bundle: inputs.sha256' "$work/b-stale.skip" || fail "(b) inputs.sha256 の照合で SKIP していない: $(head -1 "$work/b-stale.skip")"
pass "(b) 古い dist: exit 0、SKIP $(grep -c . "$work/b-stale.skip") 件（inputs.sha256 が合わない）"
if real b-stale-req CELERIS_SCRIPT_REQUIRE_BUNDLE=1; then fail "(b) REQUIRE=1 で古い dist なのに通った"; fi
pass "(b) REQUIRE=1 では失敗"

# (c) inputs.sha256 だけを source の値にすると、例外を投げる本文が使われて落ちる。
python3 -I - "$cts" >"$cts/dist/inputs.sha256" <<'PY'
import hashlib, os, sys
root = sys.argv[1]
files = []
for d, _, names in os.walk(os.path.join(root, "src")):
    for n in names:
        p = os.path.join(d, n)
        if os.path.isfile(p) and not os.path.islink(p):
            files.append(os.path.relpath(p, root).replace(os.sep, "/"))
files += ["bundle.config.json", "package.json", "pnpm-lock.yaml"]
text = "".join("%s  %s\n" % (hashlib.sha256(open(os.path.join(root, f), "rb").read()).hexdigest(), f) for f in sorted(files))
print(hashlib.sha256(text.encode()).hexdigest())
PY
if real c-used; then fail "(c) 例外を投げる bundle が使われたのに通った"; fi
[ ! -s "$work/c-used.skip" ] || fail "(c) inputs.sha256 が合うのに SKIP した: $(head -1 "$work/c-used.skip")"
pass "(c) inputs.sha256 が合う dist は使われる（例外を投げる本文で失敗、SKIP 0）"

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
