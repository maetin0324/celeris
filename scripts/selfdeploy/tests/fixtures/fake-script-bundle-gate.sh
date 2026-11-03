#!/usr/bin/env bash
# release.sh の SD_SCRIPT_BUNDLE_GATE に渡す偽の script-bundle-gate.sh（control-ts を持たない偽の木の release 試験用）。
# bundle で dist に bundle.mjs と manifest.json（bundle_sha256 は本物の sha256）を書き、他の段は引数だけ確かめて通す。
# 本物の段の挙動は scripts/dev/test-release-script-gate.sh が確かめる。
set -euo pipefail
case "$1" in
  install) [ $# -eq 2 ] ;;
  bundle)
    mkdir -p "$2/dist"
    printf 'export const v = 1;\n' >"$2/dist/bundle.mjs"
    printf '{"schema":"celeris.script-manifest/1","bundle_sha256":"%s"}\n' \
      "$(sha256sum "$2/dist/bundle.mjs" | cut -d' ' -f1)" >"$2/dist/manifest.json"
    ;;
  verify) [ $# -eq 4 ] && [ -f "$3/manifest.json" ] ;;
  test) [ "${CELERIS_SCRIPT_REQUIRE_BUNDLE:-}" = 1 ] && [ "${CELERIS_SCRIPT_BUNDLE_DIR:-}" = "$3" ] ;;
  *) exit 2 ;;
esac
