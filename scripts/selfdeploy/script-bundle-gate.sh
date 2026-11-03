#!/usr/bin/env bash
# release の gate の TS 制御層の段（境界 ADR 2026-11-02 (d)(e)、ADR 2026-11-03-release-script-bundle-gate）。
# release.sh が段ごとに呼ぶ。どの段も「作れない・欠けた・古い・hash が合わない」を SKIP にせず非 0 で落とす。
#
#   script-bundle-gate.sh install <control-ts>
#       `pnpm install --offline --frozen-lockfile`（pnpm は packageManager の版を corepack で。SD_CONTROL_TS_PNPM で差し替え）。
#       node_modules/.modules.yaml と .bin の shim（esbuild・tsc・vitest）が揃わなければ失敗。
#   script-bundle-gate.sh bundle <control-ts>
#       `node scripts/build-bundle.mjs` で dist/bundle.mjs・dist/manifest.json を作る（`pnpm run` を通さない）。
#       前後で node_modules/.bin の shim の sha256 と、control-ts の追跡 file の状態を比べ、変わったら失敗（S4 の副作用の防止）。
#   script-bundle-gate.sh verify <repo-root> <bundle-dir> <expected-commit>
#       dist の 2 file を検証する: 在る・manifest の schema・bundle_sha256 = bundle の sha256・schema_sha256 = 木の
#       host-v1.schema.json の sha256・host_api が `^<Host API の major>`（crates/script-runtime/src/dto.rs）・
#       domains = bundle.config.json の key（Rust の KNOWN_DOMAINS の内、host_selftest を含む）・capabilities =
#       宣言の和・source_commit = <expected-commit>（`-dirty` や別 commit は古い bundle として落とす）・
#       ESM として構文が通り（node --check）、static import は celeris:host だけ。
#   script-bundle-gate.sh test <repo-root> <bundle-dir>
#       CELERIS_SCRIPT_REQUIRE_BUNDLE=1 と CELERIS_SCRIPT_BUNDLE_DIR=<bundle-dir> が渡されていなければ失敗。
#       script-load-check（登録と同じ manifest の検証と selftest の 1 回の呼び出し）を走らせ、続けて script の実 bundle
#       試験（celeris の tests/script_*、celeris-script の tests/generation）を nextest で回す。SKIP が 1 件でもあれば失敗。
#   script-bundle-gate.sh verify-release <release-dir>
#       verify.sh が使う。release の gate.json に上の 4 段が skip されずに exit 0 で載っていること、release の
#       script-bundle/ の bundle の sha256 が manifest と gate.json の script_bundle.bundle_sha256 に一致することを見る。
#
# exit: 0 = 通った、1 = 検査の失敗、2 = 使い方の誤り。本番には触れない（読むのは引数の dir だけ）。
set -euo pipefail

usage() {
  sed -n '3,24p' "${BASH_SOURCE[0]}" >&2
  exit 2
}
die() { echo "script-bundle-gate: $*" >&2; exit 1; }

SHIMS="esbuild tsc vitest"

# `shim_state <control-ts>`: shim ごとに「名前 sha256」を 1 行ずつ。欠けた shim は「名前 missing」。
shim_state() {
  local n
  for n in $SHIMS; do
    if [ -f "$1/node_modules/.bin/$n" ]; then
      printf '%s %s\n' "$n" "$(sha256sum "$1/node_modules/.bin/$n" | cut -d' ' -f1)"
    else
      printf '%s missing\n' "$n"
    fi
  done
}

control_ts_pnpm() {
  local cts="$1" ver
  shift
  if [ -n "${SD_CONTROL_TS_PNPM:-}" ]; then
    # shellcheck disable=SC2086
    (cd "$cts" && $SD_CONTROL_TS_PNPM "$@")
    return
  fi
  ver="$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["packageManager"].split("@",1)[1])' "$cts/package.json")" \
    || die "control-ts/package.json has no packageManager pnpm@<version>"
  command -v corepack >/dev/null 2>&1 || die "corepack not found in PATH (control-ts needs corepack pnpm@$ver)"
  (cd "$cts" && corepack "pnpm@$ver" "$@")
}

cmd_install() {
  local cts="$1"
  [ -f "$cts/package.json" ] && [ -f "$cts/pnpm-lock.yaml" ] || die "$cts has no package.json/pnpm-lock.yaml"
  control_ts_pnpm "$cts" install --offline --frozen-lockfile || die "pnpm install --offline --frozen-lockfile failed in $cts"
  [ -f "$cts/node_modules/.modules.yaml" ] || die "install did not create $cts/node_modules/.modules.yaml"
  if shim_state "$cts" | grep -q ' missing$'; then
    die "install did not create the shims: $(shim_state "$cts" | grep ' missing$' | cut -d' ' -f1 | tr '\n' ' ')"
  fi
  echo "script-bundle-gate: install ok ($cts)"
}

cmd_bundle() {
  local cts="$1" before after tracked_before tracked_after
  [ -f "$cts/scripts/build-bundle.mjs" ] || die "$cts/scripts/build-bundle.mjs not found"
  before="$(shim_state "$cts")"
  tracked_before="$(git -C "$cts" status --porcelain --untracked-files=no -- . 2>/dev/null || true)"
  rm -f "$cts/dist/bundle.mjs" "$cts/dist/manifest.json"
  (cd "$cts" && node scripts/build-bundle.mjs) || die "node scripts/build-bundle.mjs failed"
  after="$(shim_state "$cts")"
  if [ "$before" != "$after" ]; then
    printf 'before:\n%s\nafter:\n%s\n' "$before" "$after" >&2
    die "bundle changed node_modules/.bin shims (side effect; see KB control-ts-bundle-node-bin-side-effect)"
  fi
  tracked_after="$(git -C "$cts" status --porcelain --untracked-files=no -- . 2>/dev/null || true)"
  [ "$tracked_before" = "$tracked_after" ] || die "bundle changed tracked files under $cts: $tracked_after"
  echo "script-bundle-gate: bundle ok; shims unchanged: $(printf '%s' "$after" | tr '\n' ' ')"
}

cmd_verify() {
  local root="$1" dir="$2" expected="$3"
  [ -f "$dir/bundle.mjs" ] || die "bundle missing: $dir/bundle.mjs"
  [ -f "$dir/manifest.json" ] || die "manifest missing: $dir/manifest.json"
  python3 -I - "$root" "$dir" "$expected" <<'PY'
import hashlib, json, re, sys
root, d, expected = sys.argv[1:4]
def fail(msg):
    print(f"script-bundle-gate: verify: {msg}", file=sys.stderr)
    sys.exit(1)
sha = lambda b: hashlib.sha256(b).hexdigest()
bundle = open(f"{d}/bundle.mjs", "rb").read()
try:
    m = json.load(open(f"{d}/manifest.json", encoding="utf-8"))
except ValueError as e:
    fail(f"manifest.json is not JSON: {e}")
if m.get("schema") != "celeris.script-manifest/1":
    fail(f"manifest schema is {m.get('schema')!r}")
if m.get("bundle_sha256") != sha(bundle):
    fail(f"bundle_sha256 {m.get('bundle_sha256')} != sha256(bundle.mjs) {sha(bundle)}")
schema = open(f"{root}/docs/api/script-host/host-v1.schema.json", "rb").read()
if m.get("schema_sha256") != sha(schema):
    fail(f"schema_sha256 {m.get('schema_sha256')} != sha256(host-v1.schema.json) {sha(schema)} (stale bundle)")
dto = open(f"{root}/crates/script-runtime/src/dto.rs", encoding="utf-8").read()
v = re.search(r'pub const HOST_API_VERSION: &str = "(\d+)\.\d+\.\d+";', dto)
if not v:
    fail("cannot read HOST_API_VERSION from crates/script-runtime/src/dto.rs")
if m.get("host_api") != f"^{v.group(1)}":
    fail(f"host_api {m.get('host_api')!r} does not match Host API v{v.group(1)} (HOST_API_VERSION major {v.group(1)})")
gen = open(f"{root}/crates/script-runtime/src/generation.rs", encoding="utf-8").read()
k = re.search(r"pub const KNOWN_DOMAINS: &\[&str\] = &\[(.*?)\];", gen, re.S)
if not k:
    fail("cannot read KNOWN_DOMAINS from crates/script-runtime/src/generation.rs")
known = set(re.findall(r'"([a-z_]+)"', k.group(1)))
cfg = json.load(open(f"{root}/control-ts/bundle.config.json", encoding="utf-8"))
want = sorted(cfg["domains"])
if m.get("domains") != want:
    fail(f"manifest domains {m.get('domains')} != bundle.config.json domains {want} (stale bundle)")
if not set(want) <= known:
    fail(f"domains {sorted(set(want) - known)} are not in KNOWN_DOMAINS {sorted(known)}")
if "host_selftest" not in want:
    fail("manifest does not declare host_selftest")
caps = sorted({c for cs in cfg["domains"].values() for c in cs})
if sorted(m.get("capabilities", [])) != caps:
    fail(f"manifest capabilities {m.get('capabilities')} != declared {caps}")
if m.get("source_commit") != expected:
    fail(f"source_commit {m.get('source_commit')!r} != expected {expected!r} (stale or dirty bundle)")
text = bundle.decode("utf-8")
imports = re.findall(r'^\s*import\s[^;]*?from\s*["\']([^"\']+)["\']', text, re.M)
imports += re.findall(r'^\s*import\s*["\']([^"\']+)["\']', text, re.M)
stray = sorted(set(i for i in imports if i != "celeris:host"))
if stray:
    fail(f"bundle imports other than celeris:host: {stray}")
if re.search(r'\brequire\s*\(', text):
    fail("bundle calls require() (not an ESM bundle)")
print(json.dumps({"bundle_sha256": m["bundle_sha256"], "host_api": m["host_api"], "domains": m["domains"],
                  "source_commit": m["source_commit"], "toolchain": m.get("toolchain")}))
PY
  # ESM としての構文（実行はしない）。
  node --check "$dir/bundle.mjs" || die "node --check failed: $dir/bundle.mjs is not a valid ES module"
  echo "script-bundle-gate: verify ok ($dir)"
}

cmd_test() {
  local root="$1" dir="$2" skip_log skips
  [ "${CELERIS_SCRIPT_REQUIRE_BUNDLE:-}" = 1 ] || die "CELERIS_SCRIPT_REQUIRE_BUNDLE=1 is required"
  [ "${CELERIS_SCRIPT_BUNDLE_DIR:-}" = "$dir" ] || die "CELERIS_SCRIPT_BUNDLE_DIR must be $dir (got '${CELERIS_SCRIPT_BUNDLE_DIR:-}')"
  [ -f "$dir/bundle.mjs" ] && [ -f "$dir/manifest.json" ] || die "bundle missing in $dir"
  skip_log="$(mktemp "${TMPDIR:-/tmp}/script-gate-skip.XXXXXX")"
  trap 'rm -f "$skip_log"' RETURN
  export CELERIS_SCRIPT_SKIP_LOG="$skip_log"
  (cd "$root" && cargo run --locked -q -p celeris-script --bin script-load-check -- --bundle-dir "$dir") \
    || die "script-load-check rejected $dir"
  (cd "$root" && cargo nextest run --locked -p celeris -p celeris-script \
      -E '(package(celeris) & binary(/^script_/)) | (package(celeris-script) & binary(=generation))') \
    || die "script real-bundle tests failed"
  skips="$(grep -c . "$skip_log" || true)"
  if [ "$skips" -gt 0 ]; then
    sed 's/^/SKIP /' "$skip_log" >&2
    die "$skips script test(s) skipped the bundle; the release gate does not accept SKIP"
  fi
  echo "script-bundle-gate: test ok (SKIP 0)"
}

cmd_verify_release() {
  local rel="$1"
  python3 -I - "$rel" <<'PY'
import hashlib, json, sys
rel = sys.argv[1]
def fail(msg):
    print(f"script-bundle-gate: verify-release: {msg}", file=sys.stderr)
    sys.exit(1)
try:
    g = json.load(open(f"{rel}/gate.json", encoding="utf-8"))
except (OSError, ValueError) as e:
    fail(f"cannot read gate.json: {e}")
steps = {s.get("step"): s for s in g.get("steps", [])}
for name in ("control-ts-install", "control-ts-bundle", "script-bundle-verify", "script-bundle-tests"):
    s = steps.get(name)
    if s is None:
        fail(f"gate.json has no step {name} (release built without the script bundle gate)")
    if s.get("skipped") or s.get("exit") != 0:
        fail(f"gate step {name} did not pass (exit={s.get('exit')}, skipped={s.get('skipped')})")
sb = g.get("script_bundle") or {}
try:
    bundle = open(f"{rel}/script-bundle/bundle.mjs", "rb").read()
    m = json.load(open(f"{rel}/script-bundle/manifest.json", encoding="utf-8"))
except (OSError, ValueError) as e:
    fail(f"release has no usable script-bundle/: {e}")
actual = hashlib.sha256(bundle).hexdigest()
if not (actual == m.get("bundle_sha256") == sb.get("bundle_sha256")):
    fail(f"bundle sha256 {actual} / manifest {m.get('bundle_sha256')} / gate.json {sb.get('bundle_sha256')} differ")
print(f"script-bundle-gate: verify-release ok (bundle_sha256 {actual})")
PY
}

[ $# -ge 1 ] || usage
sub="$1"
shift
case "$sub" in
  install) [ $# -eq 1 ] || usage; cmd_install "$1" ;;
  bundle) [ $# -eq 1 ] || usage; cmd_bundle "$1" ;;
  verify) [ $# -eq 3 ] || usage; cmd_verify "$1" "$2" "$3" ;;
  test) [ $# -eq 2 ] || usage; cmd_test "$1" "$2" ;;
  verify-release) [ $# -eq 1 ] || usage; cmd_verify_release "$1" ;;
  *) usage ;;
esac
