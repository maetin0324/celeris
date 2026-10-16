#!/usr/bin/env bash
# Build a standalone web gateway bundle with a private offline pnpm store (prod deps only).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(node -p "require('./package.json').version")"
RELEASE="${CELERIS_WEB_RELEASE:-$(git rev-parse --short=12 HEAD)}"
[[ "$RELEASE" =~ ^[a-zA-Z0-9._-]+$ ]] || { echo "invalid release identifier" >&2; exit 1; }

./node_modules/.bin/vite build
STAGE_ROOT="$(mktemp -d)"
trap 'rm -rf "$STAGE_ROOT"' EXIT
NAME="celeris-web-$VERSION-$RELEASE"
STAGE="$STAGE_ROOT/$NAME"
mkdir -p "$STAGE/deploy"
cp -a dist server pnpm-lock.yaml pnpm-workspace.yaml "$STAGE/"
cp deploy/celeris-web.service "$STAGE/deploy/"
printf '\nstoreDir: .pnpm-store\n' >> "$STAGE/pnpm-workspace.yaml"
node - "$STAGE/package.json" "$RELEASE" <<'NODE'
const fs = require('node:fs');
const pkg = require('./package.json');
pkg.release = process.argv[3];
fs.writeFileSync(process.argv[2], `${JSON.stringify(pkg, null, 2)}\n`);
NODE

# 共有 store（`pnpm store path`）は host の他の pnpm が同時に書き換える。丸ごと写すと壊れた写しが入るので、
# 配布物専用の空 store に lockfile の prod 依存だけを fetch する（ADR 2026-10-16-web-bundle-private-store）。
corepack pnpm@12.6.0 -C "$STAGE" fetch --prod
corepack pnpm@12.6.0 -C "$STAGE" install --prod --offline --frozen-lockfile
rm -rf "$STAGE/node_modules"

mkdir -p release
tar -czf "release/$NAME.tar.gz" -C "$STAGE_ROOT" "$NAME"
printf '%s\n' "release/$NAME.tar.gz"
