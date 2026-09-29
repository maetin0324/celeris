#!/usr/bin/env bash
# browser の人待ちと Live View の e2e（gui/e2e/g14-browser-live.spec.ts）を、実 celeris daemon・実 GUI（パスワード認証）・
# 実 agent-browser dashboard・実ブラウザ（Playwright）で 1 コマンドで回す。LLM は呼ばない（fake ワーカー）。
#
#   gui/scripts/browser-live-e2e.sh
#
# やること:
#   1. cargo build -p celeris -p celerisctl -p celeris-credentiald、pnpm build（G14_SKIP_BUILD=1 / G14_SKIP_GUI_BUILD=1 で省略）
#   2. 一意な scratch directory を作る（config・DB・workspaces・token・GUI パスワード・human attestation の鍵・credentiald の scratch HOME）
#   3. celeris daemon（token 認証、fake ワーカー = test/celeris/fixtures/g14-worker.sh）→ scratch の celeris-credentiald
#      （daemon の PID だけを control に admit）→ agent-browser dashboard（+ loopback の fixture ページを開いた
#      session 1 つ）→ GUI（パスワード認証 + owner socket + CELERIS_GUI_LIVE_VIEW_UPSTREAM）を起動
#   4. pnpm exec playwright test e2e/g14-browser-live.spec.ts（スクリーンショット・events の dump は $E2E_ARTIFACTS_DIR）
#   5. 全部止め（trap EXIT）、ログを $E2E_ARTIFACTS_DIR に写し、停止後にもう一度 sentinel（credential フォームに入れた
#      パスワード）を DB・WAL・ログ・artifacts から grep -a する。見つかれば失敗。
# 終了コード: Playwright の終了コード（sentinel が見つかった・起動に失敗したときは非 0）。
#
# 注意（no LLM の代用）:
#   - browser-enabled の task は task-ops が adapter を acp / claude-code に固定し、fake ワーカーでは走らない。この e2e の task は
#     browser skill を持たない fake の task で、wait（POST /tasks/{id}/browser/requests）は trusted supervisor の代わりに spec が
#     admin token で開く（wait・登録・決定・再開の store / API / GUI は skill を見ない）。
#   - Live View の門（GUI）は `browser_updated` の RUNNING を要る。これを出すのは dispatcher の browser supervisor（実 agent と
#     agent-browser が要る）だけなので、spec が task の Running 中に scratch DB の events 表へ `browser_updated` を 1 行追記して
#     supervisor の代わりをする（sqlite3 CLI。store の append_event_tx と同じ列: task_id, seq = MAX(seq)+1, ts, json）。
#
# 環境変数（全て任意）:
#   E2E_ARTIFACTS_DIR   スクリーンショット・ログの置き場（既定 scratch directory の artifacts。空の directory を指定する）
#   G14_API_LISTEN      daemon の API（既定 127.0.0.1:27710）
#   G14_GUI_BIND        GUI（既定 127.0.0.1:27700）
#   G14_DASHBOARD_PORT  agent-browser dashboard（既定 27848）
#   G14_FIXTURE_PORT    dashboard の session が開く loopback の静的ページ（既定 27861）
#   G14_AGENT_BROWSER   agent-browser の実行ファイル
#   G14_CHROME          Chromium の実行ファイル（agent-browser の session 用）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(cd "$ROOT/.." && pwd)"
API_LISTEN="${G14_API_LISTEN:-127.0.0.1:27710}"
GUI_BIND="${G14_GUI_BIND:-127.0.0.1:27700}"
DASH_PORT="${G14_DASHBOARD_PORT:-27848}"
FIXTURE_PORT="${G14_FIXTURE_PORT:-27861}"
AGENT_BROWSER="${G14_AGENT_BROWSER:-$(command -v agent-browser || true)}"
CHROME="${G14_CHROME:-$(cd "$ROOT" && node --input-type=module -e 'import { chromium } from "@playwright/test"; process.stdout.write(chromium.executablePath());')}"

die() { echo "browser-live-e2e: $*" >&2; exit 2; }
log() { echo "browser-live-e2e: $*" >&2; }

# 運用中の Celeris（7700 / 7710）と他の spec のポートは使わない。
for p in "${API_LISTEN##*:}" "${GUI_BIND##*:}" "$DASH_PORT" "$FIXTURE_PORT"; do
  case "$p" in 7700 | 7710 | 7721 | 7722 | 7723) die "port $p is reserved (production Celeris / other specs)" ;; esac
done
command -v sqlite3 >/dev/null || die "sqlite3 is required (browser_updated seeding)"
command -v python3 >/dev/null || die "python3 is required (fixture page)"
[ -x "$AGENT_BROWSER" ] || die "set G14_AGENT_BROWSER to agent-browser 0.38.1"
[ "$("$AGENT_BROWSER" --version)" = "agent-browser 0.38.1" ] || die "agent-browser must be pinned to 0.38.1"
[ -x "$CHROME" ] || die "Chromium not found (install Playwright Chromium or set G14_CHROME)"

# Each invocation owns its scratch directory; never delete another worktree's run.
RUN_ROOT="${CELERIS_RUN_ROOT:-${TMPDIR:-/tmp}/celeris-gui-run-$(id -un)}"
mkdir -p "$RUN_ROOT"
RUN_DIR="$(mktemp -d "$RUN_ROOT/g14.XXXXXXXX")"
ARTIFACTS="${E2E_ARTIFACTS_DIR:-$RUN_DIR/artifacts}"
mkdir -p "$ARTIFACTS"
[ -z "$(find "$ARTIFACTS" -mindepth 1 -maxdepth 1 -print -quit)" ] || die "use an empty E2E_ARTIFACTS_DIR (previous evidence is preserved)"

# ---- build ----
BIN_DIR="${CARGO_TARGET_DIR:-$REPO/target}/debug"
if [ "${G14_SKIP_BUILD:-}" != "1" ]; then
  log "cargo build -p celeris -p celerisctl -p celeris-credentiald"
  (cd "$REPO" && cargo build -p celeris -p celerisctl -p celeris-credentiald)
fi
for b in celeris celerisctl celeris-credentiald; do [ -x "$BIN_DIR/$b" ] || die "binary not found: $BIN_DIR/$b"; done
if [ "${G14_SKIP_GUI_BUILD:-}" != "1" ]; then
  log "pnpm build"
  (cd "$ROOT" && pnpm build)
fi

# ---- scratch ----
for addr in "$API_LISTEN" "$GUI_BIND" "127.0.0.1:$DASH_PORT" "127.0.0.1:$FIXTURE_PORT"; do
  if (exec 3<>"/dev/tcp/${addr%:*}/${addr##*:}") 2>/dev/null; then die "something already listens on $addr; stop it first"; fi
done
mkdir -p "$RUN_DIR/workspaces" "$RUN_DIR/markers" "$ARTIFACTS"
chmod 700 "$RUN_DIR"
cp "$ROOT/test/celeris/fixtures/g14-worker.sh" "$RUN_DIR/g14-worker.sh"
cp "$ROOT/test/celeris/fixtures/read-run-request.mjs" "$RUN_DIR/read-run-request.mjs"
chmod +x "$RUN_DIR/g14-worker.sh"
rand() { head -c "$1" /dev/urandom | od -An -tx1 | tr -d ' \n'; }
(umask 177 && rand 32 > "$RUN_DIR/api.token" && printf 'pw-%s\n' "$(rand 12)" > "$RUN_DIR/gui-password")
SENTINEL="SENTINEL-$(rand 12)"
# credentiald（HOME/.config/celeris/credentiald と HOME/.local/celeris/credentiald）は scratch の HOME に閉じ込める。
CRED_HOME="$RUN_DIR/credhome"
CRED_RUNTIME="$RUN_DIR/runtime"
mkdir -m 700 "$CRED_HOME" "$CRED_HOME/.config" "$CRED_HOME/.local" "$CRED_HOME/.config/celeris" "$CRED_HOME/.local/celeris" "$CRED_RUNTIME"
CRED_SOCKET="$CRED_RUNTIME/celeris-credentiald/control.sock"
# human attestation の鍵（GUI が PKCS#8 PEM の秘密鍵で署名、daemon が hex の公開鍵で検証）。
mkdir -m 700 "$RUN_DIR/gui-keys"
node -e '
const c = require("node:crypto"), fs = require("node:fs");
const { publicKey, privateKey } = c.generateKeyPairSync("ed25519");
fs.writeFileSync(process.argv[1], privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
fs.writeFileSync(process.argv[2], Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url").toString("hex") + "\n");
' "$RUN_DIR/gui-keys/attestation.pem" "$RUN_DIR/attestation.pub"
sed -e "s#@RUN_DIR@#$RUN_DIR#g" -e "s#@API_LISTEN@#$API_LISTEN#g" -e "s#@CREDENTIALD_SOCKET@#$CRED_SOCKET#g" \
  "$ROOT/test/celeris/g14-browser-live.toml.tmpl" > "$RUN_DIR/config.toml"
OWNER_SOCKET="$RUN_DIR/owner/owner.sock"
mkdir -m 700 "$RUN_DIR/owner"
AB_HOME="$RUN_DIR/abhome"
AB_SOCKETS="$RUN_DIR/ab"
mkdir -m 700 "$AB_HOME" "$AB_SOCKETS"

DAEMON_PID="" CRED_PID="" GUI_PID="" FIXTURE_PID="" AB_SESSION=""
ab() {
  env -i PATH="$PATH" HOME="$AB_HOME" AGENT_BROWSER_NAMESPACE=celeris-e2e-live AGENT_BROWSER_SOCKET_DIR="$AB_SOCKETS" \
    "$AGENT_BROWSER" "$@"
}
stop_pid() {
  local pid="$1" i
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || return 0
  kill -TERM "$pid" 2>/dev/null || true
  for i in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.1; done
  kill -KILL "$pid" 2>/dev/null || true
}
cleanup() {
  local rc=$?
  set +e
  stop_pid "$GUI_PID"
  if [ -n "$AB_SESSION" ]; then ab --config "$RUN_DIR/ab-config.json" --session "$AB_SESSION" close >/dev/null 2>&1; fi
  ab --config "$RUN_DIR/ab-config.json" dashboard stop >/dev/null 2>&1
  stop_pid "$FIXTURE_PID"
  stop_pid "$DAEMON_PID"
  stop_pid "$CRED_PID"
  # dispatcher が止め損ねた fake ワーカー（sleep ループ）と、この namespace の agent-browser / Chromium の取り残し。
  pkill -f "$RUN_DIR/g14-worker.sh" 2>/dev/null
  pkill -f "$AB_SOCKETS" 2>/dev/null
  pkill -f "$AB_HOME" 2>/dev/null
  for f in celeris.log gui.log credentiald.log dashboard.log fixture-http.log; do
    [ -f "$RUN_DIR/$f" ] && cp "$RUN_DIR/$f" "$ARTIFACTS/$f"
  done
  # 停止後の sentinel 走査（DB・WAL・shm・全ログ・workspaces・credentiald の vault・artifacts）。
  local hits
  hits="$(grep -rlaF -- "$SENTINEL" "$RUN_DIR" "$ARTIFACTS" 2>/dev/null)"
  if [ -n "$hits" ]; then
    echo "browser-live-e2e: FAIL: the credential sentinel leaked into:" >&2
    echo "$hits" >&2
    [ "$rc" -eq 0 ] && rc=1
  else
    echo "browser-live-e2e: sentinel scan after shutdown: 0 hits ($(find "$RUN_DIR" "$ARTIFACTS" -type f | wc -l) files)" >&2
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ---- daemon ----
log "starting celeris on $API_LISTEN"
( cd "$RUN_DIR" && exec "$BIN_DIR/celeris" --config "$RUN_DIR/config.toml" --log-format text >> "$RUN_DIR/celeris.log" 2>&1 ) &
DAEMON_PID=$!
ok=""
for _ in $(seq 1 100); do
  kill -0 "$DAEMON_PID" 2>/dev/null || { tail -n 30 "$RUN_DIR/celeris.log" >&2; die "celeris exited early"; }
  if curl -sf -o /dev/null "http://$API_LISTEN/api/v1/health"; then ok=1; break; fi
  sleep 0.2
done
[ -n "$ok" ] || die "celeris did not answer http://$API_LISTEN/api/v1/health"

# ---- credentiald（daemon の PID だけを control socket に admit）----
log "starting scratch celeris-credentiald for daemon pid $DAEMON_PID"
HOME="$CRED_HOME" XDG_RUNTIME_DIR="$CRED_RUNTIME" "$BIN_DIR/celeris-credentiald" init
( HOME="$CRED_HOME" XDG_RUNTIME_DIR="$CRED_RUNTIME" exec "$BIN_DIR/celeris-credentiald" serve "$DAEMON_PID" >> "$RUN_DIR/credentiald.log" 2>&1 ) &
CRED_PID=$!
for _ in $(seq 1 100); do [ -S "$CRED_SOCKET" ] && break; sleep 0.1; done
[ -S "$CRED_SOCKET" ] || { cat "$RUN_DIR/credentiald.log" >&2; die "credentiald control socket did not appear"; }

# ---- agent-browser dashboard（Live View の upstream）----
printf '{"executablePath":"%s","args":"--no-sandbox"}\n' "$CHROME" > "$RUN_DIR/ab-config.json"
mkdir -p "$RUN_DIR/fixture-site"
printf '<!doctype html><title>g14 Live View fixture</title><h1>g14 Live View fixture</h1><p>loopback page, no credentials.</p>\n' \
  > "$RUN_DIR/fixture-site/index.html"
( cd "$RUN_DIR/fixture-site" && exec python3 -m http.server "$FIXTURE_PORT" --bind 127.0.0.1 >> "$RUN_DIR/fixture-http.log" 2>&1 ) &
FIXTURE_PID=$!
DASH_SESSIONS=1
AB_SESSION="g14-live-$(rand 4)"
sleep 0.5
ab --config "$RUN_DIR/ab-config.json" --session "$AB_SESSION" --allowed-domains 127.0.0.1 --json \
  open "http://127.0.0.1:$FIXTURE_PORT/" >> "$RUN_DIR/dashboard.log" 2>&1 \
  || die "could not open an agent-browser session (see dashboard.log)"
log "starting agent-browser dashboard on 127.0.0.1:$DASH_PORT ($DASH_SESSIONS session)"
# stdout は捨てる（dashboard の bootstrap token を記録しない）。
ab --config "$RUN_DIR/ab-config.json" dashboard start --port "$DASH_PORT" > /dev/null 2>> "$RUN_DIR/dashboard.log"
ok=""
for _ in $(seq 1 100); do
  if curl -sf -o /dev/null "http://127.0.0.1:$DASH_PORT/"; then ok=1; break; fi
  sleep 0.2
done
[ -n "$ok" ] || die "agent-browser dashboard did not answer on 127.0.0.1:$DASH_PORT"

# ---- GUI ----
log "starting GUI on $GUI_BIND (password auth, owner socket, Live View upstream 127.0.0.1:$DASH_PORT)"
(
  cd "$ROOT"
  export CELERIS_GUI_BIND="$GUI_BIND" CELERIS_API_URL="http://$API_LISTEN" CELERIS_API_TOKEN_FILE="$RUN_DIR/api.token" \
    CELERIS_GUI_PASSWORD_FILE="$RUN_DIR/gui-password" CELERIS_GUI_OWNER_SOCKET="$OWNER_SOCKET" \
    CELERIS_GUI_ATTESTATION_KEY_FILE="$RUN_DIR/gui-keys/attestation.pem" \
    CELERIS_GUI_LIVE_VIEW_UPSTREAM="127.0.0.1:$DASH_PORT" NODE_ENV=production
  exec node server.js >> "$RUN_DIR/gui.log" 2>&1
) &
GUI_PID=$!
ok=""
for _ in $(seq 1 100); do
  kill -0 "$GUI_PID" 2>/dev/null || { cat "$RUN_DIR/gui.log" >&2; die "GUI exited early"; }
  if curl -sf -o /dev/null "http://$GUI_BIND/healthz"; then ok=1; break; fi
  sleep 0.2
done
[ -n "$ok" ] || die "GUI did not answer http://$GUI_BIND/healthz"

# ---- Playwright ----
log "running e2e/g14-browser-live.spec.ts (artifacts: $ARTIFACTS)"
set +e
(
  cd "$ROOT"
  CELERIS_E2E_EXTERNAL_GUI=1 CELERIS_GUI_BIND="$GUI_BIND" CELERIS_API_URL="http://$API_LISTEN" \
    E2E_ARTIFACTS_DIR="$ARTIFACTS" G14_RUN_DIR="$RUN_DIR" G14_SENTINEL="$SENTINEL" G14_OWNER_SOCKET="$OWNER_SOCKET" \
    G14_CELERISCTL="$BIN_DIR/celerisctl" G14_DASHBOARD_PORT="$DASH_PORT" G14_DASHBOARD_SESSIONS="$DASH_SESSIONS" \
    pnpm exec playwright test e2e/g14-browser-live.spec.ts "$@"
) 2>&1 | tee "$ARTIFACTS/playwright.log"
PW_RC=${PIPESTATUS[0]}
set -e
log "playwright exit code $PW_RC"
exit "$PW_RC"
