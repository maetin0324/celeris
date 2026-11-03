#!/usr/bin/env bash
# 検証専用の script-host-runner と Rust golden の cache（ADR 2026-11-02-ts-fast-verify (4)）。
#   bash scripts/dev/script-runner-cache.sh key      cache key（64 hex）を出す。cargo を起動しない
#   bash scripts/dev/script-runner-cache.sh inputs   key の元の文字列（6 入力）を出す。cargo を起動しない
#   bash scripts/dev/script-runner-cache.sh path     cache が当たれば runner の path を出して exit 0、無ければ exit 1。
#                                                    golden は同じ dir の golden/<domain>/<file>.jsonl。cargo を起動しない
#   bash scripts/dev/script-runner-cache.sh ensure   key の dir が無い（照合に落ちた）ときだけ cargo で runner を build し、
#                                                    golden を task-core の example で作り直して control-ts/fixtures と cmp し、
#                                                    example で作らない golden（GOLDEN_COPIED）は control-ts/fixtures から写す。
#                                                    最後に runner の path を出す
# key = 次の行の sha256: host の source（crates/script-runtime/・golden の元の crate・ルート Cargo.toml）の hash、
# Cargo.lock の hash、features（profile・bin・features・--locked）、`rustc -vV` の全行、host-v1.schema.json の hash、
# GOLDEN_COPIED の各 file の hash。
# 置き場所: ${CELERIS_SCRIPT_RUNNER_CACHE:-${CELERIS_SCRATCH_DIR:-/local/celeris/data/scratch}/script-runner-cache}/<key>/
# （git の作業ツリーの中・/tmp の下は exit 2 で拒否）。
# exit: 0 = 成功、1 = cache が無い（path）・build・golden の不一致（ensure）、2 = 使い方・置き場所の誤り。
# この script は検証の道具で、daemon・celerisctl の経路に入らない。
set -euo pipefail

usage() { echo "usage: script-runner-cache.sh key|inputs|path|ensure" >&2; exit 2; }
[ "$#" -eq 1 ] || usage
cmd="$1"
case "$cmd" in key|inputs|path|ensure) ;; *) usage ;; esac

top="$(git rev-parse --show-toplevel)"
cd "$top"

KEY_VERSION="celeris.script-runner-key/1"
# golden の元の crate（fixture を書く example の crate と、その workspace 内の通常依存の閉包）。
# ensure が cargo metadata で閉包を計算し、この一覧と違えば失敗する。
GOLDEN_CRATES="task-core"
HOST_DIRS="crates/script-runtime crates/task-core"
FEATURES="profile=release bin=script-host-runner features=default locked=1"
SCHEMA="docs/api/script-host/host-v1.schema.json"
# golden の file と、それを stdout に書く task-core の example。
GOLDEN_SPECS="lane_rules/all-features.jsonl:dump-lane-fixtures route_priority/cases.jsonl:dump-route-priority-fixtures cos_digest/cases.jsonl:dump-cos-digest-fixtures"
# example で作らない golden。pass_traces.jsonl は task-dispatch の試験（cos_digest_trace.rs）が書き、その試験が byte 一致を確かめる。
# task-dispatch を golden の元の crate にすると key の source が広がるので、fixture の内容を key に入れて golden/ へ写す
# （ADR 2026-11-02-ts-fast-verify 付記 grade-cos-digest）。
GOLDEN_COPIED="cos_digest/pass_traces.jsonl"
KEEP=4

sha_file() { # path → sha256 か absent
  if [ -f "$1" ]; then sha256sum <"$1" | cut -d' ' -f1; else echo absent; fi
}

host_src_hash() {
  # 作業ツリーの内容（未 commit の編集も入る）。index にあって消えた file は absent。
  # 1 行 = "<path> <sha256>"、path 順。その全体の sha256。
  # shellcheck disable=SC2086
  git ls-files -co --exclude-standard -z -- $HOST_DIRS Cargo.toml | python3 -I -c '
import hashlib, sys
paths = sorted({p for p in sys.stdin.buffer.read().decode().split("\0") if p})
h = hashlib.sha256()
for p in paths:
    try:
        with open(p, "rb") as f:
            d = hashlib.sha256(f.read()).hexdigest()
    except (FileNotFoundError, IsADirectoryError):
        d = "absent"
    h.update(f"{p} {d}\n".encode())
print(h.hexdigest())'
}

key_inputs() {
  local rustc_out
  rustc_out="$(rustc -vV)" || { echo "script-runner-cache: rustc -vV が失敗した" >&2; exit 2; }
  printf '%s\n' "$KEY_VERSION"
  printf 'host_src   %s\n' "$(host_src_hash)"
  printf 'cargo_lock %s\n' "$(sha_file Cargo.lock)"
  printf 'features   %s\n' "$FEATURES"
  printf '%s\n' "$rustc_out" | sed 's/^/rustc      /'
  printf 'schema     %s\n' "$(sha_file "$SCHEMA")"
  local rel
  for rel in $GOLDEN_COPIED; do
    printf 'copied     %s %s\n' "$rel" "$(sha_file "control-ts/fixtures/$rel")"
  done
}

if [ "$cmd" = inputs ]; then key_inputs; exit 0; fi
inputs="$(key_inputs)"
key="$(printf '%s\n' "$inputs" | sha256sum | cut -d' ' -f1)"
if [ "$cmd" = key ]; then echo "$key"; exit 0; fi

root="${CELERIS_SCRIPT_RUNNER_CACHE:-${CELERIS_SCRATCH_DIR:-/local/celeris/data/scratch}/script-runner-cache}"
root_abs="$(realpath -m "$root")"
top_abs="$(realpath -m "$top")"
case "$root_abs/" in
  /tmp/*) echo "script-runner-cache: 置き場所 $root_abs は /tmp の下（拒否）" >&2; exit 2 ;;
  "$top_abs"/*) echo "script-runner-cache: 置き場所 $root_abs は git の作業ツリーの中（拒否）" >&2; exit 2 ;;
esac
dir="$root_abs/$key"

# meta.json の runner と golden の sha256 が file と一致するか。一致すれば used_at を更新する。
verify_dir() { # dir → exit 0/1
  [ -f "$1/meta.json" ] && [ -x "$1/script-host-runner" ] || return 1
  python3 -I - "$1" <<'PY'
import hashlib, json, os, sys, time
d = sys.argv[1]
def sha(p):
    with open(p, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()
try:
    meta = json.load(open(os.path.join(d, "meta.json")))
    if sha(os.path.join(d, "script-host-runner")) != meta["runner_sha256"]:
        sys.exit(1)
    golden = meta["golden_sha256"]
    if not golden:
        sys.exit(1)
    for rel, want in golden.items():
        if sha(os.path.join(d, "golden", rel)) != want:
            sys.exit(1)
except (OSError, KeyError, ValueError):
    sys.exit(1)
meta["used_at"] = time.time()
tmp = os.path.join(d, ".meta.json.tmp")
with open(tmp, "w") as f:
    json.dump(meta, f, indent=1, sort_keys=True)
os.replace(tmp, os.path.join(d, "meta.json"))
PY
}

if [ "$cmd" = path ]; then
  if verify_dir "$dir"; then echo "$dir/script-host-runner"; exit 0; fi
  echo "script-runner-cache: key $key の cache が無い（または照合に落ちた）: $dir" >&2
  exit 1
fi

# ---- ensure ----
mkdir -p "$root_abs"
exec 9>"$root_abs/$key.lock"
flock 9
if verify_dir "$dir"; then echo "$dir/script-host-runner"; exit 0; fi

# golden の元の crate の一覧が古くないか（task-core の workspace 内の通常依存の閉包）。
closure="$(cargo metadata --format-version 1 --no-deps --locked | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
members = set(meta["workspace_members"])
pk = {p["name"]: p for p in meta["packages"] if p["id"] in members}
seen, todo = set(), sys.argv[1].split()
while todo:
    n = todo.pop()
    if n in seen or n not in pk:
        continue
    seen.add(n)
    for d in pk[n]["dependencies"]:
        if d.get("kind") in (None, "build") and d["name"] in pk:
            todo.append(d["name"])
print(" ".join(sorted(seen)))
' "$GOLDEN_CRATES")"
if [ "$closure" != "$GOLDEN_CRATES" ]; then
  echo "script-runner-cache: golden の元の crate の閉包が「$closure」で、script の一覧「$GOLDEN_CRATES」と違う。" >&2
  echo "  GOLDEN_CRATES と HOST_DIRS を直す（key に入る source が足りない）" >&2
  exit 1
fi

tmp="$root_abs/.tmp-$$"
rm -rf "$tmp"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/golden"
start="$(date +%s.%N)"
target_dir="$(cargo metadata --format-version 1 --no-deps --locked | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
echo "script-runner-cache: key $key の runner を build する（cargo build --release）" >&2
cargo build --release --locked -p celeris-script --bin script-host-runner >&2
cp "$target_dir/release/script-host-runner" "$tmp/script-host-runner"
chmod +x "$tmp/script-host-runner"

for spec in $GOLDEN_SPECS; do
  rel="${spec%%:*}"
  example="${spec#*:}"
  mkdir -p "$tmp/golden/$(dirname "$rel")"
  cargo run -q --locked -p task-core --example "$example" >"$tmp/golden/$rel"
  if ! cmp -s "$tmp/golden/$rel" "control-ts/fixtures/$rel"; then
    echo "script-runner-cache: Rust の example $example が作った golden と control-ts/fixtures/$rel が違う。" >&2
    echo "  fixture を Rust に合わせて作り直すのは Rust の変更の一部（ts-only の経路では直さない）" >&2
    cmp "$tmp/golden/$rel" "control-ts/fixtures/$rel" >&2 || true
    exit 1
  fi
done
for rel in $GOLDEN_COPIED; do
  mkdir -p "$tmp/golden/$(dirname "$rel")"
  cp "control-ts/fixtures/$rel" "$tmp/golden/$rel"
done
end="$(date +%s.%N)"

printf '%s\n' "$inputs" >"$tmp/key-inputs.txt"
python3 -I - "$tmp" "$key" "$start" "$end" "$GOLDEN_SPECS $GOLDEN_COPIED" <<'PY'
import hashlib, json, os, sys, time
d, key, start, end, specs = sys.argv[1:6]
def sha(p):
    with open(p, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()
golden = {s.split(":", 1)[0]: sha(os.path.join(d, "golden", s.split(":", 1)[0])) for s in specs.split()}
meta = {
    "key": key,
    "inputs": open(os.path.join(d, "key-inputs.txt")).read().splitlines(),
    "runner_sha256": sha(os.path.join(d, "script-host-runner")),
    "golden_sha256": golden,
    "created_at": time.time(),
    "used_at": time.time(),
    "build_secs": round(float(end) - float(start), 3),
}
with open(os.path.join(d, "meta.json"), "w") as f:
    json.dump(meta, f, indent=1, sort_keys=True)
PY
rm -rf "$dir"
mv "$tmp" "$dir"
trap - EXIT

# 最近使った KEEP 個より古い key の dir を消す（1 個は約 70 MB）。
python3 -I - "$root_abs" "$KEEP" "$key" <<'PY'
import json, os, shutil, sys
root, keep, current = sys.argv[1], int(sys.argv[2]), sys.argv[3]
dirs = []
for name in os.listdir(root):
    p = os.path.join(root, name)
    if len(name) != 64 or not os.path.isdir(p):
        continue
    try:
        used = json.load(open(os.path.join(p, "meta.json"))).get("used_at", 0)
    except (OSError, ValueError):
        used = 0
    dirs.append((used, name))
dirs.sort(reverse=True)
for _, name in dirs[keep:]:
    if name != current:
        shutil.rmtree(os.path.join(root, name), ignore_errors=True)
        try:
            os.remove(os.path.join(root, name + ".lock"))
        except OSError:
            pass
PY
echo "$dir/script-host-runner"
