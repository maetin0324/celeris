#!/usr/bin/env bash
# 変更 crate とその逆依存 crate だけの試験を走らせる（ADR 2026-10-20 full-tests-at-release-only）。
# 全体の試験（test-parallel.sh・web e2e・本物 browser/userns 試験）は release gate だけ。
#   bash scripts/dev/test-affected.sh          nextest + doc test を対象 crate に走らせる
#   bash scripts/dev/test-affected.sh --list   対象 crate 名を 1 行ずつ出すだけ（何も走らせない）
# 基準は ${CELERIS_WU_BASE:-$(git merge-base HEAD main)}。差分は基準との git diff と未追跡 file。
# Cargo.lock・ルート Cargo.toml・.config/nextest.toml・rust-toolchain の変更は全 crate 扱い（stderr に警告）。
# crates に差分がなく web/ だけなら web の unit 試験（e2e でない test script のみ）。docs だけなら何もせず exit 0。
set -euo pipefail

list_only=0
case "${1:-}" in
  --list) list_only=1 ;;
  "") ;;
  *) echo "usage: test-affected.sh [--list]" >&2; exit 2 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "$root"
base="${CELERIS_WU_BASE:-$(git merge-base HEAD main)}"

changed="$(mktemp "${TMPDIR:-/tmp}/test-affected.XXXXXX")"
trap 'rm -f "$changed"' EXIT
{
  git diff --name-only "$base" --
  git ls-files --others --exclude-standard
} | sort -u >"$changed"

all=0
web=0
while IFS= read -r f; do
  case "$f" in
    Cargo.lock|Cargo.toml|.config/nextest.toml|rust-toolchain|rust-toolchain.toml) all=1 ;;
    web/*) web=1 ;;
  esac
done <"$changed"

if [ "$all" -eq 1 ]; then
  echo "test-affected: workspace 共通 file が変わったため全 crate を対象にする" >&2
fi

# 変更 file → crate（manifest dir の最長一致）→ 逆依存閉包。workspace 内の依存（dev 含む）だけを辿る。
crates="$(cargo metadata --format-version 1 --no-deps | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
all_crates = sys.argv[1] == "1"
root = meta["workspace_root"].rstrip("/") + "/"
pk = [p for p in meta["packages"] if p["id"] in set(meta["workspace_members"])]
names = {p["name"] for p in pk}
dirs = {}
for p in pk:
    d = p["manifest_path"].rsplit("/", 1)[0] + "/"
    dirs[p["name"]] = d[len(root):] if d.startswith(root) else d
rdeps = {n: set() for n in names}
for p in pk:
    for d in p["dependencies"]:
        if d["name"] in names:
            rdeps[d["name"]].add(p["name"])
hit = set(names) if all_crates else set()
for f in open(sys.argv[2]).read().split("\n"):
    if not f:
        continue
    best = None
    for n, d in dirs.items():
        if (f + "/").startswith(d) or f.startswith(d):
            if best is None or len(d) > len(dirs[best]):
                best = n
    if best:
        hit.add(best)
todo = list(hit)
while todo:
    n = todo.pop()
    for r in rdeps[n]:
        if r not in hit:
            hit.add(r)
            todo.append(r)
print("\n".join(sorted(hit)))
' "$all" "$changed")"

if [ "$list_only" -eq 1 ]; then
  [ -z "$crates" ] || printf '%s\n' "$crates"
  exit 0
fi

if [ -n "$crates" ]; then
  args=()
  while IFS= read -r c; do args+=(-p "$c"); done <<<"$crates"
  echo "test-affected: crates: $(echo "$crates" | tr '\n' ' ')" >&2
  cargo nextest run "${args[@]}"
  cargo test --doc "${args[@]}"
  exit 0
fi

if [ "$web" -eq 1 ]; then
  script="$(python3 -I -c 'import json;print(json.load(open("web/package.json")).get("scripts",{}).get("test",""))')"
  case "$script" in
    "") echo "test-affected: web/package.json に test script がない。何も走らせない" >&2; exit 0 ;;
    *e2e*|*playwright*) echo "test-affected: web の test script が e2e のため走らせない: $script" >&2; exit 0 ;;
  esac
  pm="$(python3 -I -c 'import json;print(json.load(open("web/package.json")).get("packageManager",""))')"
  ver="${pm#pnpm@}"
  ver="${ver%%+*}"
  echo "test-affected: web unit 試験 ($script)" >&2
  exec corepack "pnpm@$ver" -C web test
fi

echo "test-affected: 試験対象の変更なし（docs のみ等）" >&2
exit 0
