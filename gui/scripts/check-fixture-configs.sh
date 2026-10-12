#!/usr/bin/env bash
# Build celeris and prove every GUI e2e config fixture can start and answer /health.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$ROOT/.." && pwd)"
BIN="${CARGO_TARGET_DIR:-$REPO/target}/debug/celeris"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/celeris-fixtures.XXXXXX")"
child=""
cleanup() {
  if [[ -n "$child" ]]; then kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true; fi
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

cargo build -p celeris -p celerisctl --manifest-path "$REPO/Cargo.toml"

for tmpl in "$ROOT"/test/celeris/*.toml.tmpl; do
  name="$(basename "$tmpl" .toml.tmpl)"
  dir="$TMP/$name"
  mkdir -p "$dir"
  port="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
  sed -e "s#@RUN_DIR@#$dir#g" \
      -e "s#@API_LISTEN@#127.0.0.1:$port#g" \
      -e "s#@CREDENTIALD_SOCKET@#$dir/credentiald.sock#g" \
      "$tmpl" > "$dir/config.toml"
  if grep -q 'token_file = "api.token"' "$dir/config.toml"; then printf 'fixture-token\n' > "$dir/api.token"; fi
  if [[ "$name" == g14-browser-live ]]; then
    node -e 'const c=require("node:crypto"),fs=require("node:fs");const {publicKey}=c.generateKeyPairSync("ed25519");fs.writeFileSync(process.argv[1],Buffer.from(publicKey.export({format:"jwk"}).x,"base64url").toString("hex")+"\n")' "$dir/attestation.pub"
  fi
  if [[ "$name" == org ]]; then cp "$REPO/config/org.example.toml" "$dir/org.toml"; fi
  "$BIN" --config "$dir/config.toml" >"$dir/celeris.log" 2>&1 &
  child=$!
  ok=0
  auth=()
  if [[ -f "$dir/api.token" ]]; then auth=(-H 'Authorization: Bearer fixture-token'); fi
  for _ in $(seq 1 100); do
    if curl --silent --fail "${auth[@]}" "http://127.0.0.1:$port/api/v1/health" >/dev/null; then ok=1; break; fi
    if ! kill -0 "$child" 2>/dev/null; then break; fi
    sleep 0.1
  done
  if [[ $ok != 1 ]]; then
    echo "fixture $name failed to start or answer /health" >&2
    cat "$dir/celeris.log" >&2
    exit 1
  fi
  kill "$child" 2>/dev/null || true
  wait "$child" 2>/dev/null || true
  child=""
  echo "fixture $name: ok"
done
