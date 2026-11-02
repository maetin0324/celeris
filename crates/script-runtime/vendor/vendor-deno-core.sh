#!/bin/sh
# deno_core の写しを crates.io の .crate から機械的に作り直す。
# ADR agent-docs/adr/2026-11-02-deno-core-vendored-serde-json-features.md D2。
#   sh crates/script-runtime/vendor/vendor-deno-core.sh          # 写しを置き換える
#   sh crates/script-runtime/vendor/vendor-deno-core.sh --check  # repo の写しが作り直した物と同じか確かめる
set -eu
here=$(cd "$(dirname "$0")" && pwd)
lock="$here/deno_core.lock.toml"
dest="$here/deno_core"
mode=write
case "${1:-}" in
  --check) mode=check ;;
  "") ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

field() { sed -n "s/^$1 = \"\\(.*\\)\"\$/\\1/p" "$lock"; }
name=$(field name)
version=$(field version)
sha=$(field crate_sha256)
features=$(sed -n 's/^removed_serde_json_features = \[\(.*\)\]$/\1/p' "$lock" | tr -d '" ' | tr ',' ' ')
[ -n "$name" ] && [ -n "$version" ] && [ -n "$sha" ] && [ -n "$features" ] || { echo "bad $lock" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/vendor-deno-core.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

crate=""
for c in "${CARGO_HOME:-$HOME/.cargo}"/registry/cache/*/"$name-$version.crate"; do
  [ -f "$c" ] && { crate=$c; break; }
done
if [ -z "$crate" ]; then
  crate="$work/$name-$version.crate"
  curl -fsSL -o "$crate" "https://static.crates.io/crates/$name/$name-$version.crate"
fi
echo "$sha  $crate" | sha256sum -c --quiet - || { echo "sha256 mismatch: $crate" >&2; exit 1; }

mkdir "$work/x"
tar -xzf "$crate" -C "$work/x"
src="$work/x/$name-$version"
[ -f "$src/Cargo.toml" ] || { echo "no Cargo.toml in $crate" >&2; exit 1; }
cp "$src/Cargo.toml" "$work/Cargo.toml.crates-io"

# [dependencies.serde_json] の features 配列から記録の feature の行だけを消す。
awk -v drop="$features" '
  BEGIN { n = split(drop, d, " "); for (i = 1; i <= n; i++) rm["\"" d[i] "\","] = 1 }
  /^\[/ { sect = $0 }
  { line = $0; t = line; gsub(/^[ \t]+|[ \t]+$/, "", t) }
  sect == "[dependencies.serde_json]" && (t in rm) { removed++; next }
  { print line }
  END { if (removed != n) { printf "removed %d of %d features\n", removed, n > "/dev/stderr"; exit 1 } }
' "$work/Cargo.toml.crates-io" > "$src/Cargo.toml"

# 差分が消した feature の行だけであることを確かめる。
got=$(diff "$work/Cargo.toml.crates-io" "$src/Cargo.toml" | grep '^[<>]' | sed 's/^< *//' | tr -d '",' | sort | tr '\n' ' ')
want=$(printf '%s\n' $features | sort | tr '\n' ' ')
[ "$got" = "$want" ] || { echo "unexpected Cargo.toml diff: '$got' (want '$want')" >&2; exit 1; }

if [ "$mode" = check ]; then
  diff -r "$src" "$dest" >/dev/null || { echo "vendored $name differs from $name-$version.crate + patch" >&2; diff -rq "$src" "$dest" >&2 || true; exit 1; }
  echo "ok: $dest = $name-$version.crate ($sha) - serde_json features: $features"
else
  rm -rf "$dest"
  cp -a "$src" "$dest"
  echo "wrote $dest from $name-$version.crate ($sha)"
fi
